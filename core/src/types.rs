use serde::{Deserialize, Serialize};
use std::fmt;

/// Channel sender for tools to emit `AgentEvent`s back to the parent agent.
pub type EventSender = tokio::sync::mpsc::UnboundedSender<AgentEvent>;
/// Channel receiver for the parent agent to consume tool-emitted events.
pub type EventReceiver = tokio::sync::mpsc::UnboundedReceiver<AgentEvent>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        function_name: Option<String>,
        arguments_delta: Option<String>,
    },
    Done,
}

// ── Tool execution mode ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

// ── Rich Agent Events ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    // ── Agent lifecycle ─────────────────────────────────────────────
    AgentStart,
    AgentEnd {
        messages: Vec<Message>,
    },

    // ── Turn lifecycle ──────────────────────────────────────────────
    TurnStart {
        turn_index: usize,
    },
    TurnEnd {
        turn_index: usize,
        assistant_message: Message,
        tool_results: Vec<ToolResultRecord>,
    },

    // ── Message lifecycle (user, assistant, toolResult) ─────────────
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        delta: MessageDelta,
    },
    MessageEnd {
        message: Message,
    },

    // ── Tool execution ─────────────────────────────────────────────
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        partial_result: String,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: String,
        is_error: bool,
    },

    // ── Subagent lifecycle ─────────────────────────────────────────
    SubagentStart {
        subagent_id: String,
        task: String,
    },
    SubagentTurnStart {
        subagent_id: String,
        turn_index: usize,
    },
    SubagentMessageUpdate {
        subagent_id: String,
        delta: MessageDelta,
    },
    SubagentToolStart {
        subagent_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    SubagentToolEnd {
        subagent_id: String,
        tool_call_id: String,
        tool_name: String,
        result: String,
        is_error: bool,
    },
    SubagentEnd {
        subagent_id: String,
        success: bool,
        iterations_used: usize,
    },

    // ── Permissions ─────────────────────────────────────────────────
    ApprovalRequired {
        prompt_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        danger_level: String,
        explanation: String,
    },

    // ── Errors ─────────────────────────────────────────────────────
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageDelta {
    Thinking(String),
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: String,
    pub is_error: bool,
}

// ── Agent runtime state ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Streaming,
    ExecutingTools,
    Aborted,
}

// ── Message constructors ────────────────────────────────────────────

impl Message {
    pub fn system(content: &str) -> Self {
        Self {
            role: Role::System,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(tool_call_id: String, content: String) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            name: None,
        }
    }

