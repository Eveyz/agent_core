//! Execution trace collector — high-fidelity, append-only JSONL recording of
//! the [`AgentEvent`] stream.
//!
//! This is a **pure side-channel**: it observes the event stream without
//! altering agent control flow. Recording failures are swallowed (logged to
//! stderr) so tracing can never break a run.
//!
//! Format: one JSON object per line, `{"ts": <iso8601>, "event": <AgentEvent>}`.
//! Lines exceeding `max_line_chars` are replaced with a valid truncated stub
//! so the file always stays parseable while bounding disk usage.
//!
//! Reuses the JSONL append-only paradigm established by
//! [`crate::permission::AuditLog`].

use anyhow::{Context as _, Result};
use chrono::Utc;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

use crate::types::AgentEvent;

/// Sink that records the [`AgentEvent`] stream to a JSONL file.
pub struct TraceCollector {
    file: std::fs::File,
    task_id: String,
    max_line_chars: usize,
}

impl TraceCollector {
    /// Create a new trace file at `<dir>/<task_id>.jsonl`. The directory is
    /// created if missing.
    pub fn new(dir: &str, task_id: &str) -> Result<Self> {
        let dir = expand_tilde(dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create trace directory: {dir}"))?;
        let path = PathBuf::from(&dir).join(format!("{task_id}.jsonl"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open trace file: {path:?}"))?;
        Ok(Self {
            file,
            task_id: task_id.to_string(),
            max_line_chars: 64 * 1024,
        })
    }

    /// Override the per-line size cap (default 64 KiB). Lines longer than this
    /// are written as a truncated stub.
    pub fn with_max_line_chars(mut self, max: usize) -> Self {
        self.max_line_chars = max;
        self
    }

    /// Record a single agent event. Errors are swallowed (best-effort).
    pub fn record(&mut self, event: &AgentEvent) {
        let ts = Utc::now();
        let payload = serde_json::to_value(event).unwrap_or(Value::Null);
        let line = self.format_line(ts, payload, event);

        if let Err(e) = self.write_line(&line) {
            tracing::warn!(error = %e, "failed to write trace event");
        }
    }

    /// Escape hatch for injecting synthetic/enriched lines (e.g. checkpoints
    /// with token counts) without extending [`AgentEvent`]. The caller is
    /// responsible for producing valid JSON.
    pub fn record_raw(&mut self, line: &str) {
        if let Err(e) = self.write_line(line) {
            tracing::warn!(error = %e, "failed to write raw trace line");
        }
    }

    /// Flush buffered writes to disk.
    pub fn flush(&mut self) {
        let _ = self.file.flush();
    }

    /// The task id this collector is recording under.
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    fn format_line(&self, ts: chrono::DateTime<Utc>, payload: Value, event: &AgentEvent) -> String {
        let full = serde_json::json!({ "ts": ts, "event": payload });
        let rendered = match serde_json::to_string(&full) {
            Ok(s) => s,
            Err(_) => {
                return serde_json::to_string(&serde_json::json!({
                    "ts": ts,
                    "event_tag": event_tag(event),
                    "serialize_error": true,
                }))
                .unwrap_or_else(|_| "{}".to_string());
            }
        };

        if rendered.len() <= self.max_line_chars {
            rendered
        } else {
            // Replace with a valid stub so the file stays parseable.
            serde_json::to_string(&serde_json::json!({
                "ts": ts,
                "event_tag": event_tag(event),
                "truncated": true,
                "chars": rendered.len(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        }
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        if !line.ends_with('\n') {
            writeln!(self.file, "{line}").map_err(Into::into)
        } else {
            self.file.write_all(line.as_bytes()).map_err(Into::into)
        }
    }
}

/// Short static tag identifying an event variant, used in truncated stubs.
fn event_tag(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::AgentStart => "AgentStart",
        AgentEvent::AgentEnd { .. } => "AgentEnd",
        AgentEvent::TurnStart { .. } => "TurnStart",
        AgentEvent::TurnEnd { .. } => "TurnEnd",
        AgentEvent::MessageStart { .. } => "MessageStart",
        AgentEvent::MessageUpdate { .. } => "MessageUpdate",
        AgentEvent::MessageEnd { .. } => "MessageEnd",
        AgentEvent::ToolExecutionStart { .. } => "ToolExecutionStart",
        AgentEvent::ToolExecutionUpdate { .. } => "ToolExecutionUpdate",
        AgentEvent::ToolExecutionEnd { .. } => "ToolExecutionEnd",
        AgentEvent::SubagentStart { .. } => "SubagentStart",
        AgentEvent::SubagentTurnStart { .. } => "SubagentTurnStart",
        AgentEvent::SubagentMessageUpdate { .. } => "SubagentMessageUpdate",
        AgentEvent::SubagentToolStart { .. } => "SubagentToolStart",
        AgentEvent::SubagentToolUpdate { .. } => "SubagentToolUpdate",
        AgentEvent::SubagentToolEnd { .. } => "SubagentToolEnd",
        AgentEvent::SubagentEnd { .. } => "SubagentEnd",
        AgentEvent::SubagentApprovalRequired { .. } => "SubagentApprovalRequired",
        AgentEvent::ApprovalRequired { .. } => "ApprovalRequired",
        AgentEvent::Error(_) => "Error",
        AgentEvent::Aborted { .. } => "Aborted",
        AgentEvent::WorkflowStarted { .. } => "WorkflowStarted",
        AgentEvent::WorkflowNodeStarted { .. } => "WorkflowNodeStarted",
        AgentEvent::WorkflowNodeEnded { .. } => "WorkflowNodeEnded",
        AgentEvent::WorkflowCompleted { .. } => "WorkflowCompleted",
    }
}

fn expand_tilde(path: &str) -> String {
    crate::util::expand_tilde(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use serde_json::Value;
    use tempfile::TempDir;

    fn read_lines(path: &std::path::Path) -> Vec<Value> {
        let content = std::fs::read_to_string(path).unwrap();
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    #[test]
    fn test_record_and_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t1.jsonl");
        let mut tc = TraceCollector::new(dir.path().to_str().unwrap(), "t1").unwrap();
        tc.record(&AgentEvent::AgentStart);
        tc.record(&AgentEvent::TurnStart { turn_index: 0 });
        tc.record(&AgentEvent::Error("boom".to_string()));
        tc.flush();
        drop(tc);

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["event"], "AgentStart");
        assert_eq!(lines[1]["event"]["TurnStart"]["turn_index"], 0);
        assert_eq!(lines[2]["event"]["Error"], "boom");
        // every line has a timestamp
        for l in &lines {
            assert!(l["ts"].is_string());
        }
    }

    #[test]
    fn test_huge_event_truncated_to_valid_stub() {
        let dir = TempDir::new().unwrap();
        let mut tc = TraceCollector::new(dir.path().to_str().unwrap(), "t2")
            .unwrap()
            .with_max_line_chars(256);
        // Build a ToolExecutionEnd with a very large result.
        let big = "x".repeat(50_000);
        tc.record(&AgentEvent::ToolExecutionEnd {
            tool_call_id: "c1".to_string(),
            tool_name: "bash".to_string(),
            result: big,
            is_error: false,
        });
        tc.flush();
        drop(tc);

        let path = dir.path().join("t2.jsonl");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        // Stub form: has event_tag + truncated flag, no full "event" object.
        assert_eq!(lines[0]["event_tag"], "ToolExecutionEnd");
        assert_eq!(lines[0]["truncated"], true);
        assert!(lines[0]["chars"].as_u64().unwrap() > 256);
        // The full event object must NOT be present (it was replaced).
        assert!(lines[0]["event"].is_null());
    }

    #[test]
    fn test_record_raw_escape_hatch() {
        let dir = TempDir::new().unwrap();
        let mut tc = TraceCollector::new(dir.path().to_str().unwrap(), "t3").unwrap();
        tc.record_raw(r#"{"kind":"checkpoint","token_count":1234}"#);
        tc.flush();
        drop(tc);

        let path = dir.path().join("t3.jsonl");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["kind"], "checkpoint");
        assert_eq!(lines[0]["token_count"], 1234);
    }

    #[test]
    fn test_agentend_messages_serialized() {
        let dir = TempDir::new().unwrap();
        let mut tc = TraceCollector::new(dir.path().to_str().unwrap(), "t4").unwrap();
        let msg = Message::assistant("hello");
        tc.record(&AgentEvent::AgentEnd {
            messages: vec![msg],
        });
        tc.flush();
        drop(tc);

        let path = dir.path().join("t4.jsonl");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0]["event"]["AgentEnd"]["messages"][0]["role"],
            "assistant"
        );
    }
}
