//! Permission system types: danger levels, modes, approval scopes, patterns.
//!
//! Design principles:
//! 1. Each tool has an inherent `DangerLevel` — the system never lets a
//!    low-danger tool masquerade as high-danger.
//! 2. Rules are multi-dimensional: tool name + command + path + host.
//! 3. Whitelist supports scoped approvals (once / session / timed / persistent).
//! 4. Audit trail logs every permission decision.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

// ── Permission mode (global policy posture) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Deny-by-default. Every tool call prompts the user. Safest.
    Paranoid,
    /// Built-in defaults: read-only → allow, write → ask, destructive → deny.
    Standard,
    /// Most things allowed. Only system/network/destructive → ask.
    Permissive,
    /// Everything allowed. No prompts. Use at your own risk.
    Yolo,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Standard
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paranoid => write!(f, "paranoid"),
            Self::Standard => write!(f, "standard"),
            Self::Permissive => write!(f, "permissive"),
            Self::Yolo => write!(f, "yolo"),
        }
    }
}

// ── Danger level (intrinsic to each tool) ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DangerLevel {
    /// Read-only: read_file, glob, grep, git_status, git_diff, git_log, git_show
    ReadOnly = 0,
    /// Read-write: write_file, edit, git_commit
    ReadWrite = 10,
    /// Network access: webfetch, API calls
    Network = 20,
    /// System shell: run_command
    System = 30,
    /// Destructive: rm, mkfs, dd, sudo, chmod 777
    Destructive = 40,
}

impl Default for DangerLevel {
    fn default() -> Self {
        Self::ReadOnly
    }
}

impl fmt::Display for DangerLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "read-only"),
            Self::ReadWrite => write!(f, "read-write"),
            Self::Network => write!(f, "network"),
            Self::System => write!(f, "system"),
            Self::Destructive => write!(f, "destructive"),
        }
    }
}

// ── Approval level (per-rule decision) ──────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalLevel {
    /// Run without asking.
    Allow,
    /// Prompt user for confirmation.
    Ask,
    /// Block unconditionally — overrides whitelist.
    Deny,
}

// ── Approval scope (user's choice when prompted) ────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalScope {
    /// Single use, consumed immediately.
    Once,
    /// Valid until agent process restarts.
    Session,
    /// Valid for a limited duration from approval time (serialized as "5m", "1h", etc.)
    Duration(String),
    /// Valid for a specific task ID.
    Task(String),
    /// Saved to config.toml, persists across restarts.
    Persistent,
}

impl Default for ApprovalScope {
    fn default() -> Self {
        Self::Session
    }
}

impl Serialize for ApprovalScope {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = match self {
            Self::Once => "once".to_string(),
            Self::Session => "session".to_string(),
            Self::Duration(d) => d.clone(),
            Self::Task(id) => format!("task:{}", id),
            Self::Persistent => "persistent".to_string(),
        };
        s.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for ApprovalScope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "once" => Ok(Self::Once),
            "session" => Ok(Self::Session),
            "persistent" => Ok(Self::Persistent),
            other => {
                if let Some(rest) = other.strip_prefix("task:") {
                    Ok(Self::Task(rest.to_string()))
                } else {
                    // Assume it's a duration string like "5m", "1h", "300"
                    Ok(Self::Duration(other.to_string()))
                }
            }
        }
    }
}

impl ApprovalScope {
    /// Return true if this scope is still valid (not expired).
    pub fn is_valid(&self, approved_at: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
        match self {
            Self::Once => false,
            Self::Session => true,
            Self::Duration(d_str) => {
                let secs = parse_duration_str(d_str);
                let elapsed = *now - *approved_at;
                elapsed.to_std().map(|e| e.as_secs() < secs).unwrap_or(false)
            }
            Self::Task(_) => true,
            Self::Persistent => true,
        }
    }
}

/// Parse a duration string like "5m", "1h", "30s", "600" into seconds.
fn parse_duration_str(s: &str) -> u64 {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return n; // raw seconds
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().unwrap_or(300);
    match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => 300, // default 5 minutes
    }
}

