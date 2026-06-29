//! Events emitted *by* a Run (to frontend / CLI / event log).
//!
//! Events are append-only: they form the Run's execution trace. They are
//! broadcast via a `broadcast::Sender<RunEvent>` so multiple subscribers (UI,
//! event log, reflector) can consume independently.

// CompressionResult is not serializable; we store its summary as String
use crate::permission::ApprovalChoice;
use crate::runtime::state::RunState;
use crate::types::{Message, MessageDelta};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A unique identifier for a Run.
pub type RunId = String;

/// A unique identifier for a supervised child process.
pub type ChildId = String;

/// Events emitted during a Run's lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    // ── Lifecycle ──────────────────────────────────────────────────
    RunCreated {
        id: RunId,
        session_id: Option<String>,
    },
    RunStarted,
    RunPaused,
    RunResumed,
    RunCompleted {
        final_text: String,
    },
    RunCancelled {
        reason: String,
    },
    RunFailed {
        error: String,
    },

    // ── State transitions ──────────────────────────────────────────
    StateChanged {
        from: RunState,
        to: RunState,
    },

    // ── Turn ───────────────────────────────────────────────────────
    TurnStarted {
        index: usize,
    },
    TurnEnded {
        index: usize,
    },

    // ── Model ──────────────────────────────────────────────────────
    ModelCallStarted,
    ModelStreaming {
        subagent_id: Option<String>,
        message_id: String,
        delta: MessageDelta,
    },
    ModelCallEnded {
        text: String,
        tool_count: usize,
    },

    // ── Messages ───────────────────────────────────────────────────
    MessageStart {
        message_id: String,
        message: Message,
    },
    MessageUpdate {
        subagent_id: Option<String>,
        message_id: String,
        delta: MessageDelta,
    },
    MessageEnd {
        message_id: String,
        message: Message,
    },

    // ── Tool execution ─────────────────────────────────────────────
    ToolStarted {
        subagent_id: Option<String>,
        call_id: String,
        name: String,
        args: Value,
    },
    ToolUpdate {
        subagent_id: Option<String>,
        call_id: String,
        partial: String,
    },
    ToolEnded {
        subagent_id: Option<String>,
        call_id: String,
        name: String,
        result: String,
        is_error: bool,
    },

    // ── Interaction points ─────────────────────────────────────────
    ApprovalRequired {
        subagent_id: Option<String>,
        prompt_id: String,
        tool_name: String,
        tool_input: Value,
        danger_level: String,
        explanation: String,
    },
    ApprovalResolved {
        prompt_id: String,
        choice: ApprovalChoice,
    },
    InputRequested {
        prompt_id: String,
        question: String,
    },

    // ── Context ────────────────────────────────────────────────────
    ContextCompacted {
        summary: String,
    },
    Error {
        message: String,
    },

    // ── Subagent ───────────────────────────────────────────────────
    SubagentStarted {
        subagent_id: String,
        role_name: String,
        task: String,
    },
    SubagentEnded {
        subagent_id: String,
        success: bool,
        iterations_used: usize,
    },

    // ── Process supervision ────────────────────────────────────────
    ProcessSpawned {
        child_id: ChildId,
        label: String,
    },
    ProcessKilled {
        child_id: ChildId,
        reason: String,
    },

    // ── Planning ──────────────────────────────────────────────────
    TodoUpdated {
        items: Vec<TodoItemPayload>,
    },

    // ── Cache telemetry ────────────────────────────────────────────
    /// Per-turn cache hit/miss statistics from the model API response.
    CacheInfo {
        hit_tokens: u64,
        miss_tokens: u64,
        hit_rate: f64,
    },

    // ── Mode changes ───────────────────────────────────────────────
    /// Emitted when the agent mode changes (Brain-level, affects next Run).
    /// The frontend can use this to update its mode indicator without
    /// creating a new Run.
    ModeChanged {
        mode: String,
    },
}

/// A lightweight todo item sent to the frontend via `TodoUpdated`.
/// Strips internal fields (depends_on, timestamps) the UI doesn't need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItemPayload {
    pub id: String,
    pub description: String,
    pub status: String,
}

/// A stamped envelope wrapping a [`RunEvent`] with identity + ordering.
///
/// Every event emitted by a Run (and the `RunCreated` event emitted by the
/// [`RunManager`]) is wrapped in an `Envelope` before it crosses the
/// broadcast channel. The `seq` counter is monotonic per Run — shared
/// between the manager and the Run via an `Arc<AtomicU64>` — so every event
/// has a stable, gap-detectable id.
///
/// `event_id` is a UUID stable across transport, log, and replay.
///
/// Serialization flattens the [`RunEvent`] so the on-the-wire JSON is
/// `{ "seq": 0, "event_id": "...", "run_id": "...", "event": "run_started", ... }`
/// — the `RunEvent` tag and fields sit alongside the envelope fields. This
/// keeps the existing frontend `raw.event` string check working while
/// exposing `seq` / `event_id` / `run_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Monotonic per-Run sequence number (the global event id).
    pub seq: u64,
    /// Stable UUID for this event across transport / log / replay.
    pub event_id: String,
    /// The Run this event belongs to.
    pub run_id: RunId,
    /// The turn this event belongs to (R7). `None` for lifecycle events that
    /// are not scoped to a turn (e.g. RunCreated, RunStarted, RunCompleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The wrapper tool call that spawned a subagent (R5). `None` for main-agent
    /// events; set on subagent events so the UI can link them to the wrapper
    /// tool block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_call_id: Option<String>,
    #[serde(default = "chrono::Utc::now")]
    pub ts: chrono::DateTime<chrono::Utc>,
    /// The actual event payload.
    #[serde(flatten)]
    pub event: RunEvent,
}

