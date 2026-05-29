pub mod rules;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalLevel {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool_pattern: String,
    pub action_pattern: Option<String>,
    pub level: ApprovalLevel,
}

impl PermissionRule {
    pub fn allow(tool_pattern: &str) -> Self {
        Self {
            tool_pattern: tool_pattern.to_string(),
            action_pattern: None,
            level: ApprovalLevel::Allow,
        }
    }

    pub fn deny(tool_pattern: &str) -> Self {
        Self {
            tool_pattern: tool_pattern.to_string(),
            action_pattern: None,
            level: ApprovalLevel::Deny,
        }
    }

    pub fn ask(tool_pattern: &str) -> Self {
        Self {
            tool_pattern: tool_pattern.to_string(),
            action_pattern: None,
            level: ApprovalLevel::Ask,
        }
    }

    pub fn with_action(mut self, pattern: &str) -> Self {
        self.action_pattern = Some(pattern.to_string());
        self
    }

    pub fn matches(&self, tool_name: &str, tool_input: &str) -> bool {
        if !glob_match(&self.tool_pattern, tool_name) {
            return false;
        }
        if let Some(ref action_pattern) = self.action_pattern {
            return tool_input.contains(action_pattern.as_str());
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    rules: Vec<PermissionRule>,
    sandbox_paths: Vec<PathBuf>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            rules: rules::default_rules(),
            sandbox_paths: Vec::new(),
        }
    }
}

impl PermissionPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.rules = rules;
        self
    }

    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
    }

    pub fn with_sandbox_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.sandbox_paths = paths;
        self
    }

    pub fn add_sandbox_path(&mut self, path: PathBuf) {
        self.sandbox_paths.push(path);
    }

    pub fn check(&self, tool_name: &str, tool_input: &str) -> PermissionDecision {
        for rule in &self.rules {
            if rule.matches(tool_name, tool_input) {
                return match rule.level {
                    ApprovalLevel::Allow => PermissionDecision::Allow,
                    ApprovalLevel::Ask => PermissionDecision::Ask(format!(
                        "Tool '{}' matched rule requiring approval",
                        tool_name
                    )),
                    ApprovalLevel::Deny => PermissionDecision::Deny(format!(
                        "Tool '{}' is denied by policy",
                        tool_name
                    )),
                };
            }
        }

        PermissionDecision::Allow
    }

    pub fn check_path(&self, path: &str) -> Result<(), String> {
        if self.sandbox_paths.is_empty() {
            return Ok(());
        }

        let path = Path::new(path);
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        for sandbox in &self.sandbox_paths {
            if canonical.starts_with(sandbox) {
                return Ok(());
            }
        }

        Err(format!(
            "Path '{}' is outside sandbox boundaries",
            path.display()
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask(String),
    Deny(String),
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == text;
    }

    let regex_pattern = pattern
        .replace('.', "\\.")
        .replace('*', ".*")
        .replace('?', ".");

    regex::Regex::new(&format!("^{}$", regex_pattern))
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let rule = PermissionRule::allow("read_file");
        assert!(rule.matches("read_file", "{}"));
        assert!(!rule.matches("write_file", "{}"));
    }

    #[test]
    fn test_glob_match() {
        let rule = PermissionRule::deny("*_memory");
        assert!(rule.matches("core_memory", "{}"));
        assert!(rule.matches("archival_memory", "{}"));
        assert!(!rule.matches("read_file", "{}"));
    }

    #[test]
    fn test_action_pattern() {
        let rule = PermissionRule::ask("run_command").with_action("rm ");
        assert!(rule.matches("run_command", r#"{"command": "rm -rf /"}"#));
        assert!(!rule.matches("run_command", r#"{"command": "ls"}"#));
    }

    #[test]
    fn test_policy_first_match_wins() {
        let policy = PermissionPolicy::new().with_rules(vec![
            PermissionRule::deny("run_command").with_action("rm "),
            PermissionRule::allow("run_command"),
        ]);

        assert_eq!(
            policy.check("run_command", r#"{"command": "rm -rf /"}"#),
            PermissionDecision::Deny("Tool 'run_command' is denied by policy".to_string())
        );
        assert_eq!(
            policy.check("run_command", r#"{"command": "ls"}"#),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_sandbox_path() {
        let policy =
            PermissionPolicy::new().with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);

        assert!(policy.check_path("/etc/passwd").is_err());
        assert!(policy.check_path("/home/user/file.txt").is_err());
    }

    #[test]
    fn test_default_policy_allows_common_tools() {
        let policy = PermissionPolicy::new();
        assert_eq!(policy.check("read_file", "{}"), PermissionDecision::Allow);
        assert_eq!(policy.check("glob", "{}"), PermissionDecision::Allow);
        assert_eq!(policy.check("grep", "{}"), PermissionDecision::Allow);
    }
}
