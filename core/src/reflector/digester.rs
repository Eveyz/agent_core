//! Pure, deterministic digester heuristics. The *decision to suggest* is
//! always rule-based and auditable — no LLM is involved in detection.
//!
//! Rules (from the Phase F design):
//! - 3+ consecutive errors on the same tool → suggest a SKILL with correct usage.
//! - Same tool-call pattern repeated 3+ turns → flag looping, suggest a breaker skill.
//! - Many `ApprovalRequired` followed by `DenyPersistent` → suggest tighter defaults (manual).

use super::suggestion::{Suggestion, SuggestionKind};
use super::TraceRecord;
use std::collections::HashMap;

/// A named digester rule. Pure for testability.
#[derive(Debug, Clone, Copy)]
pub struct DigesterRule {
    pub name: &'static str,
}

impl DigesterRule {
    pub const fn consecutive_tool_errors() -> Self {
        Self { name: "consecutive_tool_errors" }
    }
    pub const fn tool_loop() -> Self {
        Self { name: "tool_loop" }
    }
    pub const fn frequent_denials() -> Self {
        Self { name: "frequent_denials" }
    }
}

/// The digester. Stateless; safe to reuse.
#[derive(Debug, Default)]
pub struct Digester;

impl Digester {
    pub fn analyze(&self, records: &[TraceRecord]) -> Vec<Suggestion> {
        let mut out = Vec::new();
        out.extend(self.consecutive_tool_errors(records));
        out.extend(self.tool_loop(records));
        out.extend(self.frequent_denials(records));
        out
    }

    /// 3+ consecutive `ToolExecutionEnd{is_error:true}` for the same tool.
    fn consecutive_tool_errors(&self, records: &[TraceRecord]) -> Vec<Suggestion> {
        let mut streaks: HashMap<String, u32> = HashMap::new();
        let mut fired: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();

        for r in records {
            if let Some(snap) = r.as_tool_end() {
                if snap.is_error {
                    let count = streaks.entry(snap.tool_name.clone()).or_insert(0);
                    *count += 1;
                    if *count >= 3 && !fired.get(&snap.tool_name).copied().unwrap_or(false) {
                        fired.insert(snap.tool_name.clone(), true);
                        out.push(Suggestion {
                            id: format!("skill-{}-errors", slug(&snap.tool_name)),
                            kind: SuggestionKind::AppendSkill,
                            target: format!("{}-usage-guide", slug(&snap.tool_name)),
                            rationale: format!(
                                "Tool '{}' failed {} times in a row; a skill guiding correct usage may prevent recurrence.",
                                snap.tool_name, count
                            ),
                            detected_at: r.ts,
                            skill_triggers: vec![snap.tool_name.clone()],
                            skill_body: Some(format!(
                                "# {} usage guide\n\nWhen calling {}, ensure:\n- arguments are valid JSON\n- paths exist before access\n- check the result for `Error:` prefixes\n",
                                snap.tool_name, snap.tool_name
                            )),
                        });
                    }
                } else {
                    streaks.remove(&snap.tool_name);
                }
            }
        }
        out
    }

    /// Same tool called 3+ times across distinct turns → looping signal.
    fn tool_loop(&self, records: &[TraceRecord]) -> Vec<Suggestion> {
        let mut per_turn: HashMap<usize, Vec<String>> = HashMap::new();
        for r in records {
            if let Some(turn) = r.as_turn_start() {
                per_turn.entry(turn).or_default();
            }
            if let Some(snap) = r.as_tool_end() {
                // attribute to the most recent turn seen
                if let Some(turn) = per_turn.keys().copied().max() {
                    per_turn.entry(turn).or_default().push(snap.tool_name.clone());
                }
            }
        }

        let mut tool_turn_counts: HashMap<String, usize> = HashMap::new();
        for tools in per_turn.values() {
            for t in tools {
                *tool_turn_counts.entry(t.clone()).or_insert(0) += 1;
            }
        }

        tool_turn_counts
            .into_iter()
            .filter(|(_, c)| *c >= 3)
            .map(|(tool, count)| Suggestion {
                id: format!("skill-{}-loop", slug(&tool)),
                kind: SuggestionKind::AppendSkill,
                target: format!("{}-loop-breaker", slug(&tool)),
                rationale: format!(
                    "'{}' was called across {count} turns — possible loop. A breaker skill can cap retries or switch strategy.",
                    tool
                ),
                detected_at: chrono::Utc::now(),
                skill_triggers: vec![tool.clone()],
                skill_body: Some(format!(
                    "# {} loop breaker\n\nIf {} has been called repeatedly without progress:\n- stop and summarize what was tried\n- ask the user for clarification\n- switch to an alternative approach\n",
                    tool, tool
                )),
            })
            .collect::<Vec<_>>()
    }