    pub fn token_count(&self) -> usize {
        let mut count = 4;
        if let Some(ref content) = self.content {
            count += content.len() / 4;
        }
        if let Some(ref tool_calls) = self.tool_calls {
            for tc in tool_calls {
                count += tc.function.name.len() / 4;
                count += tc.function.arguments.len() / 4;
                count += 10;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_event_variants() {
        let events = vec![
            AgentEvent::AgentStart,
            AgentEvent::TurnStart { turn_index: 0 },
            AgentEvent::TurnEnd {
                turn_index: 0,
                assistant_message: Message::assistant("done"),
                tool_results: vec![],
            },
            AgentEvent::MessageStart {
                message: Message::user("hello"),
            },
            AgentEvent::MessageUpdate {
                delta: MessageDelta::Text("chunk".to_string()),
            },
            AgentEvent::MessageUpdate {
                delta: MessageDelta::Thinking("reasoning".to_string()),
            },
            AgentEvent::MessageEnd {
                message: Message::assistant("response"),
            },
            AgentEvent::ToolExecutionStart {
                tool_call_id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                args: serde_json::json!({"path": "/tmp/test"}),
            },
            AgentEvent::ToolExecutionUpdate {
                tool_call_id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                partial_result: "partial...".to_string(),
            },
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                result: "file content".to_string(),
                is_error: false,
            },
            AgentEvent::Error("something went wrong".to_string()),
        ];

        assert_eq!(events.len(), 11);
    }

    #[test]
    fn test_agent_state_transitions() {
        assert_eq!(AgentState::Idle, AgentState::Idle);
        assert_ne!(AgentState::Idle, AgentState::Streaming);
        assert_ne!(AgentState::Streaming, AgentState::ExecutingTools);
        assert_ne!(AgentState::ExecutingTools, AgentState::Aborted);
    }

    #[test]
    fn test_tool_execution_mode_default() {
        assert_eq!(ToolExecutionMode::default(), ToolExecutionMode::Parallel);
    }

    #[test]
    fn test_tool_result_record() {
        let record = ToolResultRecord {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            result: "content".to_string(),
            is_error: false,
        };
        assert_eq!(record.tool_name, "read_file");
        assert!(!record.is_error);
    }

    #[test]
    fn test_message_delta_variants() {
        let thinking = MessageDelta::Thinking("reasoning".to_string());
        let text = MessageDelta::Text("output".to_string());

        match thinking {
            MessageDelta::Thinking(s) => assert_eq!(s, "reasoning"),
            _ => panic!("expected Thinking"),
        }
        match text {
            MessageDelta::Text(s) => assert_eq!(s, "output"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_agent_end_contains_messages() {
        let msgs = vec![Message::user("hi"), Message::assistant("hello")];
        let event = AgentEvent::AgentEnd {
            messages: msgs.clone(),
        };
        match event {
            AgentEvent::AgentEnd { messages } => {
                assert_eq!(messages.len(), 2);
            }
            _ => panic!("expected AgentEnd"),
        }
    }

    #[test]
    fn test_turn_end_with_tool_results() {
        let results = vec![ToolResultRecord {
            tool_call_id: "c1".to_string(),
            tool_name: "bash".to_string(),
            result: "ok".to_string(),
            is_error: false,
        }];
        let event = AgentEvent::TurnEnd {
            turn_index: 2,
            assistant_message: Message::assistant("done"),
            tool_results: results,
        };
        match event {
            AgentEvent::TurnEnd {
                turn_index,
                tool_results,
                ..
            } => {
                assert_eq!(turn_index, 2);
                assert_eq!(tool_results.len(), 1);
                assert_eq!(tool_results[0].tool_name, "bash");
            }
            _ => panic!("expected TurnEnd"),
        }
    }

    #[test]
    fn test_subagent_events() {
        let events = [
            AgentEvent::SubagentStart {
                subagent_id: "researcher".to_string(),
                task: "analyze codebase".to_string(),
            },
            AgentEvent::SubagentTurnStart {
                subagent_id: "researcher".to_string(),
                turn_index: 0,
            },
            AgentEvent::SubagentMessageUpdate {
                subagent_id: "researcher".to_string(),
                delta: MessageDelta::Text("looking at files...".to_string()),
            },
            AgentEvent::SubagentToolStart {
                subagent_id: "researcher".to_string(),
                tool_call_id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                args: serde_json::json!({"path": "Cargo.toml"}),
            },
            AgentEvent::SubagentToolEnd {
                subagent_id: "researcher".to_string(),
                tool_call_id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                result: "[package]...".to_string(),
                is_error: false,
            },
            AgentEvent::SubagentEnd {
                subagent_id: "researcher".to_string(),
                success: true,
                iterations_used: 2,
            },
        ];
        assert_eq!(events.len(), 6);

        // Verify SubagentStart structure
        match &events[0] {
            AgentEvent::SubagentStart {
                subagent_id, task, ..
            } => {
                assert_eq!(subagent_id, "researcher");
                assert_eq!(task, "analyze codebase");
            }
            _ => panic!("expected SubagentStart"),
        }

        // Verify SubagentEnd structure
        match &events[5] {
            AgentEvent::SubagentEnd {
                subagent_id,
                success,
                iterations_used,
            } => {
                assert_eq!(subagent_id, "researcher");
                assert!(success);
                assert_eq!(*iterations_used, 2);
            }
            _ => panic!("expected SubagentEnd"),
        }
    }

    #[test]
    fn test_event_sender_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        tx.send(AgentEvent::SubagentStart {
            subagent_id: "test".to_string(),
            task: "test task".to_string(),
        })
        .unwrap();

        tx.send(AgentEvent::SubagentEnd {
            subagent_id: "test".to_string(),
            success: true,
            iterations_used: 1,
        })
        .unwrap();

        drop(tx);

        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }

        assert_eq!(received.len(), 2);
    }
}