// ── Rule source (where a permission decision came from) ─────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSource {
    /// Hardcoded in rules.rs
    Builtin,
    /// From config.toml [permissions.rules]
    Config,
    /// From config.toml [permissions.whitelist]
    Whitelist,
    /// From runtime user approval
    UserPrompt,
}

// ── Multi-dimensional tool permission pattern ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionPattern {
    /// Glob pattern for tool name: "read_file", "write_*", "*"
    pub tool_pattern: String,
    /// For run_command: list of allowed command prefixes ["git", "cargo"]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// For file ops: allowed path patterns ["/workspace/**", "~/.config/*"]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// For network ops: allowed host patterns ["api.github.com", "*.openai.com"]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    /// Maximum danger level this pattern applies to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_danger: Option<DangerLevel>,
}

impl ToolPermissionPattern {
    pub fn simple(tool_pattern: &str) -> Self {
        Self {
            tool_pattern: tool_pattern.to_string(),
            commands: None,
            paths: None,
            hosts: None,
            max_danger: None,
        }
    }

    pub fn with_commands(mut self, cmds: Vec<String>) -> Self {
        self.commands = Some(cmds);
        self
    }

    pub fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.paths = Some(paths);
        self
    }

    pub fn with_max_danger(mut self, level: DangerLevel) -> Self {
        self.max_danger = Some(level);
        self
    }

    /// Match tool name against this pattern.
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        glob_match(&self.tool_pattern, tool_name)
    }

    /// Match a command string (for run_command) against allowed commands.
    pub fn matches_command(&self, command: &str) -> bool {
        match &self.commands {
            Some(allowed) => {
                let cmd = command.trim();
                allowed.iter().any(|prefix| cmd.starts_with(prefix.as_str()))
            }
            None => true, // no command restriction
        }
    }

    /// Match a path against allowed paths.
    pub fn matches_path(&self, path: &str) -> bool {
        match &self.paths {
            Some(allowed) => allowed.iter().any(|p| glob_match(p, path)),
            None => true,
        }
    }

    /// Match a host against allowed hosts.
    pub fn matches_host(&self, host: &str) -> bool {
        match &self.hosts {
            Some(allowed) => allowed.iter().any(|h| glob_match(h, host)),
            None => true,
        }
    }
}

// ── Whitelist entry (persisted to config.toml) ──────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    /// Multi-dimensional match pattern
    pub pattern: ToolPermissionPattern,
    /// When this entry was created
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// When this entry was last matched
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    /// How many times this entry has been matched
    #[serde(default)]
    pub use_count: u64,
    /// When this entry expires (None = permanent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<DateTime<Utc>>,
    /// Approval scope
    #[serde(default)]
    pub scope: ApprovalScope,
}

impl WhitelistEntry {
    pub fn new(pattern: ToolPermissionPattern, scope: ApprovalScope) -> Self {
        let expires = match &scope {
            ApprovalScope::Duration(d_str) => {
                let secs = parse_duration_str(d_str);
                Some(Utc::now() + chrono::Duration::seconds(secs as i64))
            }
            _ => None,
        };
        Self {
            pattern,
            created_at: Utc::now(),
            last_used: None,
            use_count: 0,
            expires,
            scope,
        }
    }

    pub fn touch(&mut self) {
        self.last_used = Some(Utc::now());
        self.use_count += 1;
    }

    /// Check if this entry is still valid.
    pub fn is_valid(&self) -> bool {
        let now = Utc::now();
        // Check scope validity
        if !self.scope.is_valid(&self.created_at, &now) {
            return false;
        }
        // Check explicit expiry (for Duration scope)
        if let Some(expires) = self.expires {
            return now < expires;
        }
        true
    }
}

// ── Audit log entry ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
    pub tool_input: String,
    pub decision: ApprovalLevel,
    pub source: RuleSource,
    pub matched_rule: String,
    pub danger_level: DangerLevel,
    /// User-specified reason (for denials)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── Approval prompt (shown to user) ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub prompt_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub danger_level: DangerLevel,
    pub matched_rule: String,
    pub explanation: String,
}

