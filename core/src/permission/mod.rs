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
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub type PendingApprovalMap = HashMap<String, tokio::sync::oneshot::Sender<types::ApprovalChoice>>;

/// Subagent waiters keyed by `parent_run_id:prompt_id`. Main Run approvals use
/// `ApprovalResolver`; this map exists because child tools execute outside the
/// main Run actor while still presenting approvals in the parent's UI stream.
pub fn pending_subagent_approvals() -> Arc<Mutex<PendingApprovalMap>> {
    static MAP: OnceLock<Arc<Mutex<PendingApprovalMap>>> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

pub use audit::{AuditLog, AuditStats};
pub use types::AuditEntry;
pub use types::{
    ApprovalChoice, ApprovalLevel, ApprovalPrompt, ApprovalScope, ConfigRule, DangerLevel,
    PermissionConfig, PermissionMode, RuleSource, ToolPermissionPattern, WhitelistEntry,
    glob_match,
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

    /// Update the policy in-place from a new config without destroying runtime state (like session whitelist).
    pub fn update_from_config(&mut self, config: &PermissionConfig) {
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
        self.whitelist.load_persistent(config.whitelist.clone());
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
    /// `command` is extracted for `shell`; `path` for file ops; `host` for network.
    pub fn check(
        &mut self,
        tool_name: &str,
        tool_input_json: &str,
        command: Option<&str>,
        path: Option<&str>,
        host: Option<&str>,
    ) -> PermissionDecision {
        let paths: Vec<&str> = path.into_iter().collect();
        self.check_scoped(
            tool_name,
            tool_input_json,
            command,
            &paths,
            host,
            None,
        )
    }

    /// Check a fully extracted invocation using the same cwd that execution
    /// will use. Every path is sandboxed and every rule dimension is matched.
    pub fn check_scoped(
        &mut self,
        tool_name: &str,
        tool_input_json: &str,
        command: Option<&str>,
        paths: &[&str],
        host: Option<&str>,
        working_dir: Option<&str>,
    ) -> PermissionDecision {
        let danger = self.danger_level_for(tool_name, tool_input_json, command);

        // Pre-layer: Sandbox — paths outside the sandbox are hard-denied.
        // This is a security boundary: it is enforced before everything else
        // (including Yolo mode and the whitelist) so a sandboxed agent can never
        // touch files outside its allowed roots, regardless of other approvals.
        for p in paths {
            if let Err(reason) = self.check_path_from(p, working_dir) {
                self.audit_record(
                    tool_name,
                    tool_input_json,
                    &ApprovalLevel::Deny,
                    &RuleSource::Config,
                    &reason,
                    danger,
                    None,
                );
                return PermissionDecision::Deny(reason);
            }
        }

        // Whether a built-in safety rule unconditionally denies this call
        // (currently: destructive shell commands). Such denies must not be
        // bypassed by `auto_allow_up_to` — they are evaluated below at Layer 5.
        let builtin_deny =
            crate::runtime::platform_shell::is_shell_tool(tool_name)
                && danger == DangerLevel::Destructive;

        // Layer 0: Yolo mode — everything allowed
        if self.mode == PermissionMode::Yolo {
            self.audit_record(
                tool_name,
                tool_input_json,
                &ApprovalLevel::Allow,
                &RuleSource::Builtin,
                "yolo mode",
                danger,
                None,
            );
            return PermissionDecision::Allow;
        }

        // Layer 1: Blacklist — unconditional deny (even in Permissive mode)
        for bp in &self.blacklist {
            if bp.matches_invocation(tool_name, command, paths, host, danger) {
                let reason = format!(
                    "Tool '{}' is blacklisted (config.toml [permissions.blacklist])",
                    tool_name
                );
                self.audit_record(
                    tool_name,
                    tool_input_json,
                    &ApprovalLevel::Deny,
                    &RuleSource::Config,
                    &reason,
                    danger,
                    None,
                );
                return PermissionDecision::Deny(reason);
            }
        }

        // Layer 2: Whitelist — unconditional allow
        if let Some(entry) = self
            .whitelist
            .query_scoped(tool_name, command, paths, host, danger)
        {
            if entry.is_valid() {
                let reason = format!(
                    "matched whitelist: {} (scope: {:?}, used {} times)",
                    entry.pattern.tool_pattern, entry.scope, entry.use_count
                );
                self.audit_record(
                    tool_name,
                    tool_input_json,
                    &ApprovalLevel::Allow,
                    &RuleSource::Whitelist,
                    &reason,
                    danger,
                    None,
                );
                return PermissionDecision::Allow;
            }
        }

        // Layer 3: Mode-based auto-allow
        if let Some(max_danger) = self.auto_allow_up_to {
            // Never short-circuit a built-in safety deny (e.g. destructive shell
            // commands) — let it fire at Layer 5 instead. Without this, setting
            // `auto_allow_up_to = Destructive` would bypass the destructive deny.
            if danger <= max_danger && !builtin_deny {
                self.audit_record(
                    tool_name,
                    tool_input_json,
                    &ApprovalLevel::Allow,
                    &RuleSource::Config,
                    &format!("auto_allow_up_to ≥ {:?}", danger),
                    danger,
                    None,
                );
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
            PermissionMode::Developer => {
                // Auto-allow ReadOnly tools/commands
                if danger == DangerLevel::ReadOnly {
                    self.audit_record(
                        tool_name,
                        tool_input_json,
                        &ApprovalLevel::Allow,
                        &RuleSource::Builtin,
                        "developer: ReadOnly auto-allow",
                        danger,
                        None,
                    );
                    return PermissionDecision::Allow;
                }
            }
            PermissionMode::Permissive => {
                // Auto-allow ReadOnly, ReadWrite, Network
                if danger <= DangerLevel::Network {
                    self.audit_record(
                        tool_name,
                        tool_input_json,
                        &ApprovalLevel::Allow,
                        &RuleSource::Builtin,
                        &format!("permissive: {:?} ≤ Network", danger),
                        danger,
                        None,
                    );
                    return PermissionDecision::Allow;
                }
            }
            PermissionMode::Yolo => unreachable!(),
        }

        // Layer 4: Config rules (from config.toml [permissions.rules])
        for rule in &self.config_rules {
            if rule
                .pattern
                .matches_invocation(tool_name, command, paths, host, danger)
            {
                let matched = format!(
                    "config rule: {} → {:?}",
                    rule.pattern.tool_pattern, rule.level
                );
                match &rule.level {
                    ApprovalLevel::Allow => {
                        self.audit_record(
                            tool_name,
                            tool_input_json,
                            &ApprovalLevel::Allow,
                            &RuleSource::Config,
                            &matched,
                            danger,
                            None,
                        );
                        return PermissionDecision::Allow;
                    }
                    ApprovalLevel::Deny => {
                        self.audit_record(
                            tool_name,
                            tool_input_json,
                            &ApprovalLevel::Deny,
                            &RuleSource::Config,
                            &matched,
                            danger,
                            None,
                        );
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
        // Built-in safety deny: destructive shell commands are blocked by default.
        // (Config rules at Layer 4 may still explicitly allow a specific command,
        // and the whitelist at Layer 2 may allow an explicitly-approved command.)
        if builtin_deny {
            let reason = "destructive command blocked by built-in safety rule".to_string();
            self.audit_record(
                tool_name,
                tool_input_json,
                &ApprovalLevel::Deny,
                &RuleSource::Builtin,
                &reason,
                danger,
                None,
            );
            return PermissionDecision::Deny(format!("Tool '{}' denied: {}", tool_name, reason));
        }

        for (pattern, rule_danger, level) in &self.builtin_rules {
            if pattern.matches_invocation(tool_name, command, paths, host, danger) {
                let matched = format!(
                    "builtin rule: {} → {:?} (danger: {:?})",
                    pattern.tool_pattern, level, rule_danger
                );
                match level {
                    ApprovalLevel::Allow => {
                        self.audit_record(
                            tool_name,
                            tool_input_json,
                            &ApprovalLevel::Allow,
                            &RuleSource::Builtin,
                            &matched,
                            danger,
                            None,
                        );
                        return PermissionDecision::Allow;
                    }
                    ApprovalLevel::Deny => {
                        self.audit_record(
                            tool_name,
                            tool_input_json,
                            &ApprovalLevel::Deny,
                            &RuleSource::Builtin,
                            &matched,
                            danger,
                            None,
                        );
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
            self.audit_record(
                tool_name,
                tool_input_json,
                &ApprovalLevel::Allow,
                &RuleSource::Builtin,
                "permissive default",
                danger,
                None,
            );
            return PermissionDecision::Allow;
        }

        let prompt = self.build_approval_prompt(
            tool_name,
            tool_input_json,
            danger,
            command,
            paths.first().copied(),
        );
        self.audit_record(
            tool_name,
            tool_input_json,
            &ApprovalLevel::Ask,
            &RuleSource::Builtin,
            "default: Ask",
            danger,
            None,
        );
        PermissionDecision::Ask(
            format!(
                "Tool '{}' (danger: {:?}) requires approval",
                tool_name, danger
            ),
            prompt,
        )
    }

    // ── Path sandbox ─────────────────────────────────────────────

    /// Check if a path is within sandbox boundaries.
    ///
    /// Handles paths that do not exist yet (e.g. `write_file` creating a new
    /// file) by canonicalizing the existing parent directory and re-attaching
    /// the file name. Both the target and the configured sandbox roots are
    /// canonicalized so symlink/relative-path comparisons are correct.
    pub fn check_path(&self, file_path: &str) -> Result<(), String> {
        self.check_path_from(file_path, None)
    }

    pub fn check_path_from(
        &self,
        file_path: &str,
        working_dir: Option<&str>,
    ) -> Result<(), String> {
        if self.sandbox_paths.is_empty() {
            return Ok(());
        }
        let target = canonicalize_target(file_path, working_dir);
        for sandbox in &self.sandbox_paths {
            let sandbox_canon = sandbox.canonicalize().unwrap_or_else(|_| sandbox.clone());
            if target.starts_with(&sandbox_canon) {
                return Ok(());
            }
        }
        Err(format!(
            "Path '{}' is outside sandbox boundaries (allowed roots: {:?})",
            file_path, self.sandbox_paths
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
        // shell danger depends on the actual command: destructive commands
        // (rm, mkfs, sudo, …) are `Destructive`, everything else is `System`.
        // This is evaluated before the rule loop so the destructive deny in
        // `check` fires precisely, instead of treating all shell as destructive.
        if crate::runtime::platform_shell::is_shell_tool(tool_name) {
            if command.map_or(false, is_destructive_command) {
                return DangerLevel::Destructive;
            }
            if command.map_or(false, |cmd| is_readonly_command(cmd, &self.sandbox_paths)) {
                return DangerLevel::ReadOnly;
            }
            return DangerLevel::System;
        }

        // Session REPL executes arbitrary Python/JS on the host.
        if tool_name == "repl" {
            return DangerLevel::System;
        }

        // Dynamic skill script tools execute arbitrary programs. Check this
        // before the broad read-only `skill_*` metadata-tool rule; API-facing
        // names replace dots with underscores.
        if crate::tools::script::is_skill_script_tool_name(tool_name) {
            return DangerLevel::System;
        }

        // Other tools: use the danger annotated on built-in rules.
        for (pattern, danger, _) in &self.builtin_rules {
            if pattern.matches_tool(tool_name) {
                return *danger;
            }
        }

        // Fallback heuristics
        match tool_name {
            "webfetch" | "tavily_search" => DangerLevel::Network,
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
            audit.record(
                tool_name,
                tool_input,
                decision,
                source,
                matched_rule,
                danger,
                reason,
            );
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


include!("command_analysis.inc.rs");

#[cfg(test)]
include!("tests.inc.rs");
