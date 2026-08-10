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

fn serialize_content<S>(content: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match content {
        Some(s) => serializer.serialize_str(s),
        None => serializer.serialize_str(""),
    }
}

fn deserialize_content<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = Option::<String>::deserialize(deserializer)?;
    match s {
        Some(val) if val.is_empty() => Ok(None),
        other => Ok(other),
    }
}

/// Provider-native or compat reasoning carried with an assistant message.
///
/// - `text`: plaintext CoT for UI / Chat Completions (`<think>` / `reasoning_content`)
/// - `encrypted_content`: OpenAI Responses opaque blob — never truncate
/// - `signature`: Anthropic thinking signature — never truncate
/// - `summary`: optional short summary for UI (Codex-style)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl ReasoningState {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            Self::default()
        } else {
            Self {
                text: Some(text),
                ..Self::default()
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.as_ref().is_none_or(|s| s.is_empty())
            && self.encrypted_content.as_ref().is_none_or(|s| s.is_empty())
            && self.signature.as_ref().is_none_or(|s| s.is_empty())
            && self.summary.as_ref().is_none_or(|s| s.is_empty())
    }

    /// True when an opaque provider blob must be round-tripped unchanged.
    pub fn has_opaque_blob(&self) -> bool {
        self.encrypted_content.as_ref().is_some_and(|s| !s.is_empty())
            || self.signature.as_ref().is_some_and(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(serialize_with = "serialize_content", deserialize_with = "deserialize_content")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Model that ran the prompt that produced this message (per-message,
    /// distinct from the session-level `model_used`). Persisted so restored
    /// sessions show each message's own model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Structured reasoning for tool-loop continuity (Phase 1 plaintext + Phase 2 blobs).
    /// Not serialized into Chat Completions JSON bodies (unknown field). Session
    /// persistence embeds it under metadata `_reasoning` instead.
    #[serde(default, skip_serializing)]
    pub reasoning: Option<ReasoningState>,
    /// User-attached images for multimodal models. Not serialized into provider
    /// JSON directly — provider builders emit API-specific image parts. Session
    /// persistence embeds refs under metadata `_images`.
    #[serde(default, skip_serializing)]
    pub images: Option<Vec<ImageAttachment>>,
}

/// A persisted image attachment referenced by path on disk.
///
/// Files are stored content-addressably under
/// `~/.agverse/sessions/<session_id>/<prompt_id>/images/<sha256>.<ext>` and linked from
/// session message metadata via `url` (`agverse://sessions/.../images/...`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageAttachment {
    /// Absolute filesystem path to the image bytes.
    pub path: String,
    pub mime_type: String,
    /// Full SHA-256 hex digest of the file contents (content-addressable id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Stable prompt-scoped URI for DB / UI resume, e.g.
    /// `agverse://sessions/{session_id}/{prompt_id}/images/{sha256}.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
    /// Opaque provider reasoning blob (OpenAI encrypted_content or Anthropic signature).
    /// Carried separately from plaintext ThinkingDelta so hygiene never truncates it.
    ReasoningBlob {
        encrypted_content: Option<String>,
        signature: Option<String>,
        summary: Option<String>,
    },
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        function_name: Option<String>,
        arguments_delta: Option<String>,
    },
    Done,
    /// Usage statistics observed before the stream's terminal event.
    CacheUsage {
        prompt_cache_hit_tokens: Option<u64>,
        prompt_cache_miss_tokens: Option<u64>,
    },
    /// Stream has completed with usage statistics attached.
    /// Usage info is from the final SSE chunk.
    CompleteWithUsage {
        prompt_cache_hit_tokens: Option<u64>,
        prompt_cache_miss_tokens: Option<u64>,
    },
}

/// Cache usage statistics extracted from the API response.
#[derive(Debug, Clone, Default)]
pub struct CacheUsage {
    pub hit_tokens: u64,
    pub miss_tokens: u64,
}

impl CacheUsage {
    pub fn total(&self) -> u64 { self.hit_tokens + self.miss_tokens }
    pub fn hit_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 { 0.0 } else { self.hit_tokens as f64 / total as f64 }
    }
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
        message_id: String,
        message: Message,
    },
    MessageUpdate {
        message_id: String,
        delta: MessageDelta,
    },
    MessageEnd {
        message_id: String,
        message: Message,
    },

    // ── Tool execution ─────────────────────────────────────────────
    /// Mid-stream: model is still generating tool-call args (UI placeholder).
    ToolPreparing {
        index: usize,
        call_id: Option<String>,
        name: Option<String>,
        hint_path: Option<String>,
    },
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
        role_name: String,
        task: String,
    },
    SubagentTurnStart {
        subagent_id: String,
        turn_index: usize,
    },
    SubagentMessageUpdate {
        subagent_id: String,
        message_id: String,
        delta: MessageDelta,
    },
    SubagentToolStart {
        subagent_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    SubagentToolUpdate {
        subagent_id: String,
        tool_call_id: String,
        partial_result: String,
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
        role_name: String,
        success: bool,
        iterations_used: usize,
    },
    SubagentApprovalRequired {
        subagent_id: String,
        prompt_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        danger_level: String,
        explanation: String,
    },

    // ── Permissions ─────────────────────────────────────────────────
    ApprovalRequired {
        prompt_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        danger_level: String,
        explanation: String,
    },
    /// Emitted when an approval oneshot is resolved (Allow/Deny), including
    /// programmatic `resolve_approval` paths that bypass `RunCommand::Approve`.
    ApprovalResolved {
        prompt_id: String,
        /// Debug string of [`crate::permission::ApprovalChoice`].
        choice: String,
    },

    // ── Human clarification (ask_user) ──────────────────────────────
    InputRequested {
        prompt_id: String,
        title: Option<String>,
        questions: Vec<crate::runtime::input::ClarificationQuestion>,
    },

    // ── Errors ─────────────────────────────────────────────────────
    Error(String),

    // ── Cancellation ───────────────────────────────────────────────
    /// Emitted when the agent run was cancelled by the user (or a parent
    /// task). This is the termination contract: front-ends should treat it
    /// as the authoritative "run has stopped" signal and transition to a
    /// `stopped` state, rather than assuming success the moment an abort
    /// is *requested*.
    Aborted {
        reason: String,
    },

    // ── Workflow lifecycle (PLAN-0009) ─────────────────────────────
    WorkflowStarted {
        workflow_id: String,
        run_id: String,
    },
    WorkflowNodeStarted {
        run_id: String,
        node_id: String,
        node_type: String,
        label: String,
    },
    WorkflowNodeEnded {
        run_id: String,
        node_id: String,
        status: String,
        output: serde_json::Value,
    },
    WorkflowCompleted {
        run_id: String,
        status: String,
        output: serde_json::Value,
    },
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
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn user_with_model(content: &str, model: &str) -> Self {
        Self {
            role: Role::User,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            model: Some(model.to_string()),
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn assistant_with_tools(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn tool(tool_call_id: String, content: String, tool_name: Option<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            name: tool_name,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    pub fn with_reasoning(mut self, reasoning: ReasoningState) -> Self {
        if !reasoning.is_empty() {
            self.reasoning = Some(reasoning);
        }
        self
    }

    pub fn with_images(mut self, images: Vec<ImageAttachment>) -> Self {
        if !images.is_empty() {
            self.images = Some(images);
        }
        self
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
        if let Some(ref reasoning) = self.reasoning {
            if let Some(ref text) = reasoning.text {
                count += text.len() / 4;
            }
            // Opaque blobs are opaque to us; rough estimate from byte length.
            if let Some(ref blob) = reasoning.encrypted_content {
                count += blob.len() / 4;
            }
            if let Some(ref sig) = reasoning.signature {
                count += sig.len() / 4;
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
                message_id: "m1".to_string(),
                message: Message::user("hello"),
            },
            AgentEvent::MessageUpdate {
                message_id: "m1".to_string(),
                delta: MessageDelta::Text("chunk".to_string()),
            },
            AgentEvent::MessageUpdate {
                message_id: "m1".to_string(),
                delta: MessageDelta::Thinking("reasoning".to_string()),
            },
            AgentEvent::MessageEnd {
                message_id: "m1".to_string(),
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
            tool_name: "shell".to_string(),
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
                assert_eq!(tool_results[0].tool_name, "shell");
            }
            _ => panic!("expected TurnEnd"),
        }
    }

    #[test]
    fn test_subagent_events() {
        let events = [
            AgentEvent::SubagentStart {
                subagent_id: "researcher".to_string(),
                role_name: "test_role".to_string(),
                task: "analyze codebase".to_string(),
            },
            AgentEvent::SubagentTurnStart {
                subagent_id: "researcher".to_string(),
                turn_index: 0,
            },
            AgentEvent::SubagentMessageUpdate {
                subagent_id: "researcher".to_string(),
                message_id: "m2".to_string(),
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
                role_name: "test_role".to_string(),
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
                role_name,
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
            role_name: "test_role".to_string(),
            task: "test task".to_string(),
        })
        .unwrap();

        tx.send(AgentEvent::SubagentEnd {
            subagent_id: "test".to_string(),
            role_name: "test_role".to_string(),
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
