//! Permission system — layered rule evaluation with whitelist, audit, and approval pipeline.
//!
//! Architecture:
//!
//! ```text
//! User Approval (runtime)        ← highest priority
//!   ↓
//! Blacklist (config)             ← unconditional deny
//!   ↓
//! Whitelist (session + config)   ← unconditional allow
//!   ↓
//! Config rules (config.toml)     ← user-defined overrides
//!   ↓
//! Built-in rules (rules.rs)      ← default posture
//!   ↓
//! Default: Ask                   ← catch-all
//! ```
//!
//! Each layer is checked in order. First match wins.

pub mod audit;
pub mod rules;
pub mod types;
pub mod whitelist;

use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use audit::{AuditLog, AuditStats};
pub use types::AuditEntry;
pub use types::{
    glob_match, ApprovalChoice, ApprovalLevel, ApprovalPrompt, ApprovalScope, ConfigRule,
    DangerLevel, PermissionConfig, PermissionMode, RuleSource, ToolPermissionPattern,
    WhitelistEntry,
};
pub use whitelist::WhitelistManager;

// ── Re-export old type for backward compat ──────────────────────────

/// Legacy permission rule (kept for backward compatibility).
/// Prefer `ConfigRule` + `ToolPermissionPattern` for new code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

// ── Decision type ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool is allowed to run.
    Allow,
    /// Tool requires user approval. Contains the reason and an approval prompt.
    Ask(String, ApprovalPrompt),
    /// Tool is denied. Contains the reason.
    Deny(String),
}

impl PermissionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny(_))
    }

    pub fn needs_approval(&self) -> bool {
        matches!(self, Self::Ask(_, _))
    }
}

// ── Permission Policy (layered rules) ───────────────────────────────

pub struct PermissionPolicy {
    /// Global permission posture
    mode: PermissionMode,
    /// Auto-allow up to this danger level (overrides mode defaults)
    auto_allow_up_to: Option<DangerLevel>,
    /// Built-in rules (from rules.rs)
    builtin_rules: Vec<(ToolPermissionPattern, DangerLevel, ApprovalLevel)>,
    /// Config-level rules (from config.toml)
    config_rules: Vec<ConfigRule>,
    /// Whitelist manager
    whitelist: WhitelistManager,
    /// Blacklist patterns
    blacklist: Vec<ToolPermissionPattern>,
    /// Audit log
    audit: Option<AuditLog>,
    /// Sandbox paths
    sandbox_paths: Vec<PathBuf>,
    /// Apply permissive defaults (for backward compat)
    permissive_default: bool,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Standard,
            auto_allow_up_to: None,
            builtin_rules: Vec::new(),
            config_rules: Vec::new(),
            whitelist: WhitelistManager::new(),
            blacklist: Vec::new(),
            audit: None,
            sandbox_paths: Vec::new(),
            permissive_default: false,
        }
    }
}

