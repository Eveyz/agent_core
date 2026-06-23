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
    RunCreated { id: RunId, session_id: Option<String> },
    RunStarted,
    RunPaused,
    RunResumed,
    RunCompleted { final_text: String },
    RunCancelled { reason: String },
    RunFailed { error: String },

    // ── State transitions ──────────────────────────────────────────
    StateChanged { from: RunState, to: RunState },

    // ── Turn ───────────────────────────────────────────────────────
    TurnStarted { index: usize },
    TurnEnded { index: usize },

    // ── Model ──────────────────────────────────────────────────────
    ModelCallStarted,
    ModelStreaming { subagent_id: Option<String>, delta: MessageDelta },
    ModelCallEnded { text: String, tool_count: usize },

    // ── Messages ───────────────────────────────────────────────────
    MessageStart { message: Message },
    MessageUpdate { subagent_id: Option<String>, delta: MessageDelta },
    MessageEnd { message: Message },

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
    ContextCompacted { summary: String },
    Error { message: String },

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
    ProcessSpawned { child_id: ChildId, label: String },
    ProcessKilled { child_id: ChildId, reason: String },
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
            AgentEvent::MessageStart { message } => RunEvent::MessageStart { message },
            AgentEvent::MessageUpdate { delta } => RunEvent::MessageUpdate { subagent_id: None, delta },
            AgentEvent::MessageEnd { message } => RunEvent::MessageEnd { message },
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
            AgentEvent::SubagentMessageUpdate { subagent_id, delta } => RunEvent::ModelStreaming { subagent_id: Some(subagent_id), delta },
            AgentEvent::SubagentToolStart { subagent_id, tool_call_id, tool_name, args } => RunEvent::ToolStarted {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                name: tool_name,
                args,
            },
            AgentEvent::SubagentToolUpdate { subagent_id, tool_call_id, partial_result } => RunEvent::ToolUpdate {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                partial: partial_result,
            },
            AgentEvent::SubagentToolEnd { subagent_id, tool_call_id, tool_name, result, is_error } => RunEvent::ToolEnded {
                subagent_id: Some(subagent_id),
                call_id: tool_call_id,
                name: tool_name,
                result,
                is_error,
            },
            AgentEvent::SubagentApprovalRequired { subagent_id, prompt_id, tool_name, tool_input, danger_level, explanation } => RunEvent::ApprovalRequired {
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
        let ev = RunEvent::from_agent_event(
            "r1",
            AgentEvent::AgentEnd { messages: vec![] },
        );
        assert!(ev.is_none());
    }
}
