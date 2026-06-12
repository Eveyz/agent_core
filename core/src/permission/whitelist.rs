//! Whitelist manager — persistent storage, query, expiry of user-approved patterns.
//!
//! The whitelist is layered:
//! 1. In-memory session entries (Once, Session, Timed, Task-scoped)
//! 2. On-disk persistent entries (from config.toml [permissions.whitelist])

use anyhow::Result;

use super::types::{ApprovalScope, WhitelistEntry};

/// Manages the whitelist: add, query, expire, persist.
pub struct WhitelistManager {
    /// In-memory entries (session-scoped, one-time, timed)
    entries: Vec<WhitelistEntry>,
    /// Path to config.toml for persistence
    config_path: Option<String>,
}

impl WhitelistManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            config_path: None,
        }
    }

    /// Set config path for persistence operations.
    pub fn with_config_path(mut self, path: &str) -> Self {
        self.config_path = Some(path.to_string());
        self
    }

    /// Load persistent entries from config's whitelist.
    pub fn load_persistent(&mut self, entries: Vec<WhitelistEntry>) {
        let total = entries.len();
        // Filter out expired entries
        let valid: Vec<_> = entries.into_iter().filter(|e| e.is_valid()).collect();
        let expired = total - valid.len();
        if expired > 0 {
            eprintln!(
                "[permission] pruned {} expired whitelist entries",
                expired
            );
        }
        self.entries.extend(valid);
    }

    /// Add a new whitelist entry.
    pub fn add(&mut self, entry: WhitelistEntry) {
        // Avoid duplicates: replace existing match for same tool + same scope
        self.entries.retain(|existing| {
            !(existing.pattern.tool_pattern == entry.pattern.tool_pattern
                && existing.scope == entry.scope)
        });
        self.entries.push(entry);
    }

    /// Query whitelist for a matching entry. Returns `(entry_index, &entry)` if found.
    /// The entry is "touched" (last_used, use_count updated).
    pub fn query(
        &mut self,
        tool_name: &str,
        command: Option<&str>,
        path: Option<&str>,
        host: Option<&str>,
    ) -> Option<&WhitelistEntry> {
        // Purge expired first
        self.purge_expired();

        for entry in &mut self.entries {
            if !entry.pattern.matches_tool(tool_name) {
                continue;
            }
            if let Some(cmd) = command {
                if !entry.pattern.matches_command(cmd) {
                    continue;
                }
            }
            if let Some(p) = path {
                if !entry.pattern.matches_path(p) {
                    continue;
                }
            }
            if let Some(h) = host {
                if !entry.pattern.matches_host(h) {
                    continue;
                }
            }
            // Match found — touch and return
            entry.touch();
            return Some(entry);
        }
        None
    }

    /// Remove once-scoped entries after they've been consumed.
    fn purge_expired(&mut self) {
        self.entries.retain(|e| {
            // Keep session/persistent entries
            if matches!(e.scope, ApprovalScope::Session | ApprovalScope::Persistent) {
                return true;
            }
            // Keep task-scoped entries
            if matches!(e.scope, ApprovalScope::Task(_)) {
                return true;
            }
            // Remove once-scoped entries (they're consumed after first match)
            if matches!(e.scope, ApprovalScope::Once) {
                return false; // remove
            }
            // Timed entries: check expiry
            e.is_valid()
        });
    }

    /// Remove a whitelist entry by its tool pattern.
    pub fn remove(&mut self, tool_pattern: &str) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|e| e.pattern.tool_pattern != tool_pattern);
        before - self.entries.len()
    }

    /// List all active entries.
    pub fn list(&self) -> &[WhitelistEntry] {
        &self.entries
    }

    /// Get entries suitable for persistence (Persistent scope only).
    pub fn persistent_entries(&self) -> Vec<&WhitelistEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.scope, ApprovalScope::Persistent))
            .collect()
    }

    /// Persist persistent entries to config.toml.
    /// This re-reads the config file, updates the [permissions.whitelist] section,
    /// and writes back while preserving the rest of the file.
    pub fn persist_to_config(&self) -> Result<()> {
        let config_path = match &self.config_path {
            Some(p) => p.clone(),
            None => return Ok(()), // no persistence configured
        };

        let content = std::fs::read_to_string(&config_path)?;
        let persistent: Vec<_> = self
            .entries
            .iter()
            .filter(|e| matches!(e.scope, ApprovalScope::Persistent) && e.is_valid())
            .collect();

        // Build TOML array for whitelist entries
        let mut new_whitelist_section = String::new();
        new_whitelist_section.push_str("[[permissions.whitelist]]\n");
        for entry in &persistent {
            new_whitelist_section.push_str(&format!(
                "tool_pattern = \"{}\"\n",
                entry.pattern.tool_pattern
            ));
            if let Some(ref cmds) = entry.pattern.commands {
                new_whitelist_section.push_str("commands = [");
                new_whitelist_section.push_str(
                    &cmds
                        .iter()
                        .map(|c| format!("\"{}\"", c))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                new_whitelist_section.push_str("]\n");
            }
            if let Some(ref paths) = entry.pattern.paths {
                new_whitelist_section.push_str("paths = [");
                new_whitelist_section.push_str(
                    &paths
                        .iter()
                        .map(|p| format!("\"{}\"", p))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                new_whitelist_section.push_str("]\n");
            }
            new_whitelist_section.push('\n');
        }

        // Simple approach: strip old [[permissions.whitelist]] blocks, append new ones
        let mut new_content = String::new();
        let mut in_whitelist = false;
        for line in content.lines() {
            if line.trim().starts_with("[[permissions.whitelist]]") {
                in_whitelist = true;
                continue;
            }
            if in_whitelist {
                // Skip until next section header or empty line + non-whitelist content
                if line.trim().is_empty() {
                    in_whitelist = false;
                    new_content.push_str(line);
                    new_content.push('\n');
                    continue;
                }
                if line.starts_with('[') {
                    in_whitelist = false;
                    new_content.push_str(line);
                    new_content.push('\n');
                    continue;
                }
                // Skip whitelist field lines
                continue;
            }
            new_content.push_str(line);
            new_content.push('\n');
        }

        // Append new whitelist entries
        if !persistent.is_empty() {
            new_content.push('\n');
            new_content.push_str(&new_whitelist_section);
        }

        std::fs::write(&config_path, new_content)?;
        Ok(())
    }

    /// Clear all entries (for testing or reset).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Count active entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if whitelist is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for WhitelistManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::ToolPermissionPattern;

    #[test]
    fn test_add_and_query() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("git_*"),
            ApprovalScope::Session,
        ));
        assert!(wl.query("git_status", None, None, None).is_some());
        assert!(wl.query("git_commit", None, None, None).is_some());
        assert!(wl.query("run_command", None, None, None).is_none());
    }

    #[test]
    fn test_command_filter() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("run_command")
                .with_commands(vec!["git".into(), "npm".into()]),
            ApprovalScope::Persistent,
        ));
        assert!(wl.query("run_command", Some("git status"), None, None).is_some());
        assert!(wl.query("run_command", Some("npm test"), None, None).is_some());
        assert!(wl.query("run_command", Some("rm -rf /"), None, None).is_none());
    }

    #[test]
    fn test_purge_once_entries() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("run_command"),
            ApprovalScope::Once,
        ));
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("read_file"),
            ApprovalScope::Session,
        ));
        assert_eq!(wl.len(), 2);
        // Query triggers auto-purge: Once-scoped entries are removed
        wl.query("run_command", None, None, None);
        // Once entry should be purged, session entry remains
        assert_eq!(wl.len(), 1);
        // Explicit purge should keep session entry
        wl.purge_expired();
        assert_eq!(wl.len(), 1);
    }

    #[test]
    fn test_remove_entry() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("grep"),
            ApprovalScope::Session,
        ));
        assert_eq!(wl.len(), 1);
        let removed = wl.remove("grep");
        assert_eq!(removed, 1);
        assert!(wl.is_empty());
    }
}
