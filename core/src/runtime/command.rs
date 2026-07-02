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

impl SteerEntry {
    /// Convenience constructor from raw text. Generates a random id.
    pub fn from_text(text: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            message: Message::user(text),
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
}

impl RunCommand {
    /// Convert a steer command into a `Message::user`.
    pub fn steer_message(message: &str) -> Message {
        Message::user(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steer_message_is_user_role() {
        let msg = RunCommand::steer_message("use a different approach");
        assert_eq!(msg.role, crate::types::Role::User);
        assert_eq!(msg.content.as_deref(), Some("use a different approach"));
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
    }
}
