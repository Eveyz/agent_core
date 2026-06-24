//! Offline reflection framework (Phase F) — reads execution traces produced by
//! [`crate::trace::TraceCollector`] and produces improvement *suggestions*.
//!
//! ## Safety model (the most important part)
//!
//! This is a self-modifying system, so it is fenced by a strict allow-list:
//!
//! | Suggestion kind | Auto-apply? | Why |
//! |---|---|---|
//! | Append a non-destructive `SKILL.md` | ✅ yes (idempotent, reversible) | low risk, high value |
//! | Adjust memory consolidation threshold | ❌ no (manual) | affects long-term memory |
//! | Change `permissions.mode` / blacklist | ❌ no (forbidden) | privilege-escalation risk |
//! | Change `api_key` / `base_url` / `model_id` | ❌ no (forbidden) | credential/routing risk |
//! | Change `max_iterations` / token caps | ❌ no (manual) | behavior-boundary risk |
//!
//! Only the first kind is ever written to disk automatically. Everything else
//! is surfaced as a diff for human approval via the existing
//! [`crate::types::AgentEvent::ApprovalRequired`] channel. The forbidden
//! fields can never be written by the reflector, even with approval.
//!
//! The reflector is **off by default** and must be explicitly enabled.
//!
//! ## Digester
//!
//! The digester runs pure, deterministic heuristics over the trace — no LLM
//! required for the core detection. An optional LLM-backed explanation step
//! can enrich a suggestion's rationale, but the *decision to suggest* is
//! always rule-based and auditable.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::client::OpenAIClient;
use crate::types::Message;

pub mod digester;
pub mod suggestion;

pub use digester::{Digester, DigesterRule};
pub use suggestion::{
    SAFE_AUTO_APPLY, SECURITY_FIELDS, Suggestion, SuggestionAction, SuggestionKind,
};

/// The reflector: reads a trace, runs the digester, and emits suggestions.
pub struct Reflector {
    skills_dir: PathBuf,
    /// Maximum number of suggestions to emit per trace (keeps output bounded).
    max_suggestions: usize,
}

impl Reflector {
    /// Create a reflector that writes auto-applied skills into `skills_dir`.
    pub fn new(skills_dir: impl Into<PathBuf>) -> Self {
        Self {
            skills_dir: skills_dir.into(),
            max_suggestions: 10,
        }
    }

    pub fn with_max_suggestions(mut self, max: usize) -> Self {
        self.max_suggestions = max;
        self
    }

    /// Read a trace JSONL file and replay it as an ordered event list.
    pub fn load_trace(path: &Path) -> Result<Vec<TraceRecord>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read trace: {path:?}"))?;
        let mut records = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: TraceRecord = serde_json::from_str(line)
                .with_context(|| format!("failed to parse trace line {}: {line}", i + 1))?;
            records.push(rec);
        }
        Ok(records)
    }

    /// Analyze a loaded trace and produce suggestions. Pure: does not write
    /// anything. The caller decides what to auto-apply vs. surface for approval.
    pub fn analyze(&self, records: &[TraceRecord]) -> Vec<Suggestion> {
        let digester = Digester;
        let mut suggestions = digester.analyze(records);
        suggestions.truncate(self.max_suggestions);
        suggestions
    }

    /// Apply a suggestion. Returns what actually happened.
    ///
    /// - Safe auto-apply kinds (append-only SKILL) are written directly.
    /// - Everything else returns [`SuggestionAction::NeedsApproval`] so the
    ///   caller can surface it through the approval channel.
    /// - Security-sensitive fields can never be applied here.
    pub fn apply(&self, suggestion: &Suggestion) -> Result<SuggestionAction> {
        // Hard guard: security fields are never writable by the reflector,
        // not even via the approval path.
        if suggestion.touches_security_field() {
            return Ok(SuggestionAction::Forbidden);
        }

        if !SAFE_AUTO_APPLY.contains(&suggestion.kind) {
            return Ok(SuggestionAction::NeedsApproval(suggestion.diff_preview()));
        }

        // Auto-apply path: only append-only SKILL generation.
        match suggestion.kind {
            SuggestionKind::AppendSkill => {
                self.write_skill(suggestion)?;
                Ok(SuggestionAction::Applied)
            }
            _ => Ok(SuggestionAction::NeedsApproval(suggestion.diff_preview())),
        }
    }

    /// Enrich each suggestion's `rationale` with an LLM-generated explanation.
    ///
    /// This is purely cosmetic — the *decision to suggest* was already made by
    /// the deterministic digester. If the LLM call fails or returns empty text,
    /// the original rationale is kept (best-effort). The LLM cannot change the
    /// suggestion kind, target, or safety classification.
    pub async fn enrich_with_llm(&self, suggestions: &mut [Suggestion], client: &OpenAIClient) {
        for sug in suggestions.iter_mut() {
            let prompt = format!(
                "You are reviewing an AI agent execution trace. A heuristic flagged this issue:\n\n                 Issue: {}\n                 Target: {}\n                 Heuristic rationale: {}\n\n                 In one or two sentences, explain concisely why this matters and what the suggested                  change should accomplish. Do not propose any config or credential changes.\n\n                 Explanation:",
                match sug.kind {
                    SuggestionKind::AppendSkill =>
                        "a skill should be added to prevent a recurring failure pattern",
                    SuggestionKind::MemoryThreshold =>
                        "a memory consolidation threshold may need tuning",
                    SuggestionKind::PermissionChange =>
                        "permission defaults may need review (manual only)",
                    SuggestionKind::CredentialChange => "credentials may need review (manual only)",
                    SuggestionKind::BehaviorLimit => "an iteration/token limit may need adjustment",
                },
                sug.target,
                sug.rationale,
            );
            let messages = vec![Message::user(&prompt)];
            match client.chat_completion(&messages, &[]).await {
                Ok((text, _)) => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        sug.rationale = trimmed.to_string();
                    }
                }
                Err(_) => { /* keep original rationale */ }
            }
        }
    }

    fn write_skill(&self, suggestion: &Suggestion) -> Result<()> {
        std::fs::create_dir_all(&self.skills_dir)?;
        let file_name = format!(
            "reflector-{}.md",
            suggestion
                .id
                .replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
        );
        let path = self.skills_dir.join(&file_name);
        let body = suggestion.skill_body.as_deref().unwrap_or("");
        let frontmatter = format!(
            "---\n\
             name: {name}\n\
             description: {desc}\n\
             version: \"1.0\"\n\
             generated_by: reflector\n\
             generated_at: {ts}\n\
             triggers: [{triggers}]\n\
             priority: 5\n\
             ---\n",
            name = suggestion.target,
            desc = suggestion
                .rationale
                .replace('"', "")
                .chars()
                .take(120)
                .collect::<String>(),
            ts = suggestion.detected_at,
            triggers = suggestion
                .skill_triggers
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        std::fs::write(&path, format!("{frontmatter}\n{body}\n"))
            .with_context(|| format!("failed to write skill: {path:?}"))?;
        Ok(())
    }
}