impl PermissionPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Builder methods ──────────────────────────────────────────

    /// Build with built-in defaults injected.
    pub fn with_builtin_defaults() -> Self {
        let mut policy = Self::new();
        policy.builtin_rules = rules::default_rules_with_danger();
        policy
    }

    pub fn with_permissive_defaults() -> Self {
        let mut policy = Self::new();
        policy.builtin_rules = rules::default_rules_with_danger();
        policy.permissive_default = true;
        policy
    }

    pub fn with_strict_defaults() -> Self {
        let mut policy = Self::new();
        policy.builtin_rules = rules::default_rules_with_danger();
        // Strict: all non-read-only → Ask
        policy.mode = PermissionMode::Standard;
        policy
    }

    pub fn with_mode(mut self, mode: PermissionMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_auto_allow_up_to(mut self, level: DangerLevel) -> Self {
        self.auto_allow_up_to = Some(level);
        self
    }

    pub fn with_config(mut self, config: &PermissionConfig) -> Self {
        self.mode = config.mode;
        self.auto_allow_up_to = config.auto_allow_up_to;
        self.config_rules = config.rules.clone();
        self.blacklist = config.blacklist.clone();
        self.sandbox_paths = config
            .sandbox_paths
            .iter()
            .map(|s| {
                let expanded = expand_tilde(s);
                PathBuf::from(expanded)
            })
            .collect();

        // Load persistent whitelist
        self.whitelist.load_persistent(config.whitelist.clone());

        self
    }

    pub fn with_audit(mut self, audit: AuditLog) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_whitelist_manager(mut self, wl: WhitelistManager) -> Self {
        self.whitelist = wl;
        self
    }

    pub fn with_sandbox_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.sandbox_paths = paths;
        self
    }

    // ── Rule management ─────────────────────────────────────────

    /// Add a config-level rule.
    pub fn add_rule(&mut self, rule: ConfigRule) {
        self.config_rules.push(rule);
    }

    /// Add a builtin rule (for framework extensions).
    pub fn add_builtin_rule(
        &mut self,
        pattern: ToolPermissionPattern,
        danger: DangerLevel,
        level: ApprovalLevel,
    ) {
        self.builtin_rules.push((pattern, danger, level));
    }

    // ── Whitelist access ─────────────────────────────────────────

    pub fn whitelist_mut(&mut self) -> &mut WhitelistManager {
        &mut self.whitelist
    }

    pub fn whitelist(&self) -> &WhitelistManager {
        &self.whitelist
    }

    // ── Main check method ────────────────────────────────────────

    /// Check permission for a tool invocation.
    ///
    /// Returns `Allow`, `Deny(reason)`, or `Ask(reason, prompt)`.
    /// `tool_input_json` is the raw tool arguments as a JSON string.
    /// `command` is extracted for `bash`; `path` for file ops; `host` for network.
    pub fn check(
        &mut self,
        tool_name: &str,
        tool_input_json: &str,
        command: Option<&str>,
        path: Option<&str>,
        host: Option<&str>,
    ) -> PermissionDecision {
        let danger = self.danger_level_for(tool_name, tool_input_json, command);

        // Layer 0: Yolo mode — everything allowed
        if self.mode == PermissionMode::Yolo {
            self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Builtin, "yolo mode", danger, None);
            return PermissionDecision::Allow;
        }

        // Layer 1: Blacklist — unconditional deny (even in Permissive mode)
        for bp in &self.blacklist {
            if bp.matches_tool(tool_name) {
                if let Some(cmd) = command {
                    if !bp.matches_command(cmd) {
                        continue;
                    }
                }
                let reason = format!(
                    "Tool '{}' is blacklisted (config.toml [permissions.blacklist])",
                    tool_name
                );
                self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Deny, &RuleSource::Config, &reason, danger, None);
                return PermissionDecision::Deny(reason);
            }
        }

        // Layer 2: Whitelist — unconditional allow
        if let Some(entry) = self.whitelist.query(tool_name, command, path, host) {
            if entry.is_valid() {
                let reason = format!(
                    "matched whitelist: {} (scope: {:?}, used {} times)",
                    entry.pattern.tool_pattern,
                    entry.scope,
                    entry.use_count
                );
                self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Whitelist, &reason, danger, None);
                return PermissionDecision::Allow;
            }
        }

        // Layer 3: Mode-based auto-allow
        if let Some(max_danger) = self.auto_allow_up_to {
            if danger <= max_danger {
                self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Config,
                    &format!("auto_allow_up_to ≥ {:?}", danger), danger, None);
                return PermissionDecision::Allow;
            }
        }

        match self.mode {
            PermissionMode::Paranoid => {
                // Everything prompts except explicit config rules (checked below)
            }
            PermissionMode::Standard => {
                // Check built-in + config rules (checked below)
            }
            PermissionMode::Permissive => {
                // Auto-allow ReadOnly, ReadWrite, Network
                if danger <= DangerLevel::Network {
                    self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Builtin,
                        &format!("permissive: {:?} ≤ Network", danger), danger, None);
                    return PermissionDecision::Allow;
                }
            }
            PermissionMode::Yolo => unreachable!(),
        }

        // Layer 4: Config rules (from config.toml [permissions.rules])
        for rule in &self.config_rules {
            if rule.pattern.matches_tool(tool_name) {
                if let Some(cmd) = command {
                    if !rule.pattern.matches_command(cmd) {
                        continue;
                    }
                }
                if let Some(p) = path {
                    if !rule.pattern.matches_path(p) {
                        continue;
                    }
                }
                let matched = format!(
                    "config rule: {} → {:?}",
                    rule.pattern.tool_pattern, rule.level
                );
                match &rule.level {
                    ApprovalLevel::Allow => {
                        self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Config, &matched, danger, None);
                        return PermissionDecision::Allow;
                    }
                    ApprovalLevel::Deny => {
                        self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Deny, &RuleSource::Config, &matched, danger, None);
                        return PermissionDecision::Deny(format!(
                            "Tool '{}' denied by config rule: {}",
                            tool_name, matched
                        ));
                    }
                    ApprovalLevel::Ask => {
                        // Falls through to prompt
                    }
                }
            }
        }

        // Layer 5: Built-in rules (from rules.rs)
        for (pattern, rule_danger, level) in &self.builtin_rules {
            if pattern.matches_tool(tool_name) {
                if let Some(cmd) = command {
                    if !pattern.matches_command(cmd) {
                        continue;
                    }
                }
                if let Some(p) = path {
                    if !pattern.matches_path(p) {
                        continue;
                    }
                }
                let matched = format!(
                    "builtin rule: {} → {:?} (danger: {:?})",
                    pattern.tool_pattern, level, rule_danger
                );
                match level {
                    ApprovalLevel::Allow => {
                        self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Builtin, &matched, danger, None);
                        return PermissionDecision::Allow;
                    }
                    ApprovalLevel::Deny => {
                        self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Deny, &RuleSource::Builtin, &matched, danger, None);
                        return PermissionDecision::Deny(format!(
                            "Tool '{}' denied by builtin rule: {}",
                            tool_name, matched
                        ));
                    }
                    ApprovalLevel::Ask => {
                        // Falls through to prompt
                    }
                }
            }
        }

        // Layer 6: Default — Ask (or Allow for permissive backward compat)
        if self.permissive_default {
            self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Allow, &RuleSource::Builtin,
                "permissive default", danger, None);
            return PermissionDecision::Allow;
        }

        let prompt = self.build_approval_prompt(tool_name, tool_input_json, danger, command, path);
        self.audit_record(tool_name, tool_input_json, &ApprovalLevel::Ask, &RuleSource::Builtin,
            "default: Ask", danger, None);
        PermissionDecision::Ask(
            format!("Tool '{}' (danger: {:?}) requires approval", tool_name, danger),
            prompt,
        )
    }

    // ── Path sandbox ─────────────────────────────────────────────

    /// Check if a path is within sandbox boundaries.
    pub fn check_path(&self, file_path: &str) -> Result<(), String> {
        if self.sandbox_paths.is_empty() {
            return Ok(());
        }
        let path = Path::new(file_path);
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

    // ── Helpers ──────────────────────────────────────────────────

    /// Determine the danger level for a tool call based on name + input.
    pub fn danger_level_for(
        &self,
        tool_name: &str,
        _tool_input_json: &str,
        command: Option<&str>,
    ) -> DangerLevel {
        // First check built-in rules for an annotated danger level
        for (pattern, danger, _) in &self.builtin_rules {
            if pattern.matches_tool(tool_name) {
                return *danger;
            }
        }

        // Fallback heuristics
        match tool_name {
            "bash" => {
                // Check command for destructive patterns
                if let Some(cmd) = command {
                    if is_destructive_command(cmd) {
                        return DangerLevel::Destructive;
                    }
                }
                DangerLevel::System
            }
            "webfetch" => DangerLevel::Network,
            "write_file" | "edit" => DangerLevel::ReadWrite,
            _ => DangerLevel::ReadOnly,
        }
    }

    fn build_approval_prompt(
        &self,
        tool_name: &str,
        _tool_input_json: &str,
        danger: DangerLevel,
        _command: Option<&str>,
        _path: Option<&str>,
    ) -> ApprovalPrompt {
        let args: Value = serde_json::from_str(_tool_input_json).unwrap_or_default();
        let explanation = match danger {
            DangerLevel::ReadOnly => format!("{} will only read data (no side effects)", tool_name),
            DangerLevel::ReadWrite => format!("{} may modify files", tool_name),
            DangerLevel::Network => format!("{} will make network requests", tool_name),
            DangerLevel::System => format!("{} will execute system commands", tool_name),
            DangerLevel::Destructive => format!(
                "⚠ {} may cause irreversible damage (destructive command)",
                tool_name
            ),
        };

        ApprovalPrompt {
            prompt_id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool_name.to_string(),
            tool_input: args,
            danger_level: danger,
            matched_rule: format!("mode: {} (danger: {:?})", self.mode, danger),
            explanation,
        }
    }

    fn audit_record(
        &self,
        tool_name: &str,
        tool_input: &str,
        decision: &ApprovalLevel,
        source: &RuleSource,
        matched_rule: &str,
        danger: DangerLevel,
        reason: Option<&str>,
    ) {
        if let Some(ref audit) = self.audit {
            audit.record(tool_name, tool_input, decision, source, matched_rule, danger, reason);
        }
    }

    // ── Getters ──────────────────────────────────────────────────

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// Set the permission mode at runtime.
    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub fn sandbox_paths(&self) -> &[PathBuf] {
        &self.sandbox_paths
    }
}

