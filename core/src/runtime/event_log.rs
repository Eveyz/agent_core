//! EventLog — append-only JSONL persistence for Run events.
//!
//! Each Run writes its events to `~/.agverse/runs/{run_id}.jsonl`.
//! This enables:
//! - **Replay**: re-execute or visualize a past Run's event sequence
//! - **Fork**: start a new Run from a past Run's context at a specific event
//! - **Audit**: full trace of what happened, for debugging and trust
//! - **Reflector**: the offline reflection framework can analyze traces
//!
//! Publication happens only after the envelope has been appended and flushed.

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
    /// Maximum file size in bytes before rotation (default: 100 MB).
    max_file_size: u64,
    /// Maximum number of rotated backup files to keep (default: 5).
    max_files: usize,
    /// Approximate bytes written since last rotation check.
    bytes_written: u64,
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
            max_file_size: 100 * 1024 * 1024, // 100 MB
            max_files: 5,
            bytes_written: 0,
        }
    }

    /// Configure rotation limits for this log.
    pub fn with_rotation(mut self, max_file_size: u64, max_files: usize) -> Self {
        self.max_file_size = max_file_size;
        self.max_files = max_files;
        self
    }

    /// Rotate log files when the current file exceeds the size limit.
    /// Renames: .jsonl → .jsonl.1, .1 → .2, ..., oldest is removed.
    fn rotate_if_needed(&mut self) {
        // Only rotate if the file actually exists and we can check its size.
        if !self.writable {
            return;
        }
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            if metadata.len() < self.max_file_size {
                return;
            }
        } else {
            return; // file doesn't exist yet — nothing to rotate
        }

        tracing::info!(
            path = %self.path.display(),
            size = self.bytes_written,
            "rotating event log"
        );

        // Shift rotated files: .jsonl.N-1 → .jsonl.N
        for i in (1..self.max_files).rev() {
            let from = self.path.with_extension(format!("jsonl.{i}"));
            let to = self.path.with_extension(format!("jsonl.{}", i + 1));
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }

        // Rename current file: .jsonl → .jsonl.1
        let backup = self.path.with_extension("jsonl.1");
        let _ = std::fs::rename(&self.path, &backup);

        // Reset write tracking — a new empty file will be created on next append.
        self.bytes_written = 0;
    }

    /// Append and flush an envelope. The caller must publish only after this
    /// succeeds, otherwise replay would not be a source of truth.
    pub fn append(&mut self, env: Envelope) -> Result<()> {
        self.entries.push(env.clone());

        if !self.writable {
            anyhow::bail!("event log directory is not writable: {}", self.path.display());
        }

        // Rotate if the file has grown too large
        self.rotate_if_needed();

        // Serialize and append to file
        let line = serde_json::to_string(&env)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .with_context(|| format!("failed to open event log: {}", self.path.display()))?;
        writeln!(file, "{line}")?;
        file.flush()?;
        self.bytes_written += line.len() as u64 + 1;
        Ok(())
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
        validate_and_sort(events)
    }

    /// Load envelopes with `seq > from_seq` from a JSONL log (for resync).
    ///
    /// Used by the frontend to recover events lost to broadcast lag (B2).
    pub fn replay_since(path: &std::path::Path, from_seq: u64) -> Result<Vec<Envelope>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read event log: {path:?}"))?;

        let mut events = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Envelope>(line) {
                Ok(env) if env.seq > from_seq => events.push(env),
                Ok(_) => {}
                Err(error) => anyhow::bail!("invalid event log line {}: {error}", i + 1),
            }
        }
        validate_and_sort(events)
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

fn validate_and_sort(mut events: Vec<Envelope>) -> Result<Vec<Envelope>> {
    events.sort_by_key(|event| event.seq);
    let mut deduped = Vec::with_capacity(events.len());
    for event in events {
        if let Some(previous) = deduped.last() {
            let previous: &Envelope = previous;
            if previous.seq == event.seq {
                if previous.event_id != event.event_id {
                    anyhow::bail!(
                        "conflicting events at sequence {}: {} != {}",
                        event.seq,
                        previous.event_id,
                        event.event_id
                    );
                }
                continue;
            }
        }
        deduped.push(event);
    }
    Ok(deduped)
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
            session_id: None,
            turn_id: None,
            parent_call_id: None,
            ts: chrono::Utc::now(),
            event: RunEvent::RunCreated {
                id: "run-1".into(),
                session_id: None,
                prompt_id: None,
            },
        }).unwrap();
        log.append(Envelope {
            seq: 1,
            event_id: "e1".into(),
            run_id: "run-1".into(),
            session_id: None,
            turn_id: None,
            parent_call_id: None,
            ts: chrono::Utc::now(),
            event: RunEvent::RunStarted,
        }).unwrap();

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
                session_id: None,
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
                event: RunEvent::RunCreated {
                    id: "run-2".into(),
                    session_id: None,
                    prompt_id: None,
                },
            }).unwrap();
            log.append(Envelope {
                seq: 1,
                event_id: "e1".into(),
                run_id: "run-2".into(),
                session_id: None,
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
            event: RunEvent::StateChanged {
                    from: RunState::Created,
                    to: RunState::Running,
                },
            }).unwrap();
            log.append(Envelope {
                seq: 2,
                event_id: "e2".into(),
                run_id: "run-2".into(),
                session_id: None,
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
            event: RunEvent::RunCompleted {
                    final_text: "done".into(),
                },
            }).unwrap();
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
                session_id: None,
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
            event: RunEvent::RunStarted,
            }).unwrap();
            let mut log2 = EventLog::new("run-b", dir.path().to_str().unwrap());
            log2.append(Envelope {
                seq: 0,
                event_id: "e0".into(),
                run_id: "run-b".into(),
                session_id: None,
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
            event: RunEvent::RunStarted,
            }).unwrap();
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

    #[test]
    fn replay_sorts_and_rejects_conflicting_duplicate_sequences() {
        let event = |seq, event_id: &str| Envelope {
            seq,
            event_id: event_id.into(),
            run_id: "run-conflict".into(),
            session_id: None,
            turn_id: None,
            parent_call_id: None,
            ts: chrono::Utc::now(),
            event: RunEvent::RunStarted,
        };
        let sorted = validate_and_sort(vec![event(2, "e2"), event(1, "e1")]).unwrap();
        assert_eq!(sorted.iter().map(|event| event.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert!(validate_and_sort(vec![event(1, "e1"), event(1, "different")]).is_err());
    }
}
