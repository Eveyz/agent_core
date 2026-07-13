//! Audit log — records every permission decision for later review.
//!
//! Format: JSONL (one JSON object per line), append-only.
//! Configurable max entries; oldest entries are trimmed when limit is reached.

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::PathBuf;

use super::types::{ApprovalLevel, AuditEntry, DangerLevel, RuleSource};

/// Manages the permission audit log.
pub struct AuditLog {
    path: PathBuf,
    max_entries: usize,
}

impl AuditLog {
    /// Create a new audit log that writes to the given file path.
    /// The parent directory must exist.
    pub fn new(path: &str, max_entries: usize) -> Result<Self> {
        let path = expand_tilde(path);
        let pb = PathBuf::from(&path);
        if let Some(parent) = pb.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create audit log directory: {:?}", parent))?;
        }
        Ok(Self {
            path: pb,
            max_entries,
        })
    }

    /// Record a permission decision.
    pub fn record(
        &self,
        tool_name: &str,
        tool_input: &str,
        decision: &ApprovalLevel,
        source: &RuleSource,
        matched_rule: &str,
        danger_level: DangerLevel,
        reason: Option<&str>,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            tool_name: tool_name.to_string(),
            tool_input: truncate(tool_input, 500),
            decision: decision.clone(),
            source: source.clone(),
            matched_rule: matched_rule.to_string(),
            danger_level,
            reason: reason.map(|s| s.to_string()),
        };

        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize audit entry");
                return;
            }
        };

        if let Err(e) = self.append_line(&line) {
            tracing::error!(error = %e, "failed to write audit entry");
        }

        // Trim if over limit
        if self.max_entries > 0 {
            let _ = self.trim_if_needed();
        }
    }

    /// Read all audit entries from the log file.
    pub fn read_all(&self) -> Result<Vec<AuditEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path)?;
        let entries: Vec<AuditEntry> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
            .collect();
        Ok(entries)
    }

    /// Get statistics about the audit log.
    pub fn stats(&self) -> Result<AuditStats> {
        let entries = self.read_all()?;
        let total = entries.len();
        let allowed = entries
            .iter()
            .filter(|e| e.decision == ApprovalLevel::Allow)
            .count();
        let denied = entries
            .iter()
            .filter(|e| e.decision == ApprovalLevel::Deny)
            .count();
        let asked = total - allowed - denied;

        Ok(AuditStats {
            total,
            allowed,
            denied,
            asked,
        })
    }

    fn append_line(&self, line: &str) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open audit log: {:?}", self.path))?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn trim_if_needed(&self) -> Result<()> {
        let entries = self.read_all()?;
        if entries.len() <= self.max_entries {
            return Ok(());
        }
        let excess = entries.len() - self.max_entries;
        let trimmed: Vec<_> = entries.into_iter().skip(excess).collect();
        let content = trimmed
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&self.path, format!("{content}\n"))?;
        Ok(())
    }
}

/// Summary statistics for the audit log.
#[derive(Debug, Clone)]
pub struct AuditStats {
    pub total: usize,
    pub allowed: usize,
    pub denied: usize,
    pub asked: usize,
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let safe_end = crate::util::floor_char_boundary(s, max_len);
        format!("{}...<truncated>", &s[..safe_end])
    }
}

fn expand_tilde(path: &str) -> String {
    crate::util::expand_tilde(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_record_and_read() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("audit.jsonl");
        let log = AuditLog::new(log_path.to_str().unwrap(), 100).unwrap();

        log.record(
            "read_file",
            r#"{"path": "/tmp/test.txt"}"#,
            &ApprovalLevel::Allow,
            &RuleSource::Builtin,
            "builtin: read_file → Allow",
            DangerLevel::ReadOnly,
            None,
        );

        log.record(
            "shell",
            r#"{"command": "rm -rf /"}"#,
            &ApprovalLevel::Deny,
            &RuleSource::Builtin,
            "builtin: shell + rm → Deny",
            DangerLevel::Destructive,
            Some("Destructive command blocked"),
        );

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].decision, ApprovalLevel::Allow);
        assert_eq!(entries[1].decision, ApprovalLevel::Deny);
    }

    #[test]
    fn test_stats() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("audit.jsonl");
        let log = AuditLog::new(log_path.to_str().unwrap(), 100).unwrap();

        log.record(
            "read_file",
            "{}",
            &ApprovalLevel::Allow,
            &RuleSource::Builtin,
            "",
            DangerLevel::ReadOnly,
            None,
        );
        log.record(
            "shell",
            "{}",
            &ApprovalLevel::Deny,
            &RuleSource::Builtin,
            "",
            DangerLevel::System,
            None,
        );
        log.record(
            "write_file",
            "{}",
            &ApprovalLevel::Ask,
            &RuleSource::Builtin,
            "",
            DangerLevel::ReadWrite,
            None,
        );

        let stats = log.stats().unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.allowed, 1);
        assert_eq!(stats.denied, 1);
        assert_eq!(stats.asked, 1);
    }
}
