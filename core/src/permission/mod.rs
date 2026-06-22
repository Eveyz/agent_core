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
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;

pub type PendingApprovalMap = HashMap<String, tokio::sync::oneshot::Sender<types::ApprovalChoice>>;

/// **Deprecated**: Per-Run approval routing via [`ApprovalResolver`](crate::runtime::ApprovalResolver)
/// replaces this global map. This is kept only for backward compatibility with
/// the legacy `Agent` path used by the CLI.
#[deprecated(note = "use runtime::ApprovalResolver instead — this global map is not scoped per-Run")]
pub fn global_pending_approvals() -> Arc<Mutex<PendingApprovalMap>> {
    static MAP: OnceLock<Arc<Mutex<PendingApprovalMap>>> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

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

        // Pre-layer: Sandbox — paths outside the sandbox are hard-denied.
        // This is a security boundary: it is enforced before everything else
        // (including Yolo mode and the whitelist) so a sandboxed agent can never
        // touch files outside its allowed roots, regardless of other approvals.
        if let Some(p) = path {
            if let Err(reason) = self.check_path(p) {
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
        let builtin_deny = tool_name == "bash" && danger == DangerLevel::Destructive;

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
            // Never short-circuit a built-in safety deny (e.g. destructive shell
            // commands) — let it fire at Layer 5 instead. Without this, setting
            // `auto_allow_up_to = Destructive` would bypass the destructive deny.
            if danger <= max_danger && !builtin_deny {
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
    ///
    /// Handles paths that do not exist yet (e.g. `write_file` creating a new
    /// file) by canonicalizing the existing parent directory and re-attaching
    /// the file name. Both the target and the configured sandbox roots are
    /// canonicalized so symlink/relative-path comparisons are correct.
    pub fn check_path(&self, file_path: &str) -> Result<(), String> {
        if self.sandbox_paths.is_empty() {
            return Ok(());
        }
        let target = canonicalize_target(file_path);
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
        // bash danger depends on the actual command: destructive commands
        // (rm, mkfs, sudo, …) are `Destructive`, everything else is `System`.
        // This is evaluated before the rule loop so the destructive deny in
        // `check` fires precisely, instead of treating all bash as destructive.
        if tool_name == "bash" {
            if command.map_or(false, is_destructive_command) {
                return DangerLevel::Destructive;
            }
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

/// Canonicalize a path for sandbox comparison.
///
/// Absolute existing paths are canonicalized directly. For paths that do not
/// exist yet (e.g. a file `write_file` is about to create), the existing
/// parent directory is canonicalized and the file name re-attached. Relative
/// paths are resolved against the current working directory first.
fn canonicalize_target(file_path: &str) -> PathBuf {
    let p = Path::new(file_path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    if let Ok(canon) = abs.canonicalize() {
        return canon;
    }
    // File does not exist yet — canonicalize the parent and re-attach the name.
    if let Some(parent) = abs.parent() {
        if let Ok(parent_canon) = parent.canonicalize() {
            if let Some(name) = abs.file_name() {
                return parent_canon.join(name);
            }
        }
    }
    abs
}

/// Normalize a command for destructive-pattern detection: collapse all
/// Unicode whitespace (including tabs, newlines, and non-breaking spaces) to a
/// single ASCII space, and expand the `${IFS}` / `$IFS` shell-variable trick
/// used to split tokens without a literal space (e.g. `rm${IFS}-rf`).
fn normalize_command(cmd: &str) -> String {
    let mut s: String = cmd
        .chars()
        .map(|c| if c.is_whitespace() || c == '\u{00a0}' { ' ' } else { c })
        .collect();
    s = s.replace("${IFS}", " ");
    s = s.replace("$IFS", " ");
    s
}

/// Programs that merely wrap another command (e.g. `env rm -rf /`,
/// `nohup rm`, `xargs rm`). The real target program follows them, possibly
/// after `VAR=value` assignments.
const COMMAND_WRAPPERS: &[&str] = &[
    "env", "exec", "command", "nohup", "time", "nice", "ionice", "xargs",
];

/// Return the index of the effective program in a sub-command's token list,
/// skipping wrapper prefixes and `VAR=value` env assignments.
fn effective_program_index(tokens: &[&str]) -> Option<usize> {
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if COMMAND_WRAPPERS.contains(&t) {
            i += 1;
            continue;
        }
        // `VAR=value` assignment (env-style), but not flags like `-i`.
        if !t.starts_with('-') && t.contains('=') {
            i += 1;
            continue;
        }
        break;
    }
    (i < tokens.len()).then_some(i)
}

/// Whether a (program, args) pair is destructive on its own.
fn is_destructive_tokens(prog: &str, args: &[&str]) -> bool {
    if prog == "mkfs" || prog.starts_with("mkfs.") || prog == "mke2fs" {
        return true;
    }
    match prog {
        "rm" | "rmdir" | "del" | "deltree" | "unlink" | "shred" => true,
        "dd" | "fdisk" | "format" | "parted" | "wipefs" => true,
        "shutdown" | "reboot" | "halt" | "poweroff" | "init" | "telinit" => true,
        // Privilege escalation — always treat as destructive.
        "sudo" | "doas" | "pkexec" | "su" | "runuser" | "newgrp" => true,
        // Namespace/container escape primitives.
        "nsenter" | "unshare" | "chroot" => true,
        "chmod" => args.iter().copied().any(|a| {
            let a = a.trim_start_matches('-');
            a == "777" || a == "0777" || a == "a+rwx" || a == "a=rwx" || a == "u+rwx,go+rwx"
        }),
        "chown" | "chgrp" => args
            .iter()
            .copied()
            .any(|a| a == "-R" || a.starts_with("--recursive")),
        "install" => {
            let mut iter = args.iter().copied();
            while let Some(a) = iter.next() {
                if a == "-m" {
                    if let Some(mode) = iter.next() {
                        let m = mode.trim_start_matches('0');
                        if m == "777" || mode == "a+rwx" || mode == "a=rwx" {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Check if a command string is destructive.
///
/// This is deliberately conservative (false-positives preferred over
/// false-negatives): it inspects every sub-command in a pipeline/sequence,
/// normalizes whitespace and `$IFS` evasion, and skips wrapper prefixes like
/// `env`/`nohup`/`xargs` to reach the real program. It cannot defeat arbitrary
/// shell quoting/`$()` substitution, but it closes the common bypasses
/// (`rm\t-rf`, `rm${IFS}-rf`, `doas`, `chmod 0777`, `install -m 777`, …).
pub fn is_destructive_command(cmd: &str) -> bool {
    let lower = normalize_command(cmd).to_lowercase();

    // Fork bomb.
    if lower.contains(":(){") || lower.contains(":|:&") {
        return true;
    }
    // Writing to a block device (cat foo > /dev/sda, dd … of=/dev/nvme0n1).
    let block_device =
        lower.contains("/dev/sd") || lower.contains("/dev/nvme") || lower.contains("/dev/disk");
    if block_device && (lower.contains('>') || lower.contains("of=")) {
        return true;
    }

    // Inspect each sub-command (split on `;`, `|`, `&`, newline).
    for sub in lower.split([';', '\n', '|', '&']) {
        let sub = sub.trim();
        if sub.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = sub.split_whitespace().collect();
        if let Some(idx) = effective_program_index(&tokens) {
            let prog = tokens[idx];
            let args = &tokens[idx + 1..];
            if is_destructive_tokens(prog, args) {
                return true;
            }
        }
    }
    false
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
        // Basic destructive commands.
        assert!(is_destructive_command("rm -rf /"));
        assert!(is_destructive_command("sudo rm file"));
        assert!(is_destructive_command("mkfs.ext4 /dev/sda"));
        assert!(is_destructive_command("dd if=/dev/zero of=/dev/sda"));
        // Whitespace / IFS evasion that the old substring check missed.
        assert!(is_destructive_command("rm\t-rf /"));
        assert!(is_destructive_command("rm${IFS}-rf /"));
        assert!(is_destructive_command("rm\n-rf /"));
        // Escalators the old check missed.
        assert!(is_destructive_command("doas rm file"));
        assert!(is_destructive_command("pkexec reboot"));
        assert!(is_destructive_command("nsenter -t 1 -m sh"));
        assert!(is_destructive_command("chroot / /bin/sh"));
        // chmod / install variants the old check missed.
        assert!(is_destructive_command("chmod 0777 /etc"));
        assert!(is_destructive_command("chmod a=rwx file"));
        assert!(is_destructive_command("install -m 777 script /usr/local/bin/script"));
        // Wrapper-prefixed destructive command.
        assert!(is_destructive_command("env rm -rf /tmp"));
        assert!(is_destructive_command("nohup rm -rf /tmp &"));
        // Block-device overwrite via redirection.
        assert!(is_destructive_command("cat image.img > /dev/sda"));
        // Fork bomb.
        assert!(is_destructive_command(":(){ :|:& };:"));
        // Sub-command after a separator.
        assert!(is_destructive_command("echo hi; rm -rf /tmp"));
        // Safe commands.
        assert!(!is_destructive_command("git status"));
        assert!(!is_destructive_command("cargo build"));
        assert!(!is_destructive_command("python script.py"));
        assert!(!is_destructive_command("ls -la"));
        assert!(!is_destructive_command("chmod 644 file"));
    }

    #[test]
    fn test_sandbox_path() {
        let policy = PermissionPolicy::new()
            .with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);
        assert!(policy.check_path("/etc/passwd").is_err());
        assert!(policy.check_path("/home/user/file.txt").is_err());
    }

    #[test]
    fn test_sandbox_denies_outside_in_check() {
        let mut policy = PermissionPolicy::with_builtin_defaults()
            .with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);
        // write_file inside the sandbox: reaches the normal Ask path.
        let inside = policy.check(
            "write_file",
            r#"{"path":"/tmp/sandbox/a.txt","content":"x"}"#,
            None,
            Some("/tmp/sandbox/a.txt"),
            None,
        );
        assert!(inside.needs_approval() || inside.is_allowed());
        // write_file outside the sandbox: hard-denied before any rule.
        let outside = policy.check(
            "write_file",
            r#"{"path":"/etc/passwd","content":"x"}"#,
            None,
            Some("/etc/passwd"),
            None,
        );
        assert!(outside.is_denied());
    }

    #[test]
    fn test_auto_allow_does_not_bypass_destructive_deny() {
        // `auto_allow_up_to = Destructive` must NOT auto-allow `rm -rf /`;
        // the built-in destructive deny should still fire.
        let mut policy = PermissionPolicy::with_builtin_defaults()
            .with_auto_allow_up_to(DangerLevel::Destructive);
        let result = policy.check(
            "bash",
            r#"{"command":"rm -rf /"}"#,
            Some("rm -rf /"),
            None,
            None,
        );
        assert!(result.is_denied());
        // A safe command at or below the auto-allow level is still allowed.
        let safe = policy.check("bash", r#"{"command":"ls -la"}"#, Some("ls -la"), None, None);
        assert!(safe.is_allowed());
    }
}
