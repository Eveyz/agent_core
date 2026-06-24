//! Pure, deterministic digester heuristics. The *decision to suggest* is
//! always rule-based and auditable — no LLM is involved in detection.
//!
//! Rules (from the Phase F design):
//! - 3+ consecutive errors on the same tool → suggest a SKILL with correct usage.
//! - Same tool-call pattern repeated 3+ turns → flag looping, suggest a breaker skill.
//! - Many `ApprovalRequired` followed by `DenyPersistent` → suggest tighter defaults (manual).

use super::suggestion::{Suggestion, SuggestionKind};
use std::collections::HashMap;

/// Normalized event the digester operates on.
/// Decoupled from both AgentEvent and RunEvent so it works with either trace source.
#[derive(Debug, Clone)]
pub struct DigestEvent {
    pub kind: DigestEventKind,
    pub tool_name: Option<String>,
    pub is_error: bool,
    pub message: Option<String>,
    pub turn_index: Option<usize>,
    pub ts: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestEventKind {
    TurnStart,
    ToolEnd,
    Error,
}

/// A named digester rule. Pure for testability.
#[derive(Debug, Clone, Copy)]
pub struct DigesterRule {
    pub name: &'static str,
}

impl DigesterRule {
    pub const fn consecutive_tool_errors() -> Self {
        Self {
            name: "consecutive_tool_errors",
        }
    }
    pub const fn tool_loop() -> Self {
        Self { name: "tool_loop" }
    }
    pub const fn frequent_denials() -> Self {
        Self {
            name: "frequent_denials",
        }
    }
}

/// The digester. Stateless; safe to reuse.
#[derive(Debug, Default)]
pub struct Digester;

impl Digester {
    pub fn analyze(&self, events: &[DigestEvent]) -> Vec<Suggestion> {
        let mut out = Vec::new();
        out.extend(self.consecutive_tool_errors(events));
        out.extend(self.tool_loop(events));
        out.extend(self.frequent_denials(events));
        out
    }

    /// 3+ consecutive tool errors for the same tool.
    fn consecutive_tool_errors(&self, events: &[DigestEvent]) -> Vec<Suggestion> {
        let mut streaks: HashMap<String, u32> = HashMap::new();
        let mut fired: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();

        for ev in events {
            if ev.kind != DigestEventKind::ToolEnd {
                continue;
            }
            if let Some(ref name) = ev.tool_name {
                if ev.is_error {
                    let count = streaks.entry(name.clone()).or_insert(0);
                    *count += 1;
                    if *count >= 3 && !fired.get(name).copied().unwrap_or(false) {
                        fired.insert(name.clone(), true);
                        out.push(Suggestion {
                            id: format!("skill-{}-errors", slug(name)),
                            kind: SuggestionKind::AppendSkill,
                            target: format!("{}-usage-guide", slug(name)),
                            rationale: format!(
                                "Tool '{}' failed {} times in a row; a skill guiding correct usage may prevent recurrence.",
                                name, count
                            ),
                            detected_at: ev.ts,
                            skill_triggers: vec![name.clone()],
                            skill_body: Some(format!(
                                "# {} usage guide\n\nWhen calling {}, ensure:\n- arguments are valid JSON\n- paths exist before access\n- check the result for `Error:` prefixes\n",
                                name, name
                            )),
                        });
                    }
                } else {
                    streaks.remove(name);
                }
            }
        }
        out
    }

    /// Same tool called 3+ times across distinct turns → looping signal.
    fn tool_loop(&self, events: &[DigestEvent]) -> Vec<Suggestion> {
        let mut per_turn: HashMap<usize, Vec<String>> = HashMap::new();
        let mut current_turn: Option<usize> = None;

        for ev in events {
            match ev.kind {
                DigestEventKind::TurnStart => {
                    if let Some(t) = ev.turn_index {
                        current_turn = Some(t);
                        per_turn.entry(t).or_default();
                    }
                }
                DigestEventKind::ToolEnd => {
                    if let Some(ref name) = ev.tool_name {
                        if let Some(t) = current_turn {
                            per_turn.entry(t).or_default().push(name.clone());
                        }
                    }
                }
                _ => {}
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

    /// Frequent errors mentioning permission denial → manual suggestion
    /// to review defaults. (Never auto-writes permissions.)
    fn frequent_denials(&self, events: &[DigestEvent]) -> Vec<Suggestion> {
        let denials = events
            .iter()
            .filter(|ev| ev.kind == DigestEventKind::Error)
            .filter_map(|ev| ev.message.as_deref())
            .filter(|m| m.contains("Permission denied") || m.contains("not approved"))
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

    fn tool_end(name: &str, is_error: bool) -> DigestEvent {
        DigestEvent {
            kind: DigestEventKind::ToolEnd,
            tool_name: Some(name.to_string()),
            is_error,
            message: None,
            turn_index: None,
            ts: chrono::Utc::now(),
        }
    }

    fn turn_start(i: usize) -> DigestEvent {
        DigestEvent {
            kind: DigestEventKind::TurnStart,
            tool_name: None,
            is_error: false,
            message: None,
            turn_index: Some(i),
            ts: chrono::Utc::now(),
        }
    }

    fn error_event(msg: &str) -> DigestEvent {
        DigestEvent {
            kind: DigestEventKind::Error,
            tool_name: None,
            is_error: true,
            message: Some(msg.to_string()),
            turn_index: None,
            ts: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_consecutive_errors_fires_skill() {
        let events = vec![
            tool_end("bash", true),
            tool_end("bash", true),
            tool_end("bash", true),
        ];
        let d = Digester;
        let s = d.analyze(&events);
        assert!(
            s.iter()
                .any(|s| s.kind == SuggestionKind::AppendSkill && s.target.contains("bash"))
        );
    }

    #[test]
    fn test_consecutive_errors_resets_on_success() {
        let events = vec![
            tool_end("bash", true),
            tool_end("bash", true),
            tool_end("bash", false), // resets
            tool_end("bash", true),
        ];
        let d = Digester;
        let s = d.analyze(&events);
        assert!(!s.iter().any(|s| s.target.contains("usage-guide")));
    }

    #[test]
    fn test_tool_loop_detected_across_turns() {
        let events = vec![
            turn_start(0),
            tool_end("grep", false),
            turn_start(1),
            tool_end("grep", false),
            turn_start(2),
            tool_end("grep", false),
        ];
        let d = Digester;
        let s = d.analyze(&events);
        assert!(s.iter().any(|s| s.target.contains("loop-breaker")));
    }

    #[test]
    fn test_frequent_denials_emits_manual_only() {
        let events: Vec<DigestEvent> = (0..4)
            .map(|_| error_event("Permission denied by user"))
            .collect();
        let d = Digester;
        let s = d.analyze(&events);
        assert!(s.iter().any(|s| s.kind == SuggestionKind::PermissionChange));
    }

    #[test]
    fn test_no_false_positives_on_clean_trace() {
        let events = vec![
            turn_start(0),
            tool_end("bash", false),
            tool_end("grep", false),
        ];
        let d = Digester;
        assert!(d.analyze(&events).is_empty());
    }
}
