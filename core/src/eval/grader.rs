//! Task graders for harness eval.

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use serde::Deserialize;

use crate::runtime::event::{Envelope, RunEvent};

use super::ledger::RunLedger;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraderKind {
    #[default]
    ExpectEvents,
    Command,
    FileEquals,
    /// Always pass (metrics-only / smoke).
    None,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GraderSpec {
    #[serde(default)]
    pub kind: GraderKind,
    /// Shell command relative to workspace (for Command).
    #[serde(default)]
    pub command: Option<String>,
    /// Relative path + expected contents (for FileEquals).
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub expect: Option<String>,
    /// Required event type names (snake_case tag), e.g. "run_completed".
    #[serde(default)]
    pub require_events: Vec<String>,
    /// Forbidden event type names.
    #[serde(default)]
    pub forbid_events: Vec<String>,
    /// If set, ledger must include these fail tags (taxonomy self-check).
    #[serde(default)]
    pub require_fail_tags: Vec<String>,
    /// If set, ledger must NOT include these fail tags.
    #[serde(default)]
    pub forbid_fail_tags: Vec<String>,
    /// Require terminal event name: RunCompleted | RunFailed | RunCancelled
    #[serde(default)]
    pub require_terminal: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GradeOutcome {
    pub pass: bool,
    pub note: Option<String>,
}

pub fn grade(
    spec: &GraderSpec,
    envelopes: &[Envelope],
    ledger: &RunLedger,
    workspace: &Path,
) -> Result<GradeOutcome> {
    match spec.kind {
        GraderKind::None => Ok(GradeOutcome {
            pass: true,
            note: None,
        }),
        GraderKind::ExpectEvents => grade_expect_events(spec, envelopes, ledger),
        GraderKind::Command => grade_command(spec, workspace),
        GraderKind::FileEquals => grade_file_equals(spec, workspace),
    }
}

fn event_tag(ev: &RunEvent) -> &'static str {
    match ev {
        RunEvent::RunCreated { .. } => "run_created",
        RunEvent::RunStarted => "run_started",
        RunEvent::RunPaused => "run_paused",
        RunEvent::RunResumed => "run_resumed",
        RunEvent::RunCompleted { .. } => "run_completed",
        RunEvent::RunCancelled { .. } => "run_cancelled",
        RunEvent::RunFailed { .. } => "run_failed",
        RunEvent::StateChanged { .. } => "state_changed",
        RunEvent::TurnStarted { .. } => "turn_started",
        RunEvent::TurnEnded { .. } => "turn_ended",
        RunEvent::ModelCallStarted => "model_call_started",
        RunEvent::ModelStreaming { .. } => "model_streaming",
        RunEvent::ModelCallEnded { .. } => "model_call_ended",
        RunEvent::MessageStart { .. } => "message_start",
        RunEvent::MessageUpdate { .. } => "message_update",
        RunEvent::MessageEnd { .. } => "message_end",
        RunEvent::MessageInterrupted { .. } => "message_interrupted",
        RunEvent::ToolPreparing { .. } => "tool_preparing",
        RunEvent::ToolStarted { .. } => "tool_started",
        RunEvent::ToolUpdate { .. } => "tool_update",
        RunEvent::ToolEnded { .. } => "tool_ended",
        RunEvent::ApprovalRequired { .. } => "approval_required",
        RunEvent::ApprovalResolved { .. } => "approval_resolved",
        RunEvent::InputRequested { .. } => "input_requested",
        RunEvent::InputResolved { .. } => "input_resolved",
        RunEvent::ContextCompacted { .. } => "context_compacted",
        RunEvent::Notice { .. } => "notice",
        RunEvent::Error { .. } => "error",
        RunEvent::SteerQueued { .. } => "steer_queued",
        RunEvent::SteerInjected { .. } => "steer_injected",
        RunEvent::SteerCancelled { .. } => "steer_cancelled",
        RunEvent::SteerFailed { .. } => "steer_failed",
        RunEvent::SubagentStarted { .. } => "subagent_started",
        RunEvent::SubagentEnded { .. } => "subagent_ended",
        RunEvent::ProcessSpawned { .. } => "process_spawned",
        RunEvent::ProcessKilled { .. } => "process_killed",
        RunEvent::TodoUpdated { .. } => "todo_updated",
        RunEvent::GoalSet { .. } => "goal_set",
        RunEvent::GoalCompleted { .. } => "goal_completed",
        RunEvent::GoalCleared => "goal_cleared",
        RunEvent::CacheInfo { .. } => "cache_info",
        RunEvent::CacheSummary { .. } => "cache_summary",
    }
}

fn grade_expect_events(
    spec: &GraderSpec,
    envelopes: &[Envelope],
    ledger: &RunLedger,
) -> Result<GradeOutcome> {
    let tags: Vec<&str> = envelopes.iter().map(|e| event_tag(&e.event)).collect();

    for req in &spec.require_events {
        if !tags.iter().any(|t| *t == req.as_str()) {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!("missing required event: {req}")),
            });
        }
    }
    for forbid in &spec.forbid_events {
        if tags.iter().any(|t| *t == forbid.as_str()) {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!("forbidden event present: {forbid}")),
            });
        }
    }
    if let Some(term) = &spec.require_terminal {
        if &ledger.result.terminal != term {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!(
                    "terminal want {term}, got {}",
                    ledger.result.terminal
                )),
            });
        }
    }
    for tag in &spec.require_fail_tags {
        if !ledger.result.fail_tags.iter().any(|t| t == tag) {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!("missing required fail_tag: {tag}")),
            });
        }
    }
    for tag in &spec.forbid_fail_tags {
        if ledger.result.fail_tags.iter().any(|t| t == tag) {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!("forbidden fail_tag present: {tag}")),
            });
        }
    }

    // Default: harness tags must be empty unless require_fail_tags set
    if spec.require_fail_tags.is_empty() {
        let harness = super::taxonomy::harness_fail_count(&ledger.result.fail_tags);
        if harness > 0 {
            return Ok(GradeOutcome {
                pass: false,
                note: Some(format!(
                    "harness fail tags: {}",
                    ledger.result.fail_tags.join(",")
                )),
            });
        }
    }

    Ok(GradeOutcome {
        pass: true,
        note: None,
    })
}

fn grade_command(spec: &GraderSpec, workspace: &Path) -> Result<GradeOutcome> {
    let cmd = spec
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("grader.command required"))?;
    let status = Command::new("bash")
        .arg("-lc")
        .arg(cmd)
        .current_dir(workspace)
        .status()?;
    Ok(GradeOutcome {
        pass: status.success(),
        note: if status.success() {
            None
        } else {
            Some(format!("command failed: {cmd}"))
        },
    })
}

fn grade_file_equals(spec: &GraderSpec, workspace: &Path) -> Result<GradeOutcome> {
    let file = spec
        .file
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("grader.file required"))?;
    let expect = spec
        .expect
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("grader.expect required"))?;
    let path = workspace.join(file);
    let got = std::fs::read_to_string(&path).unwrap_or_default();
    let pass = got.trim() == expect.trim();
    Ok(GradeOutcome {
        pass,
        note: if pass {
            None
        } else {
            Some(format!("file {file} mismatch"))
        },
    })
}
