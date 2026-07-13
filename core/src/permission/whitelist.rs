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
            tracing::warn!(expired_count = expired, "pruned expired whitelist entries");
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

    /// Query whitelist for a matching entry. Returns `Some(entry)` (cloned) if found.
    /// The entry is "touched" (last_used, use_count updated).
    /// Once-scoped entries are consumed (removed) after the first match.
    pub fn query(
        &mut self,
        tool_name: &str,
        command: Option<&str>,
        path: Option<&str>,
        host: Option<&str>,
    ) -> Option<WhitelistEntry> {
        // Purge expired first
        self.purge_expired();

        // Find matching entry index
        let match_idx = self.entries.iter().position(|entry| {
            if !entry.pattern.matches_tool(tool_name) {
                return false;
            }
            if let Some(cmd) = command {
                if !entry.pattern.matches_command(cmd) {
                    return false;
                }
            }
            if let Some(p) = path {
                if !entry.pattern.matches_path(p) {
                    return false;
                }
            }
            if let Some(h) = host {
                if !entry.pattern.matches_host(h) {
                    return false;
                }
            }
            true
        });

        if let Some(idx) = match_idx {
            // Touch the entry
            self.entries[idx].touch();

            // Once entries are consumed after one match
            let is_once = matches!(self.entries[idx].scope, ApprovalScope::Once);

            if is_once {
                let entry = self.entries.remove(idx);
                Some(entry)
            } else {
                Some(self.entries[idx].clone())
            }
        } else {
            None
        }
    }

    /// Remove expired entries (timed-out Duration scopes).
    /// Once-scoped entries are NOT removed here — they are consumed in
    /// [`query`] after the first successful match.
    fn purge_expired(&mut self) {
        self.entries.retain(|e| {
            // Keep session/persistent/task entries
            if matches!(
                e.scope,
                ApprovalScope::Session | ApprovalScope::Persistent | ApprovalScope::Task(_)
            ) {
                return true;
            }
            // Keep Once entries — they are consumed on match, not by expiry.
            if matches!(e.scope, ApprovalScope::Once) {
                return true;
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

    /// Persist persistent entries to config.toml using toml_edit to safely
    /// update the [[permissions.whitelist]] section while preserving comments
    /// and formatting in the rest of the file.
    pub fn persist_to_config(&self) -> Result<()> {
        use toml_edit::{DocumentMut, Item, Table, value};

        let config_path = match &self.config_path {
            Some(p) => p.clone(),
            None => return Ok(()),
        };

        let content = std::fs::read_to_string(&config_path)?;
        let mut doc = content.parse::<DocumentMut>()
            .map_err(|e| anyhow::anyhow!("failed to parse config TOML: {e}"))?;

        // Collect persistent entries
        let persistent: Vec<_> = self
            .entries
            .iter()
            .filter(|e| matches!(e.scope, ApprovalScope::Persistent) && e.is_valid())
            .collect();

        // Build new whitelist array-of-tables
        let mut whitelist_arr = toml_edit::ArrayOfTables::new();
        for entry in &persistent {
            let mut tbl = Table::new();
            tbl.insert("tool_pattern", value(&entry.pattern.tool_pattern));
            tbl.insert("scope", value("Persistent"));
            if let Some(ref cmds) = entry.pattern.commands {
                let mut arr = toml_edit::Array::new();
                for cmd in cmds {
                    arr.push(cmd.as_str());
                }
                tbl.insert("commands", Item::Value(arr.into()));
            }
            if let Some(ref paths) = entry.pattern.paths {
                let mut arr = toml_edit::Array::new();
                for path in paths {
                    arr.push(path.as_str());
                }
                tbl.insert("paths", Item::Value(arr.into()));
            }
            if let Some(ref hosts) = entry.pattern.hosts {
                let mut arr = toml_edit::Array::new();
                for host in hosts {
                    arr.push(host.as_str());
                }
                tbl.insert("hosts", Item::Value(arr.into()));
            }
            if let Some(ref max_danger) = entry.pattern.max_danger {
                tbl.insert("max_danger", value(format!("{max_danger:?}")));
            }
            whitelist_arr.push(tbl);
        }

        // Get or create permissions table, replacing old whitelist entries
        use toml_edit::Entry;
        match doc.entry("permissions") {
            Entry::Occupied(mut o) => {
                let perm_table = o.get_mut().as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("config.toml: [permissions] is not a table"))?;
                perm_table.insert("whitelist", Item::ArrayOfTables(whitelist_arr));
            }
            Entry::Vacant(v) => {
                let mut perm_table = Table::new();
                if !persistent.is_empty() {
                    perm_table.insert("whitelist", Item::ArrayOfTables(whitelist_arr));
                }
                v.insert(Item::Table(perm_table));
            }
        }

        std::fs::write(&config_path, doc.to_string())?;
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
        assert!(wl.query("shell", None, None, None).is_none());
    }

    #[test]
    fn test_command_filter() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("shell").with_commands(vec!["git".into(), "npm".into()]),
            ApprovalScope::Persistent,
        ));
        assert!(wl.query("shell", Some("git status"), None, None).is_some());
        assert!(wl.query("shell", Some("npm test"), None, None).is_some());
        assert!(wl.query("shell", Some("rm -rf /"), None, None).is_none());
    }

    #[test]
    fn test_purge_once_entries() {
        let mut wl = WhitelistManager::new();
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("shell"),
            ApprovalScope::Once,
        ));
        wl.add(WhitelistEntry::new(
            ToolPermissionPattern::simple("read_file"),
            ApprovalScope::Session,
        ));
        assert_eq!(wl.len(), 2);
        // Query triggers auto-purge: Once-scoped entries are removed
        wl.query("shell", None, None, None);
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
