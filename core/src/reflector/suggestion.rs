//! Suggestion model + safety allow-lists.

use serde::{Deserialize, Serialize};

/// What kind of change a suggestion proposes. Drives the safety rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuggestionKind {
    /// Append a new, non-destructive SKILL.md (the only auto-apply kind).
    AppendSkill,
    /// Adjust a memory consolidation threshold.
    MemoryThreshold,
    /// Change a permission mode or rule (FORBIDDEN — never writable).
    PermissionChange,
    /// Change an api_key / base_url / model_id (FORBIDDEN — never writable).
    CredentialChange,
    /// Change iteration / token limits.
    BehaviorLimit,
}

/// Kinds that may be auto-applied without human approval. Everything else
/// requires approval; security fields are forbidden entirely.
pub const SAFE_AUTO_APPLY: &[SuggestionKind] = &[SuggestionKind::AppendSkill];

/// Config fields the reflector must never write, even with approval.
/// These are the privilege/credential escape hatches.
pub const SECURITY_FIELDS: &[&str] = &[
    "api_key",
    "base_url",
    "model_id",
    "permissions",
    "mode",
    "blacklist",
];

/// A single proposed change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// Stable id (slug) for dedup / approval tracking.
    pub id: String,
    pub kind: SuggestionKind,
    /// What to change: a config key path, a skill name, etc.
    pub target: String,
    /// Why this was suggested, grounded in the trace.
    pub rationale: String,
    /// ISO-8601 timestamp when the digester detected it.
    pub detected_at: chrono::DateTime<chrono::Utc>,
    /// Triggers to attach to a generated skill (AppendSkill only).
    #[serde(default)]
    pub skill_triggers: Vec<String>,
    /// Body of a generated skill (AppendSkill only).
    #[serde(default)]
    pub skill_body: Option<String>,
}

impl Suggestion {
    /// True if this suggestion would touch a security-sensitive field.
    pub fn touches_security_field(&self) -> bool {
        matches!(
            self.kind,
            SuggestionKind::PermissionChange | SuggestionKind::CredentialChange
        )
    }

    /// A human-readable preview of the proposed change, for the approval UI.
    pub fn diff_preview(&self) -> String {
        match self.kind {
            SuggestionKind::AppendSkill => {
                format!("[skill] + {} (triggers: {:?})\n{}", self.target, self.skill_triggers, self.skill_body.as_deref().unwrap_or(""))
            }
            SuggestionKind::MemoryThreshold => {
                format!("[config] ~ memory.consolidation → {}\n# requires approval", self.target)
            }
            SuggestionKind::BehaviorLimit => {
                format!("[config] ~ {}\n# requires approval", self.target)
            }
            SuggestionKind::PermissionChange | SuggestionKind::CredentialChange => {
                format!(
                    "[FORBIDDEN] {} — security field; reflector cannot write this. Manual review only:\n{}",
                    self.target, self.rationale
                )
            }
        }
    }
}

/// What happened when [`crate::reflector::Reflector::apply`] ran.
#[derive(Debug, Clone)]
pub enum SuggestionAction {
    /// Written to disk (only happens for safe auto-apply kinds).
    Applied,
    /// Needs human approval; carries a diff preview.
    NeedsApproval(String),
    /// Refused: touches a security field and can never be applied here.
    Forbidden,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: SuggestionKind) -> Suggestion {
        Suggestion {
            id: "s1".into(),
            kind,
            target: "x".into(),
            rationale: "r".into(),
            detected_at: chrono::Utc::now(),
            skill_triggers: vec![],
            skill_body: None,
        }
    }

    #[test]
    fn test_safe_auto_apply_only_skill() {
        assert_eq!(SAFE_AUTO_APPLY, &[SuggestionKind::AppendSkill]);
    }

    #[test]
    fn test_security_fields_nonempty() {
        assert!(SECURITY_FIELDS.iter().any(|f| *f == "api_key"));
        assert!(SECURITY_FIELDS.iter().any(|f| *f == "permissions"));
    }

    #[test]
    fn test_touches_security_field() {
        assert!(sample(SuggestionKind::PermissionChange).touches_security_field());
        assert!(sample(SuggestionKind::CredentialChange).touches_security_field());
        assert!(!sample(SuggestionKind::AppendSkill).touches_security_field());
        assert!(!sample(SuggestionKind::MemoryThreshold).touches_security_field());
    }

    #[test]
    fn test_forbidden_preview_marked() {
        let s = sample(SuggestionKind::CredentialChange);
        assert!(s.diff_preview().contains("FORBIDDEN"));
    }
}
