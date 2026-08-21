//! Shared turn-loop policy for Interactive (`Run`) and Nested (`Subagent`).
//!
//! The two adapters keep different identity (FSM, mailbox, transcript). This
//! module is the policy and stream-partial types they both consume.

use crate::types::ToolExecutionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    DualTranscript,
    TrimToFit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopPolicy {
    pub compact: CompactMode,
    pub recovery: bool,
    pub tool_mode: ToolExecutionMode,
    pub ask_user: bool,
    pub steering: bool,
}

impl LoopPolicy {
    pub fn interactive() -> Self {
        Self {
            compact: CompactMode::DualTranscript,
            recovery: true,
            tool_mode: ToolExecutionMode::Parallel,
            ask_user: true,
            steering: true,
        }
    }

    pub fn nested() -> Self {
        Self {
            compact: CompactMode::TrimToFit,
            recovery: false,
            tool_mode: ToolExecutionMode::Sequential,
            ask_user: false,
            steering: false,
        }
    }
}

/// Accumulated streaming text/thinking while a model call is in flight.
#[derive(Debug, Clone, Default)]
pub struct StreamPartial {
    pub text: String,
    pub thinking: String,
    pub message_id: Option<String>,
}

impl StreamPartial {
    pub fn merge_attempt(&mut self, attempt: &StreamPartial) {
        if attempt.text.len() > self.text.len() {
            self.text.clone_from(&attempt.text);
        }
        if attempt.thinking.len() > self.thinking.len() {
            self.thinking.clone_from(&attempt.thinking);
        }
        if attempt.message_id.is_some() {
            self.message_id.clone_from(&attempt.message_id);
        }
    }

    pub fn recoverable_text(&self) -> String {
        crate::hygiene::wrap_thinking(&self.thinking, &self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_does_not_gain_interactive_ports() {
        let nested = LoopPolicy::nested();
        assert!(!nested.recovery);
        assert!(!nested.ask_user);
        assert!(!nested.steering);
        assert_eq!(nested.compact, CompactMode::TrimToFit);
        assert_eq!(nested.tool_mode, ToolExecutionMode::Sequential);
    }

    #[test]
    fn interactive_keeps_steer_and_ask_user() {
        let interactive = LoopPolicy::interactive();
        assert!(interactive.recovery);
        assert!(interactive.ask_user);
        assert!(interactive.steering);
        assert_eq!(interactive.compact, CompactMode::DualTranscript);
    }

    #[test]
    fn stream_partial_keeps_longest_attempt() {
        let mut acc = StreamPartial {
            text: "ab".into(),
            thinking: "t".into(),
            message_id: Some("1".into()),
        };
        acc.merge_attempt(&StreamPartial {
            text: "abcd".into(),
            thinking: String::new(),
            message_id: None,
        });
        assert_eq!(acc.text, "abcd");
        assert_eq!(acc.thinking, "t");
        assert_eq!(acc.message_id.as_deref(), Some("1"));
    }
}