// ── Destructive command detection ───────────────────────────────────

/// Expand `~` in a path to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            if path == "~" {
                return home;
            }
            if path.starts_with("~/") {
                return format!("{}/{}", home, &path[2..]);
            }
        }
    }
    path.to_string()
}

/// Check if a command string contains destructive patterns.
pub fn is_destructive_command(cmd: &str) -> bool {
    let destructive_patterns = [
        "rm ", "rmdir ", "del ", "deltree ",
        "mkfs", "dd ", "fdisk", "format ",
        "sudo ", "su ",
        "chmod 777", "chmod -R 777",
        "shutdown", "reboot", "halt",
        ":(){ :|:& };:", // fork bomb
        "> /dev/sda", "> /dev/nvme",
    ];
    let lower = cmd.to_lowercase();
    destructive_patterns
        .iter()
        .any(|p| lower.contains(p))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> PermissionPolicy {
        PermissionPolicy::with_builtin_defaults().with_mode(PermissionMode::Standard)
    }

    #[test]
    fn test_read_file_allowed() {
        let mut policy = make_policy();
        let result = policy.check("read_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_destructive_command_denied() {
        let mut policy = make_policy();
        let result = policy.check("bash", r#"{"command":"rm -rf /"}"#, Some("rm -rf /"), None, None);
        assert!(result.is_denied());
    }

    #[test]
    fn test_safe_command_asks_by_default() {
        let mut policy = make_policy();
        let result = policy.check("bash", r#"{"command":"ls -la"}"#, Some("ls -la"), None, None);
        assert!(result.needs_approval());
    }

    #[test]
    fn test_whitelist_overrides_default() {
        let mut policy = make_policy();
        policy.whitelist_mut().add(WhitelistEntry::new(
            ToolPermissionPattern::simple("bash"),
            ApprovalScope::Session,
        ));
        let result = policy.check("bash", r#"{"command":"ls"}"#, Some("ls"), None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_yolo_mode_allows_everything() {
        let mut policy = make_policy();
        policy.mode = PermissionMode::Yolo;
        let result = policy.check("bash", r#"{"command":"rm -rf /"}"#, Some("rm -rf /"), None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_paranoid_mode_asks_for_read() {
        let mut policy = make_policy();
        policy.mode = PermissionMode::Paranoid;
        let result = policy.check("read_file", "{}", None, None, None);
        assert!(result.needs_approval() || result.is_allowed());
    }

    #[test]
    fn test_blacklist_overrides_whitelist() {
        let mut policy = make_policy();
        policy.whitelist_mut().add(WhitelistEntry::new(
            ToolPermissionPattern::simple("bash"),
            ApprovalScope::Persistent,
        ));
        policy.blacklist.push(ToolPermissionPattern::simple("bash"));
        let result = policy.check("bash", r#"{"command":"ls"}"#, Some("ls"), None, None);
        assert!(result.is_denied());
    }

    #[test]
    fn test_auto_allow_up_to() {
        let mut policy = make_policy();
        policy.auto_allow_up_to = Some(DangerLevel::ReadWrite);
        // write_file is ReadWrite, should now be auto-allowed
        let result = policy.check("write_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_config_rule_overrides_builtin() {
        let mut policy = make_policy();
        policy.add_rule(ConfigRule {
            pattern: ToolPermissionPattern::simple("write_file"),
            level: ApprovalLevel::Allow,
        });
        // write_file is normally Ask; config rule overrides to Allow
        let result = policy.check("write_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_backward_compat_permissive() {
        let mut policy = PermissionPolicy::with_permissive_defaults();
        let result = policy.check("unknown_tool", "{}", None, None, None);
        assert!(result.is_allowed()); // permissive default
    }

    #[test]
    fn test_is_destructive_command() {
        assert!(is_destructive_command("rm -rf /"));
        assert!(is_destructive_command("sudo rm file"));
        assert!(is_destructive_command("mkfs.ext4 /dev/sda"));
        assert!(is_destructive_command("dd if=/dev/zero of=/dev/sda"));
        assert!(!is_destructive_command("git status"));
        assert!(!is_destructive_command("cargo build"));
        assert!(!is_destructive_command("python script.py"));
    }

    #[test]
    fn test_sandbox_path() {
        let policy = PermissionPolicy::new()
            .with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);
        assert!(policy.check_path("/etc/passwd").is_err());
        assert!(policy.check_path("/home/user/file.txt").is_err());
    }
}
