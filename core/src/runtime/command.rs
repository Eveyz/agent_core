//! Commands sent *to* a Run (from frontend / CLI / RunManager).
//!
//! A Run owns an `mpsc::Receiver<RunCommand>`. External callers push commands
//! through the corresponding `mpsc::Sender`. The Run's loop polls this channel
//! at turn boundaries and during blocking waits (approval / input / pause).

use crate::permission::ApprovalChoice;
use crate::types::Message;
use serde::{Deserialize, Serialize};


/// A queued steering message with identity metadata.
///
/// Wraps a user [`Message`] with a frontend-supplied `id` (so the UI can
/// track individual steer messages across queue → inject → cancel lifecycle)
/// and the raw text for event payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerEntry {
    /// Frontend-supplied unique id for this steer message.
    pub id: String,
    /// The message to inject into context at the next turn boundary.
    pub message: Message,
    /// Original text (for event payloads / display).
    pub raw_text: String,
    /// Unix epoch milliseconds when queued.
    pub timestamp: u64,
}

/// Prefix wrapped around mid-run steer text before it is added to model context.
/// UI / events keep the raw user text; only the LLM sees this framing.
pub const STEER_MID_RUN_PREFIX: &str = "\
[USER STEER MID-RUN]
The user injected a mid-run follow-up. Adjust your approach for the CURRENT step. \
Do NOT call todo_write to replan unless the user explicitly asks to change the plan. \
Address the steer in your next tool actions.

";

impl SteerEntry {
    /// Convenience constructor from raw text. Generates a random id.
    pub fn from_text(text: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            message: RunCommand::steer_message(text),
            raw_text: text.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

/// A command that drives a Run's state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunCommand {
    /// Begin execution. Only valid in `Created` state.
    Start,
    /// Suspend the run at the next turn boundary. Valid in `Running`.
    Pause,
    /// Resume a paused run. Valid in `Paused`.
    Resume,
    /// Cancel the run immediately. Valid in any non-terminal state.
    Cancel,
    /// Inject a message into the context mid-run (does not change state).
    /// The message is processed at the next turn boundary.
    /// `steer_id` is a frontend-supplied unique id used to track the steer
    /// message across its lifecycle (queued → injected / cancelled).
    Steer { steer_id: String, message: String },
    /// Cancel a pending steer message by its id. If the message has already
    /// been injected (no longer in the queue) this is a silent no-op.
    CancelSteer { steer_id: String },
    /// Resolve a pending approval request. Valid in `AwaitingApproval`.
    Approve {
        prompt_id: String,
        choice: ApprovalChoice,
    },
    /// Answer a pending input request. Valid in `AwaitingInput`.
    Answer { prompt_id: String, answer: String },

    /// Set the agent mode on the Brain. Takes effect on the NEXT Run.
    /// Existing Runs keep their mode.
    SetMode { mode: String },

    /// Queue a follow-up message that will be processed after the Run finishes,
    /// mimicking the user sending a follow-up input.
    FollowUp { message: String },
    /// Clear all queued steer and follow-up messages.
    ClearQueues,
}

impl RunCommand {
    /// Convert a steer command into a framed `Message::user` for model context.
    ///
    /// The framing tells the model this is a mid-run follow-up that should take
    /// priority when it conflicts with the previous plan. Callers that display
    /// the message to the user should keep the raw text separately.
    pub fn steer_message(message: &str) -> Message {
        Message::user(&format!("{STEER_MID_RUN_PREFIX}{message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steer_message_is_user_role_with_mid_run_prefix() {
        let msg = RunCommand::steer_message("use a different approach");
        assert_eq!(msg.role, crate::types::Role::User);
        let content = msg.content.as_deref().unwrap();
        assert!(content.starts_with("[USER STEER MID-RUN]"));
        assert!(content.ends_with("use a different approach"));
    }

    #[test]
    fn steer_entry_from_text_keeps_raw_and_wraps_message() {
        let entry = SteerEntry::from_text("阿根廷怎么样");
        assert_eq!(entry.raw_text, "阿根廷怎么样");
        let content = entry.message.content.as_deref().unwrap();
        assert!(content.contains("[USER STEER MID-RUN]"));
        assert!(content.ends_with("阿根廷怎么样"));
    }

    #[test]
    fn command_serialization() {
        let cmd = RunCommand::Pause;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"pause\""));

        let cmd2 = RunCommand::Steer {
            steer_id: "s1".into(),
            message: "hello".into(),
        };
        let json2 = serde_json::to_string(&cmd2).unwrap();
        assert!(json2.contains("\"type\":\"steer\""));
        assert!(json2.contains("hello"));

        let cmd3 = RunCommand::FollowUp {
            message: "next step".into(),
        };
        let json3 = serde_json::to_string(&cmd3).unwrap();
        assert!(json3.contains("\"type\":\"follow_up\""));

        let cmd4 = RunCommand::ClearQueues;
        let json4 = serde_json::to_string(&cmd4).unwrap();
        assert!(json4.contains("\"type\":\"clear_queues\""));
    }
}