    /// Frequent `Error` events mentioning permission denial → manual suggestion
    /// to review defaults. (Never auto-writes permissions.)
    fn frequent_denials(&self, records: &[TraceRecord]) -> Vec<Suggestion> {
        let denials = records
            .iter()
            .filter_map(|r| r.as_error())
            .filter(|e| e.contains("Permission denied") || e.contains("not approved"))
            .count();

        if denials >= 3 {
            vec![Suggestion {
                id: "review-permission-defaults".into(),
                kind: SuggestionKind::PermissionChange,
                target: "permissions.mode".into(),
                rationale: format!(
                    "{denials} permission denials recorded; review whether default permissions are too strict or the task needs escalation (manual review)."
                ),
                detected_at: chrono::Utc::now(),
                skill_triggers: vec![],
                skill_body: None,
            }]
        } else {
            vec![]
        }
    }
}

fn slug(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_end(name: &str, is_error: bool) -> TraceRecord {
        TraceRecord {
            ts: chrono::Utc::now(),
            event: json!({
                "ToolExecutionEnd": {
                    "tool_call_id": "c",
                    "tool_name": name,
                    "result": if is_error { "Error: boom" } else { "ok" },
                    "is_error": is_error,
                }
            }),
        }
    }

    fn turn_start(i: usize) -> TraceRecord {
        TraceRecord {
            ts: chrono::Utc::now(),
            event: json!({ "TurnStart": { "turn_index": i } }),
        }
    }

    #[test]
    fn test_consecutive_errors_fires_skill() {
        let recs = vec![
            tool_end("bash", true),
            tool_end("bash", true),
            tool_end("bash", true),
        ];
        let d = Digester;
        let s = d.analyze(&recs);
        assert!(s.iter().any(|s| s.kind == SuggestionKind::AppendSkill
            && s.target.contains("bash")));
    }

    #[test]
    fn test_consecutive_errors_resets_on_success() {
        let recs = vec![
            tool_end("bash", true),
            tool_end("bash", true),
            tool_end("bash", false), // resets
            tool_end("bash", true),
        ];
        let d = Digester;
        let s = d.analyze(&recs);
        // 2 then 1 — never reaches 3
        assert!(!s.iter().any(|s| s.target.contains("usage-guide")));
    }

    #[test]
    fn test_tool_loop_detected_across_turns() {
        let recs = vec![
            turn_start(0),
            tool_end("grep", false),
            turn_start(1),
            tool_end("grep", false),
            turn_start(2),
            tool_end("grep", false),
        ];
        let d = Digester;
        let s = d.analyze(&recs);
        assert!(s.iter().any(|s| s.target.contains("loop-breaker")));
    }

    #[test]
    fn test_frequent_denials_emits_manual_only() {
        let recs: Vec<TraceRecord> = (0..4)
            .map(|_| TraceRecord {
                ts: chrono::Utc::now(),
                event: json!("Error"), // tag-only; as_error needs string value
            })
            .collect();
        // Build proper Error events: AgentEvent::Error(String) serializes as
        // {"Error": "message"}.
        let recs: Vec<TraceRecord> = (0..4)
            .map(|_| TraceRecord {
                ts: chrono::Utc::now(),
                event: json!({"Error": "Permission denied by user"}),
            })
            .collect();
        let d = Digester;
        let s = d.analyze(&recs);
        assert!(s.iter().any(|s| s.kind == SuggestionKind::PermissionChange));
    }

    #[test]
    fn test_no_false_positives_on_clean_trace() {
        let recs = vec![
            turn_start(0),
            tool_end("bash", false),
            tool_end("grep", false),
        ];
        let d = Digester;
        assert!(d.analyze(&recs).is_empty());
    }
}