/// User's response to an approval prompt.
#[derive(Debug, Clone)]
pub enum ApprovalChoice {
    /// Allow this specific invocation.
    AllowOnce,
    /// Allow for the rest of this session.
    AllowSession,
    /// Allow for a limited time.
    AllowFor(std::time::Duration),
    /// Allow permanently (save to config).
    AllowPersistent,
    /// Deny this invocation.
    Deny,
    /// Deny permanently (add to deny list).
    DenyPersistent,
}

// ── Permission config (for config.toml) ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionConfig {
    /// Global permission posture
    #[serde(default)]
    pub mode: PermissionMode,

    /// Maximum danger level to auto-allow (overrides mode defaults)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_allow_up_to: Option<DangerLevel>,

    /// User-defined rules (overlay on built-in)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ConfigRule>,

    /// Whitelist entries (user-approved patterns)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whitelist: Vec<WhitelistEntry>,

    /// Blacklist — unconditionally denied patterns
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blacklist: Vec<ToolPermissionPattern>,

    /// Sandbox paths — restrict file operations to these roots
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sandbox_paths: Vec<String>,

    /// Max audit log entries (0 = unlimited)
    #[serde(default = "default_audit_max")]
    pub audit_max_entries: usize,

    /// Path for audit log (relative to config dir)
    #[serde(default = "default_audit_path")]
    pub audit_path: String,
}

fn default_audit_max() -> usize {
    10000
}

fn default_audit_path() -> String {
    "audit.jsonl".to_string()
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            mode: PermissionMode::default(),
            auto_allow_up_to: None,
            rules: Vec::new(),
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            sandbox_paths: Vec::new(),
            audit_max_entries: 10000,
            audit_path: "audit.jsonl".to_string(),
        }
    }
}

/// A config-level permission rule (for config.toml [permissions.rules]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigRule {
    pub pattern: ToolPermissionPattern,
    pub level: ApprovalLevel,
}

// ── Glob matching ───────────────────────────────────────────────────

pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Literal match (no wildcards)
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == text;
    }
    // Simple glob-to-regex
    let regex_pattern = pattern
        .replace('.', "\\.")
        .replace('*', ".*")
        .replace('?', ".");
    regex::Regex::new(&format!("^{}$", regex_pattern))
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("read_file", "read_file"));
        assert!(!glob_match("read_file", "write_file"));
    }

    #[test]
    fn test_glob_match_wildcard() {
        assert!(glob_match("git_*", "git_status"));
        assert!(glob_match("git_*", "git_commit"));
        assert!(!glob_match("git_*", "grep"));
    }

    #[test]
    fn test_tool_pattern_matches_command() {
        let pat = ToolPermissionPattern::simple("run_command")
            .with_commands(vec!["git".into(), "cargo".into()]);
        assert!(pat.matches_command("git status"));
        assert!(pat.matches_command("cargo build"));
        assert!(!pat.matches_command("rm -rf /"));
    }

    #[test]
    fn test_whitelist_entry_valid() {
        let entry = WhitelistEntry::new(
            ToolPermissionPattern::simple("git_*"),
            ApprovalScope::Persistent,
        );
        assert!(entry.is_valid());
    }

    #[test]
    fn test_whitelist_entry_once_consumed() {
        let entry = WhitelistEntry::new(
            ToolPermissionPattern::simple("run_command"),
            ApprovalScope::Once,
        );
        // Once scope says is_valid = false (consumed), but we don't delete it.
        // The caller should check scope and decide.
        assert!(!entry.scope.is_valid(&entry.created_at, &Utc::now()));
    }

    #[test]
    fn test_danger_level_ordering() {
        assert!(DangerLevel::ReadOnly < DangerLevel::ReadWrite);
        assert!(DangerLevel::ReadWrite < DangerLevel::Network);
        assert!(DangerLevel::System < DangerLevel::Destructive);
    }
}