/// Convert a legacy `AgentEvent` into a `RunEvent`.
///
/// This bridge lets us migrate incrementally: existing tool/subagent code
/// still emits `AgentEvent`, and the Run translates before broadcasting.
impl RunEvent {
    pub fn from_agent_event(run_id: &str, ev: crate::types::AgentEvent) -> Option<Self> {
        use crate::types::AgentEvent;
        Some(match ev {
            AgentEvent::AgentStart => RunEvent::RunStarted,
            AgentEvent::AgentEnd { .. } => return None, // handled by Run directly
            AgentEvent::Aborted { reason } => RunEvent::RunCancelled { reason },
            AgentEvent::TurnStart { turn_index } => RunEvent::TurnStarted { index: turn_index },
            AgentEvent::TurnEnd { turn_index, .. } => RunEvent::TurnEnded { index: turn_index },
            AgentEvent::MessageStart {
                message_id,
                message,
            } => RunEvent::MessageStart {
                message_id,
                message,
            },
            AgentEvent::MessageUpdate { message_id, delta } => RunEvent::MessageUpdate {
                subagent_id: None,
                message_id,
                delta,
            },
            AgentEvent::MessageEnd {
                message_id,
                message,
            } => RunEvent::MessageEnd {
                message_id,
                message,
            },
            // ModelCall events don't exist in AgentEvent — Run emits them directly
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => RunEvent::ToolStarted {
                subagent_id: None,
                call_id: tool_call_id,
                name: tool_name,
                args,
            },
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => RunEvent::ToolUpdate {
                subagent_id: None,
                call_id: tool_call_id,
                partial: partial_result,
            },
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => RunEvent::ToolEnded {
                subagent_id: None,
                call_id: tool_call_id,
                name: tool_name,
                result,
                is_error,
            },
            AgentEvent::ApprovalRequired {
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            } => RunEvent::ApprovalRequired {
                subagent_id: None,
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            },
            AgentEvent::Error(message) => RunEvent::Error { message },
            AgentEvent::SubagentStart {
                subagent_id,
                role_name,
                task,
            } => RunEvent::SubagentStarted {
                subagent_id,
                role_name,
                task,
            },
            AgentEvent::SubagentEnd {
                subagent_id,
                success,
                iterations_used,
                ..
            } => RunEvent::SubagentEnded {
                subagent_id,
                success,
                iterations_used,
            },
            // Subagent streaming events map to generic streaming events.
            AgentEvent::SubagentMessageUpdate {
                subagent_id,
                message_id,
                delta,
            } => RunEvent::ModelStreaming {
                subagent_id: Some(subagent_id),
                message_id,
                delta,
            },
            AgentEvent::SubagentToolStart {
                subagent_id,
                tool_call_id,
                tool_name,
                args,
            } => RunEvent::ToolStarted {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                name: tool_name,
                args,
            },
            AgentEvent::SubagentToolUpdate {
                subagent_id,
                tool_call_id,
                partial_result,
            } => RunEvent::ToolUpdate {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                partial: partial_result,
            },
            AgentEvent::SubagentToolEnd {
                subagent_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => RunEvent::ToolEnded {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                name: tool_name,
                result,
                is_error,
            },
            AgentEvent::SubagentApprovalRequired {
                subagent_id,
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            } => RunEvent::ApprovalRequired {
                subagent_id: Some(subagent_id),
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            },
            AgentEvent::SubagentTurnStart { .. } => return None,
            // ContextCompacted doesn't exist in AgentEvent — Run emits it directly
        })
    }

    /// Convert a `RunEvent` back into a legacy `AgentEvent`.
    /// Used by the CLI TUI which still consumes `AgentEvent` but is backed
    /// by the Runtime (Brain/Run) engine.
    pub fn to_agent_event(&self) -> Option<crate::types::AgentEvent> {
        use crate::types::AgentEvent;
        let ev = match &self {
            RunEvent::RunStarted => AgentEvent::AgentStart,
            RunEvent::RunCancelled { reason } => AgentEvent::Aborted { reason: reason.clone() },
            RunEvent::TurnStarted { index } => AgentEvent::TurnStart { turn_index: *index },
            RunEvent::TurnEnded { index } => AgentEvent::TurnEnd {
                turn_index: *index,
                assistant_message: crate::types::Message::assistant(""),
                tool_results: vec![],
            },
            RunEvent::MessageStart { message_id, message } => AgentEvent::MessageStart {
                message_id: message_id.clone(),
                message: message.clone(),
            },
            RunEvent::MessageUpdate { message_id, delta, .. } | RunEvent::ModelStreaming { message_id, delta, .. } => {
                AgentEvent::MessageUpdate {
                    message_id: message_id.clone(),
                    delta: delta.clone(),
                }
            }
            RunEvent::MessageEnd { message_id, message } => AgentEvent::MessageEnd {
                message_id: message_id.clone(),
                message: message.clone(),
            },
            RunEvent::ToolStarted { call_id, name, args, .. } => AgentEvent::ToolExecutionStart {
                tool_call_id: call_id.clone(),
                tool_name: name.clone(),
                args: args.clone(),
            },
            RunEvent::ToolUpdate { call_id, partial, .. } => AgentEvent::ToolExecutionUpdate {
                tool_call_id: call_id.clone(),
                tool_name: String::new(),
                partial_result: partial.clone(),
            },
            RunEvent::ToolEnded { call_id, name, result, is_error, .. } => {
                AgentEvent::ToolExecutionEnd {
                    tool_call_id: call_id.clone(),
                    tool_name: name.clone(),
                    result: result.clone(),
                    is_error: *is_error,
                }
            }
            RunEvent::ApprovalRequired { prompt_id, tool_name, tool_input, danger_level, explanation, .. } => {
                AgentEvent::ApprovalRequired {
                    prompt_id: prompt_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_input: tool_input.clone(),
                    danger_level: danger_level.clone(),
                    explanation: explanation.clone(),
                }
            }
            RunEvent::Error { message } => AgentEvent::Error(message.clone()),
            RunEvent::SubagentStarted { subagent_id, role_name, task } => {
                AgentEvent::SubagentStart {
                    subagent_id: subagent_id.clone(),
                    role_name: role_name.clone(),
                    task: task.clone(),
                }
            }
            RunEvent::SubagentEnded { subagent_id, success, iterations_used } => {
                AgentEvent::SubagentEnd {
                    subagent_id: subagent_id.clone(),
                    role_name: String::new(),
                    success: *success,
                    iterations_used: *iterations_used,
                }
            }
            RunEvent::RunCompleted { .. } => AgentEvent::AgentEnd { messages: vec![] },
            RunEvent::RunFailed { error } => AgentEvent::Error(error.clone()),
            _ => return None,
        };
        Some(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentEvent;

    #[test]
    fn from_agent_event_start() {
        let ev = RunEvent::from_agent_event("r1", AgentEvent::AgentStart);
        assert!(matches!(ev, Some(RunEvent::RunStarted)));
    }

    #[test]
    fn from_agent_event_turn() {
        let ev = RunEvent::from_agent_event("r1", AgentEvent::TurnStart { turn_index: 3 });
        assert!(matches!(ev, Some(RunEvent::TurnStarted { index: 3 })));
    }

    #[test]
    fn from_agent_event_aborted() {
        let ev = RunEvent::from_agent_event("r1", AgentEvent::Aborted { reason: "x".into() });
        assert!(matches!(ev, Some(RunEvent::RunCancelled { .. })));
    }

    #[test]
    fn from_agent_event_none_for_agent_end() {
        let ev = RunEvent::from_agent_event("r1", AgentEvent::AgentEnd { messages: vec![] });
        assert!(ev.is_none());
    }

    #[test]
    fn envelope_flattens_event_tag_alongside_envelope_fields() {
        // The envelope must serialize so that the RunEvent tag (`event`) and
        // the envelope fields (`seq`, `event_id`, `run_id`) all sit at the top
        // level. This is what keeps the frontend's `raw.event` string check
        // working while exposing the new identity fields.
        let env = Envelope {
            seq: 7,
            event_id: "evt-1".to_string(),
            run_id: "run-1".to_string(),
            turn_id: None,
            parent_call_id: None,
            ts: chrono::Utc::now(),
            ts: chrono::Utc::now(),
            event: RunEvent::RunStarted,
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["event_id"], "evt-1");
        assert_eq!(json["run_id"], "run-1");
        assert_eq!(json["event"], "run_started");
    }

    #[test]
    fn envelope_flattens_variant_fields() {
        use crate::types::MessageDelta;
        let env = Envelope {
            seq: 1,
            event_id: "e".to_string(),
            run_id: "r".to_string(),
            turn_id: Some("turn-1".to_string()),
            parent_call_id: None,
            ts: chrono::Utc::now(),
            ts: chrono::Utc::now(),
            event: RunEvent::ModelStreaming {
                subagent_id: Some("sa-1".to_string()),
                message_id: "m-1".to_string(),
                delta: MessageDelta::Text("hi".to_string()),
            },
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["event"], "model_streaming");
        assert_eq!(json["seq"], 1);
        assert_eq!(json["subagent_id"], "sa-1");
        assert_eq!(json["delta"]["Text"], "hi");
        assert_eq!(json["turn_id"], "turn-1");
    }
}
