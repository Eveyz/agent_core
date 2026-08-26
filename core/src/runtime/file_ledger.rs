//! Deterministic session file touch ledger (PLAN-0016).
//!
//! Updated from successful tool calls (`read_file` / `write_file` / `edit`).
//! Injected into RollingSummary on compact — the LLM must not invent paths.

use std::collections::HashSet;
use std::path::Path;

/// Cap on paths kept across all buckets combined (approx).
pub const FILE_LEDGER_CAP: usize = 40;

/// Paths touched during a Run, classified by operation.
#[derive(Debug, Clone, Default)]
pub struct FileLedger {
    pub read: Vec<String>,
    pub wrote: Vec<String>,
    pub deleted: Vec<String>,
}

impl FileLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_read(&mut self, path: impl AsRef<Path>) {
        let p = normalize(path);
        if p.is_empty() {
            return;
        }
        // wrote dominates — keep in wrote, don't also list as read-only.
        if self.wrote.iter().any(|w| w == &p) {
            return;
        }
        push_unique_capped(&mut self.read, p, FILE_LEDGER_CAP);
        self.trim_total();
    }

    pub fn record_wrote(&mut self, path: impl AsRef<Path>) {
        let p = normalize(path);
        if p.is_empty() {
            return;
        }
        self.read.retain(|r| r != &p);
        push_unique_capped(&mut self.wrote, p, FILE_LEDGER_CAP);
        self.trim_total();
    }

    pub fn record_deleted(&mut self, path: impl AsRef<Path>) {
        let p = normalize(path);
        if p.is_empty() {
            return;
        }
        self.read.retain(|r| r != &p);
        self.wrote.retain(|w| w != &p);
        push_unique_capped(&mut self.deleted, p, FILE_LEDGER_CAP);
        self.trim_total();
    }

    /// Update from a successful tool call's name + JSON arguments.
    pub fn observe_tool(&mut self, name: &str, arguments: &str) {
        let path = extract_path_arg(arguments);
        match name {
            "read_file" => {
                if let Some(p) = path {
                    self.record_read(p);
                }
            }
            "write_file" | "edit" => {
                if let Some(p) = path {
                    self.record_wrote(p);
                }
            }
            _ => {}
        }
    }

    fn trim_total(&mut self) {
        let total = self.read.len() + self.wrote.len() + self.deleted.len();
        if total <= FILE_LEDGER_CAP {
            return;
        }
        // Drop oldest reads first, then oldest writes, then deletes.
        let mut excess = total - FILE_LEDGER_CAP;
        while excess > 0 && !self.read.is_empty() {
            self.read.remove(0);
            excess -= 1;
        }
        while excess > 0 && !self.wrote.is_empty() {
            self.wrote.remove(0);
            excess -= 1;
        }
        while excess > 0 && !self.deleted.is_empty() {
            self.deleted.remove(0);
            excess -= 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.wrote.is_empty() && self.deleted.is_empty()
    }
}

fn normalize(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().trim().to_string()
}

fn push_unique_capped(list: &mut Vec<String>, path: String, cap: usize) {
    if let Some(pos) = list.iter().position(|p| p == &path) {
        list.remove(pos);
    }
    list.push(path);
    while list.len() > cap {
        list.remove(0);
    }
}

fn extract_path_arg(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("path")
        .and_then(|p| p.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Merge two path lists with uniqueness, preferring order of `a` then new from `b`.
pub fn merge_path_lists(a: &[String], b: &[String], cap: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in a.iter().chain(b.iter()) {
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
    while out.len() > cap {
        out.remove(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrote_dominates_read() {
        let mut led = FileLedger::new();
        led.record_read("src/a.rs");
        led.record_wrote("src/a.rs");
        assert!(led.read.is_empty());
        assert_eq!(led.wrote, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn observe_tools() {
        let mut led = FileLedger::new();
        led.observe_tool("read_file", r#"{"path":"/tmp/x.rs","offset":1}"#);
        led.observe_tool(
            "edit",
            r#"{"path":"/tmp/x.rs","old_string":"a","new_string":"b"}"#,
        );
        assert!(led.read.is_empty());
        assert_eq!(led.wrote, vec!["/tmp/x.rs".to_string()]);
    }

    #[test]
    fn cap_trims_oldest_reads() {
        let mut led = FileLedger::new();
        for i in 0..50 {
            led.record_read(format!("f{i}.rs"));
        }
        assert!(led.read.len() <= FILE_LEDGER_CAP);
        assert!(!led.read.iter().any(|p| p == "f0.rs"));
    }
}
