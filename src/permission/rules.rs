use super::{ApprovalLevel, PermissionRule};

pub fn default_rules() -> Vec<PermissionRule> {
    vec![
        PermissionRule {
            tool_pattern: "read_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "write_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        PermissionRule {
            tool_pattern: "glob".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "grep".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_status".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_diff".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_log".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_show".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_commit".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        PermissionRule {
            tool_pattern: "run_command".to_string(),
            action_pattern: Some("rm ".to_string()),
            level: ApprovalLevel::Deny,
        },
        PermissionRule {
            tool_pattern: "run_command".to_string(),
            action_pattern: Some("sudo ".to_string()),
            level: ApprovalLevel::Deny,
        },
        PermissionRule {
            tool_pattern: "run_command".to_string(),
            action_pattern: Some("mkfs".to_string()),
            level: ApprovalLevel::Deny,
        },
        PermissionRule {
            tool_pattern: "run_command".to_string(),
            action_pattern: Some("dd ".to_string()),
            level: ApprovalLevel::Deny,
        },
        PermissionRule {
            tool_pattern: "run_command".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
        PermissionRule {
            tool_pattern: "*_memory_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
    ]
}

pub fn permissive_rules() -> Vec<PermissionRule> {
    vec![PermissionRule {
        tool_pattern: "*".to_string(),
        action_pattern: None,
        level: ApprovalLevel::Allow,
    }]
}

pub fn strict_rules() -> Vec<PermissionRule> {
    vec![
        PermissionRule {
            tool_pattern: "read_file".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "glob".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "grep".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "git_*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        },
        PermissionRule {
            tool_pattern: "*".to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        },
    ]
}
