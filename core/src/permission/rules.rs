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
            ToolPermissionPattern::simple("grep"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("glob"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
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
        // ── Network (read-only fetch — safe to auto-approve) ─────────
        (
            ToolPermissionPattern::simple("webfetch"),
            DangerLevel::Network,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("tavily_search"),
            DangerLevel::Network,
            ApprovalLevel::Allow,
        ),
        // ── Shell commands ───────────────────────────────────────────
        // Read-only shell commands (safe to auto-approve)
        (
            ToolPermissionPattern::simple("bash").with_commands(vec![
                "ls ".to_string(), "ls".to_string(),
                "find ".to_string(), "find".to_string(),
                "wc ".to_string(), "wc".to_string(),
                "cat ".to_string(), "cat".to_string(),
                "grep ".to_string(), "grep".to_string(),
                "rg ".to_string(), "rg".to_string(),
                "ag ".to_string(), "ag".to_string(),
                "jq ".to_string(), "jq".to_string(),
                "head ".to_string(), "head".to_string(),
                "tail ".to_string(), "tail".to_string(),
                "awk ".to_string(),
                "sed -n ".to_string(), // -n is safe, -i is destructive
                "file ".to_string(), "file".to_string(),
                "tree ".to_string(), "tree".to_string(),
                "bat ".to_string(), "bat".to_string(),
                "stat ".to_string(), "stat".to_string(),
                "du ".to_string(), "du".to_string(),
                "df ".to_string(), "df".to_string(),
            ]),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // Destructive shell commands are denied programmatically in
        // `PermissionPolicy::check` via `is_destructive_command` (which is
        // robust to whitespace/`$IFS` evasion and covers `doas`, `pkexec`,
        // `chmod 0777`, …). See `core/src/permission/mod.rs`.
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
        // conversation_search / conversation_search_date don't match
        // the *_memory_* glob above — add explicit Allow rules.
        (
            ToolPermissionPattern::simple("conversation_search"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        (
            ToolPermissionPattern::simple("conversation_search_date"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // ── Human clarification (pre-work gate) ──────────────────────
        (
            ToolPermissionPattern::simple("ask_user"),
            DangerLevel::ReadOnly,
            ApprovalLevel::Allow,
        ),
        // ── Localhost preview (serves workspace files on loopback) ─
        (
            ToolPermissionPattern::simple("preview"),
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
            tool_pattern: "grep".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "glob".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "webfetch".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        super::PermissionRule {
            tool_pattern: "tavily_search".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
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
            tool_pattern: "grep".to_string(),
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
        let read_file = rules
            .iter()
            .find(|(p, _, _)| p.tool_pattern == "read_file")
            .unwrap();
        assert_eq!(read_file.1, DangerLevel::ReadOnly);
        assert_eq!(read_file.2, ApprovalLevel::Allow);

        // edit should be ReadWrite + Ask
        let edit_rule = rules
            .iter()
            .find(|(p, _, _)| p.tool_pattern == "edit")
            .unwrap();
        assert_eq!(edit_rule.1, DangerLevel::ReadWrite);
        assert_eq!(edit_rule.2, ApprovalLevel::Ask);
    }

    #[test]
    fn test_bash_defaults_to_ask() {
        // Two built-in bash rules now exist: a readonly-command whitelist
        // (ReadOnly/Allow) and a System→Ask catch-all for everything else.
        // Destructive commands are denied programmatically in
        // `PermissionPolicy::check` via `is_destructive_command`.
        let rules = default_rules_with_danger();
        // The catch-all has no command constraints (`commands` is None);
        // the whitelist rule carries `Some([...])`.
        let catch_all = rules
            .iter()
            .find(|(p, _, _)| p.tool_pattern == "bash" && p.commands.is_none())
            .unwrap();
        assert_eq!(catch_all.1, DangerLevel::System);
        assert_eq!(catch_all.2, ApprovalLevel::Ask);
    }
}
