//! Built-in permission rules with danger-level annotations.
//!
//! These provide the default security posture. Users can override
//! individual rules via config.toml [permissions.rules].

use super::types::{ApprovalLevel, DangerLevel, ToolPermissionPattern};

/// Default rules with danger levels. Returns `(pattern, danger_level, approval_level)`.
pub fn default_rules_with_danger() -> Vec<(ToolPermissionPattern, DangerLevel, ApprovalLevel)> {
    vec![
        // ── Read-only tools (safe, no side effects) ──────────────────
        (
            ToolPermissionPattern::simple("read_file"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("glob"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("grep"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // ── Git read-only (safe) ─────────────────────────────────────
        (
            ToolPermissionPattern::simple("git_status"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("git_diff"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("git_log"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("git_show"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // ── Git write (mutable) ──────────────────────────────────────
        (
            ToolPermissionPattern::simple("git_commit"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Ask,
        ),
        // ── File write (mutable) ─────────────────────────────────────
        (
            ToolPermissionPattern::simple("write_file"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Ask,
        ),
        (
            ToolPermissionPattern::simple("edit"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Ask,
        ),
        // ── Network ──────────────────────────────────────────────────
        (
            ToolPermissionPattern::simple("webfetch"),
            DangerLevel::Network,
            ApprovalLevel::Ask,
        ),
        // ── Shell commands ───────────────────────────────────────────
        // Destructive patterns blocked first (highest priority)
        (
            ToolPermissionPattern::simple("bash")
                .with_commands(vec![
                    "rm ".into(), "rmdir ".into(), "del ".into(),
                    "mkfs".into(), "dd ".into(), "fdisk".into(), "format ".into(),
                    "sudo ".into(), "su ".into(),
                    "shutdown".into(), "reboot".into(), "halt".into(),
                    "chmod 777".into(), "chmod -R 777".into(),
                    "> /dev/sda".into(), "> /dev/nvme".into(),
                ]),
            DangerLevel::Destructive,
            ApprovalLevel::Deny,
        ),
        // Catch-all shell commands: ask
        (
            ToolPermissionPattern::simple("bash"),
            DangerLevel::System,
            ApprovalLevel::Ask,
        ),
        // ── Memory tools (safe) ──────────────────────────────────────
        (
            ToolPermissionPattern::simple("*_memory_*"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("core_memory_*"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("recall_memory_*"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("archival_memory_*"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // ── Todo / Task / Skill (safe metadata ops) ──────────────────
        (
            ToolPermissionPattern::simple("todo_write"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("todo_read"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("task_create"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("task_update"),
            DangerLevel::ReadWrite,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("task_list"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("skill_*"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
    ]
}

/// Legacy function — returns old-style `PermissionRule` list for backward compat.
/// Prefer `default_rules_with_danger()` for new code.
pub fn default_rules() -> Vec<super::PermissionRule> {
    vec![
        super::PermissionRule {
            tool_pattern: "read_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "write_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        super::PermissionRule {
            tool_pattern: "edit".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        super::PermissionRule {
            tool_pattern: "glob".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "grep".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_status".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_diff".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_log".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_show".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_commit".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        super::PermissionRule {
            tool_pattern: "webfetch".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        super::PermissionRule {
            tool_pattern: "bash".to_string(),
            action_pattern: Some("rm ".to_string()),
            level: ApprovalLevel::Deny,
        },
        super::PermissionRule {
            tool_pattern: "bash".to_string(),
            action_pattern: Some("sudo ".to_string()),
            level: ApprovalLevel::Deny,
        },
        super::PermissionRule {
            tool_pattern: "bash".to_string(),
            action_pattern: Some("mkfs".to_string()),
            level: ApprovalLevel::Deny,
        },
        super::PermissionRule {
            tool_pattern: "bash".to_string(),
            action_pattern: Some("dd ".to_string()),
            level: ApprovalLevel::Deny,
        },
        super::PermissionRule {
            tool_pattern: "bash".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        super::PermissionRule {
            tool_pattern: "*_memory_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "todo_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "task_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "skill_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
    ]
}

/// Permissive: everything allowed.
pub fn permissive_rules() -> Vec<super::PermissionRule> {
    vec![super::PermissionRule {
        tool_pattern: "*".to_string(),
        action_pattern: None,
        level: ApprovalLevel::Allow,
    }]
}

/// Strict: only read-only allowed, rest ask.
pub fn strict_rules() -> Vec<super::PermissionRule> {
    vec![
        super::PermissionRule {
            tool_pattern: "read_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "glob".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "grep".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "git_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules_have_expected_coverage() {
        let rules = default_rules_with_danger();
        // Should have at least 10 built-in rules
        assert!(rules.len() >= 10);

        // read_file should be ReadOnly + Allow
        let read_file = rules.iter().find(|(p, _, _)| p.tool_pattern == "read_file").unwrap();
        assert_eq!(read_file.1, DangerLevel::ReadOnly);
        assert_eq!(read_file.2, ApprovalLevel::Allow);

        // git_commit should be ReadWrite + Ask
        let git_commit = rules.iter().find(|(p, _, _)| p.tool_pattern == "git_commit").unwrap();
        assert_eq!(git_commit.1, DangerLevel::ReadWrite);
        assert_eq!(git_commit.2, ApprovalLevel::Ask);
    }

    #[test]
    fn test_destructive_commands_denied() {
        let rules = default_rules_with_danger();
        let destructive = rules
            .iter()
            .find(|(p, d, _)| p.tool_pattern == "bash" && *d == DangerLevel::Destructive)
            .unwrap();
        assert_eq!(destructive.2, ApprovalLevel::Deny);
    }
}