/// One line of a trace JSONL file: a timestamp plus the serialized event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRecord {
    pub ts: chrono::DateTime<chrono::Utc>,
    /// The event payload. Stored as a raw value because the reflector only
    /// inspects a handful of variants; keeping it generic avoids coupling to
    /// every `AgentEvent` shape.
    pub event: serde_json::Value,
}

impl TraceRecord {
    /// Best-effort extraction of the event "tag" (variant name). For
    /// externally-tagged enums (`{"VariantName": {...}}` or `"VariantName"`)
    /// this returns the key/string. Returns `None` for unrecognized shapes.
    pub fn event_tag(&self) -> Option<String> {
        match &self.event {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(map) => {
                if map.len() == 1 {
                    Some(map.keys().next()?.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// If this is a `ToolExecutionEnd` event, return its fields.
    pub fn as_tool_end(&self) -> Option<ToolEndSnapshot> {
        let obj = self.event.get("ToolExecutionEnd")?.as_object()?;
        Some(ToolEndSnapshot {
            tool_name: obj.get("tool_name")?.as_str()?.to_string(),
            is_error: obj.get("is_error")?.as_bool()?,
            result: obj
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    /// If this is an `Error` event, return its message.
    pub fn as_error(&self) -> Option<String> {
        self.event.get("Error")?.as_str().map(|s| s.to_string())
    }

    /// If this is a `TurnStart` event, return its index.
    pub fn as_turn_start(&self) -> Option<usize> {
        self.event
            .get("TurnStart")?
            .get("turn_index")?
            .as_u64()
            .map(|n| n as usize)
    }
}

/// Snapshot of a tool execution end event, for digester heuristics.
#[derive(Debug, Clone)]
pub struct ToolEndSnapshot {
    pub tool_name: String,
    pub is_error: bool,
    pub result: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tag: &str) -> TraceRecord {
        TraceRecord {
            ts: chrono::Utc::now(),
            event: serde_json::json!(tag),
        }
    }

    #[test]
    fn test_event_tag_string_variant() {
        assert_eq!(rec("AgentStart").event_tag().as_deref(), Some("AgentStart"));
    }

    #[test]
    fn test_event_tag_object_variant() {
        let r = TraceRecord {
            ts: chrono::Utc::now(),
            event: serde_json::json!({"TurnStart": {"turn_index": 3}}),
        };
        assert_eq!(r.event_tag().as_deref(), Some("TurnStart"));
        assert_eq!(r.as_turn_start(), Some(3));
    }

    #[test]
    fn test_as_tool_end() {
        let r = TraceRecord {
            ts: chrono::Utc::now(),
            event: serde_json::json!({
                "ToolExecutionEnd": {
                    "tool_call_id": "c1",
                    "tool_name": "bash",
                    "result": "Error: command not found",
                    "is_error": true
                }
            }),
        };
        let snap = r.as_tool_end().unwrap();
        assert_eq!(snap.tool_name, "bash");
        assert!(snap.is_error);
        assert!(snap.result.starts_with("Error"));
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;
    use crate::reflector::suggestion::{Suggestion, SuggestionKind};
    use tempfile::TempDir;

    fn s(kind: SuggestionKind, target: &str) -> Suggestion {
        Suggestion {
            id: format!("g-{}", slug(target)),
            kind,
            target: target.into(),
            rationale: "test".into(),
            detected_at: chrono::Utc::now(),
            skill_triggers: vec!["t".into()],
            skill_body: Some("# body\n".into()),
        }
    }

    #[test]
    fn append_skill_is_auto_applied_and_writes_file() {
        let dir = TempDir::new().unwrap();
        let r = Reflector::new(dir.path().to_str().unwrap());
        let sug = s(SuggestionKind::AppendSkill, "bash-guide");
        let action = r.apply(&sug).unwrap();
        assert!(matches!(action, SuggestionAction::Applied));
        // file written with reflector marker
        // file name is derived from the suggestion id ("g-bash-guide").
        let written =
            std::fs::read_to_string(dir.path().join("reflector-g-bash-guide.md")).unwrap();
        assert!(written.contains("generated_by: reflector"));
        assert!(written.contains("name: bash-guide"));
    }

    #[test]
    fn memory_threshold_needs_approval() {
        let dir = TempDir::new().unwrap();
        let r = Reflector::new(dir.path().to_str().unwrap());
        let action = r
            .apply(&s(SuggestionKind::MemoryThreshold, "memory.consolidation"))
            .unwrap();
        assert!(matches!(action, SuggestionAction::NeedsApproval(_)));
    }

    #[test]
    fn permission_change_is_forbidden_even_though_manual() {
        let dir = TempDir::new().unwrap();
        let r = Reflector::new(dir.path().to_str().unwrap());
        let action = r
            .apply(&s(SuggestionKind::PermissionChange, "permissions.mode"))
            .unwrap();
        assert!(matches!(action, SuggestionAction::Forbidden));
        // nothing written
        assert!(std::fs::read_dir(dir.path()).unwrap().count() == 0);
    }

    #[test]
    fn credential_change_is_forbidden() {
        let dir = TempDir::new().unwrap();
        let r = Reflector::new(dir.path().to_str().unwrap());
        let action = r
            .apply(&s(SuggestionKind::CredentialChange, "api_key"))
            .unwrap();
        assert!(matches!(action, SuggestionAction::Forbidden));
    }

    #[test]
    fn behavior_limit_needs_approval() {
        let dir = TempDir::new().unwrap();
        let r = Reflector::new(dir.path().to_str().unwrap());
        let action = r
            .apply(&s(SuggestionKind::BehaviorLimit, "max_iterations"))
            .unwrap();
        assert!(matches!(action, SuggestionAction::NeedsApproval(_)));
    }

    /// End-to-end: synthesize a trace with 3 consecutive bash errors, load it,
    /// analyze, and verify the emitted suggestion auto-applies as a skill.
    #[test]
    fn end_to_end_trace_to_skill() {
        let dir = TempDir::new().unwrap();
        let trace_path = dir.path().join("t.jsonl");
        let lines = vec![
            r#"{"ts":"2026-06-18T10:00:00Z","event":"TurnStart","turn_index":0}"#,
            r#"{"ts":"2026-06-18T10:00:01Z","event":{"ToolExecutionEnd":{"tool_call_id":"c1","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
            r#"{"ts":"2026-06-18T10:00:02Z","event":{"ToolExecutionEnd":{"tool_call_id":"c2","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
            r#"{"ts":"2026-06-18T10:00:03Z","event":{"ToolExecutionEnd":{"tool_call_id":"c3","tool_name":"bash","result":"Error: not found","is_error":true}}}"#,
        ];
        std::fs::write(&trace_path, lines.join("\n") + "\n").unwrap();

        let skills_dir = dir.path().join("skills");
        let reflector = Reflector::new(&skills_dir);
        let records = Reflector::load_trace(&trace_path).unwrap();
        let suggestions = reflector.analyze(&records);

        assert!(
            suggestions
                .iter()
                .any(|s| s.kind == SuggestionKind::AppendSkill)
        );
        // Apply all; the skill one must be Applied, nothing forbidden.
        for sug in &suggestions {
            let action = reflector.apply(sug).unwrap();
            assert!(!matches!(action, SuggestionAction::Forbidden));
        }
        // The skill file exists.
        assert!(std::fs::read_dir(&skills_dir).unwrap().count() >= 1);
    }

    fn slug(s: &str) -> String {
        s.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
            .to_lowercase()
    }
}
