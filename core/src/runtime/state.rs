//! Run lifecycle state machine.
//!
//! Each user request creates a [`crate::runtime::Run`] that progresses through
//! a well-defined set of states. Transitions are guarded — illegal transitions
//! return an error rather than silently corrupting state.

use serde::{Deserialize, Serialize};

/// Lifecycle state of a single Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Constructed but not yet started. Context is initialized, waiting for `Start`.
    Created,
    /// The turn loop is actively executing (Refresh → Compact → Model → Execute → Observe).
    Running,
    /// Blocked waiting for the user to approve a tool execution.
    AwaitingApproval,
    /// Blocked waiting for the user to answer a question the agent asked.
    AwaitingInput,
    /// User explicitly paused. Suspended until `Resume`.
    Paused,
    /// Finished normally — the agent produced a final answer. (terminal)
    Completed,
    /// Cancelled by the user. All resources have been reclaimed. (terminal)
    Cancelled,
    /// Unrecoverable error or max-iterations exceeded. (terminal)
    Failed,
}

impl RunState {
    /// Returns `true` for terminal states that can never transition further.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// Returns `true` if the Run is actively blocked waiting for external input.
    pub fn is_blocked(self) -> bool {
        matches!(self, Self::AwaitingApproval | Self::AwaitingInput | Self::Paused)
    }

    /// Returns `true` if the Run is alive (not terminal).
    pub fn is_alive(self) -> bool {
        !self.is_terminal()
    }
}

impl std::fmt::Display for RunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::AwaitingApproval => write!(f, "awaiting_approval"),
            Self::AwaitingInput => write!(f, "awaiting_input"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl Default for RunState {
    fn default() -> Self {
        Self::Created
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states() {
        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Cancelled.is_terminal());
        assert!(RunState::Failed.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(!RunState::Created.is_terminal());
    }

    #[test]
    fn blocked_states() {
        assert!(RunState::AwaitingApproval.is_blocked());
        assert!(RunState::AwaitingInput.is_blocked());
        assert!(RunState::Paused.is_blocked());
        assert!(!RunState::Running.is_blocked());
    }

    #[test]
    fn alive_states() {
        assert!(RunState::Created.is_alive());
        assert!(RunState::Running.is_alive());
        assert!(!RunState::Completed.is_alive());
        assert!(!RunState::Cancelled.is_alive());
    }
}
