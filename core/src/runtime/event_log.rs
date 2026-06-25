//! EventLog — append-only JSONL persistence for Run events.
//!
//! Each Run writes its events to `~/.agverse/runs/{run_id}.jsonl`.
//! This enables:
//! - **Replay**: re-execute or visualize a past Run's event sequence
//! - **Fork**: start a new Run from a past Run's context at a specific event
//! - **Audit**: full trace of what happened, for debugging and trust
//! - **Reflector**: the offline reflection framework can analyze traces
//!
//! The log is best-effort: IO failures are logged but never block execution.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;

use crate::runtime::event::Envelope;
use crate::runtime::event::RunId;

/// Append-only event log for a single Run.
pub struct EventLog {
    run_id: RunId,
    path: PathBuf,
    /// In-memory copy for fast queries (also serves as backup if file write fails).
    entries: Vec<Envelope>,
    /// Whether writes are enabled (false if the file couldn't be opened).
    writable: bool,
}

impl EventLog {
    /// Create a new EventLog for a Run, opening the JSONL file for appending.
    /// The directory is created if it doesn't exist.
    pub fn new(run_id: &str, base_dir: &str) -> Self {
        let dir = PathBuf::from(base_dir);
        let path = dir.join(format!("{run_id}.jsonl"));

        // Ensure directory exists
        let writable = std::fs::create_dir_all(&dir).is_ok();

        Self {
            run_id: run_id.to_string(),
            path,
            entries: Vec::new(),
            writable,
        }
    }

    /// Append an envelope to the log (best-effort persistence).
    pub fn append(&mut self, env: Envelope) {
        self.entries.push(env.clone());

        if !self.writable {
            return;
        }

        // Serialize and append to file
        match serde_json::to_string(&env) {
            Ok(line) => {
                // Open in append mode — if this fails, we just skip (best-effort)
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&self.path)
                {
                    let _ = writeln!(file, "{line}");
                }
            }
            Err(e) => {
                tracing::warn!(run_id = %self.run_id, error = %e, "failed to serialize event for log");
            }
        }
    }

    /// Number of events in the log.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get a reference to all entries.
    pub fn entries(&self) -> &[Envelope] {
        &self.entries
    }

    /// The run ID this log belongs to.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The file path of the JSONL log.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Load a trace from a JSONL file (for replay).
    pub fn load(path: &std::path::Path) -> Result<Vec<Envelope>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read event log: {path:?}"))?;

        let mut events = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let env: Envelope = serde_json::from_str(line)
                .with_context(|| format!("failed to parse event log line {}: {line}", i + 1))?;
            events.push(env);
        }
        Ok(events)
    }

    /// Load envelopes with `seq > from_seq` from a JSONL log (for resync).
    ///
    /// Used by the frontend to recover events lost to broadcast lag (B2).
    pub fn replay_since(path: &std::path::Path, from_seq: u64) -> Result<Vec<Envelope>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read event log: {path:?}"))?;

        let mut events = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Envelope>(line) {
                Ok(env) if env.seq > from_seq => events.push(env),
                Ok(_) => {}
                Err(_) => continue,
            }
        }
        Ok(events)
    }

    /// List all Run IDs that have event logs in the given directory.
    pub fn list_runs(base_dir: &str) -> Result<Vec<RunId>> {
        let dir = PathBuf::from(base_dir);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut run_ids = Vec::new();
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("failed to read runs dir: {dir:?}"))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    run_ids.push(stem.to_string());
                }
            }
        }
        run_ids.sort();
        Ok(run_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::event::{Envelope, RunEvent};
    use crate::runtime::state::RunState;
    use tempfile::TempDir;

    #[test]
    fn append_writes_to_file() {
        let dir = TempDir::new().unwrap();
        let mut log = EventLog::new("run-1", dir.path().to_str().unwrap());

        log.append(Envelope {
            seq: 0,
            event_id: "e0".into(),
            run_id: "run-1".into(),
            turn_id: None,
            parent_call_id: None,
            event: RunEvent::RunCreated {
                id: "run-1".into(),
                session_id: None,
            },
        });
        log.append(Envelope {
            seq: 1,
            event_id: "e1".into(),
            run_id: "run-1".into(),
            turn_id: None,
            parent_call_id: None,
            event: RunEvent::RunStarted,
        });

        assert_eq!(log.len(), 2);

        // File should exist and contain 2 lines
        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn load_reads_back_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("run-2.jsonl");

        // Write some events
        {
            let mut log = EventLog::new("run-2", dir.path().to_str().unwrap());
            log.append(Envelope {
                seq: 0,
                event_id: "e0".into(),
                run_id: "run-2".into(),
                turn_id: None,
                parent_call_id: None,
                event: RunEvent::RunCreated {
                    id: "run-2".into(),
                    session_id: None,
                },
            });
            log.append(Envelope {
                seq: 1,
                event_id: "e1".into(),
                run_id: "run-2".into(),
                turn_id: None,
                parent_call_id: None,
                event: RunEvent::StateChanged {
                    from: RunState::Created,
                    to: RunState::Running,
                },
            });
            log.append(Envelope {
                seq: 2,
                event_id: "e2".into(),
                run_id: "run-2".into(),
                turn_id: None,
                parent_call_id: None,
                event: RunEvent::RunCompleted {
                    final_text: "done".into(),
                },
            });
        }

        // Load them back
        let events = EventLog::load(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].event, RunEvent::RunCreated { .. }));
        assert!(matches!(events[2].event, RunEvent::RunCompleted { .. }));
    }

    #[test]
    fn list_runs_finds_jsonl_files() {
        let dir = TempDir::new().unwrap();

        {
            let mut log1 = EventLog::new("run-a", dir.path().to_str().unwrap());
            log1.append(Envelope {
                seq: 0,
                event_id: "e0".into(),
                run_id: "run-a".into(),
                turn_id: None,
                parent_call_id: None,
                event: RunEvent::RunStarted,
            });
            let mut log2 = EventLog::new("run-b", dir.path().to_str().unwrap());
            log2.append(Envelope {
                seq: 0,
                event_id: "e0".into(),
                run_id: "run-b".into(),
                turn_id: None,
                parent_call_id: None,
                event: RunEvent::RunStarted,
            });
        }

        let runs = EventLog::list_runs(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.contains(&"run-a".to_string()));
        assert!(runs.contains(&"run-b".to_string()));
    }

    #[test]
    fn list_runs_empty_dir() {
        let dir = TempDir::new().unwrap();
        let runs = EventLog::list_runs(dir.path().to_str().unwrap()).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn list_runs_nonexistent_dir() {
        let runs = EventLog::list_runs("/nonexistent/path/that/does/not/exist").unwrap();
        assert!(runs.is_empty());
    }
}
