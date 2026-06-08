use agent_core::{AgentEvent, ApprovalChoice, MessageDelta};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// ── Event pump ──────────────────────────────────────────────────────

/// Bridges the async agent (tokio) to the synchronous TUI event loop.
pub struct EventPump {
    rx: mpsc::UnboundedReceiver<AgentEvent>,
    tx: mpsc::UnboundedSender<AgentEvent>,
}

impl EventPump {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { rx, tx }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<AgentEvent> {
        self.tx.clone()
    }

    /// Drain all pending agent events into state (non-blocking).
    /// Returns true if any events were processed.
    pub fn drain(&mut self, state: &mut AppState) -> bool {
        let mut any = false;
        while let Ok(event) = self.rx.try_recv() {
            state.handle_agent_event(event);
            any = true;
        }
        any
    }
}

// ── App state ───────────────────────────────────────────────────────

pub struct AutocompleteState {
    pub options: Vec<String>,
    pub filtered_options: Vec<String>,
    pub selected_index: usize,
    pub active: bool,
}

impl AutocompleteState {
    pub fn new() -> Self {
        let options = vec![
            "/quit".into(), "/exit".into(), "/help".into(), "/models".into(),
            "/clear".into(), "/memory".into(), "/tokens".into(), "/permission".into(),
            "/perm".into(), "/hooks".into(), "/todo".into(), "/tasks".into(),
            "/skills".into(), "/status".into(), "/abort".into(), "/state".into(),
            "/tool-mode".into(), "/clear-queues".into(), "/model".into(),
            "/temp".into(), "/max-tokens".into(), "/steer".into(), "/follow-up".into(),
            "/skill".into()
        ];
        Self {
            options,
            filtered_options: Vec::new(),
            selected_index: 0,
            active: false,
        }
    }
}

pub struct AppState {
    pub entries: Vec<Entry>,
    pub streaming: Option<Streaming>,
    pub scroll: usize,
    pub input: String,
    pub cursor_pos: usize,
    pub model: String,
    pub tokens: usize,
    pub agent_state: String,
    pub tool_mode: String,
    pub focus_index: Option<usize>,
    pub should_quit: bool,
    pub agent_running: bool,
    pending_request: Option<String>,
    /// Direct handle to the agent's pending approvals map — used to
    /// auto-approve without going through the tokio mutex lock on Agent.
    pub pending_approvals:
        Option<Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalChoice>>>>>,
    pending_tools: HashMap<String, PendingToolCall>,
    pub autocomplete: AutocompleteState,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            streaming: None,
            scroll: 0,
            input: String::new(),
            cursor_pos: 0,
            model: String::from("?"),
            tokens: 0,
            agent_state: String::from("idle"),
            tool_mode: String::from("parallel"),
            focus_index: None,
            should_quit: false,
            agent_running: false,
            pending_request: None,
            pending_approvals: None,
            pending_tools: HashMap::new(),
            autocomplete: AutocompleteState::new(),
        }
    }

    pub fn update_autocomplete(&mut self) {
        if self.input.starts_with('/') {
            self.autocomplete.active = true;
            let filter_text = &self.input;
            self.autocomplete.filtered_options = self.autocomplete.options
                .iter()
                .filter(|opt| opt.starts_with(filter_text))
                .cloned()
                .collect();
            
            if self.autocomplete.filtered_options.is_empty() {
                self.autocomplete.active = false;
            } else if self.autocomplete.selected_index >= self.autocomplete.filtered_options.len() {
                self.autocomplete.selected_index = self.autocomplete.filtered_options.len().saturating_sub(1);
            }
        } else {
            self.autocomplete.active = false;
        }
    }

    pub fn take_pending_request(&mut self) -> Option<String> {
        self.pending_request.take()
    }

    /// Submit a user message for the agent to process.
    pub fn submit(&mut self, text: String) {
        if self.agent_running {
            return;
        }
        self.entries.push(Entry::User { text: text.clone() });
        self.pending_request = Some(text);
        self.agent_running = true;
        self.agent_state = "streaming".into();
    }

    // ── Agent event handling ──────────────────────────────────────

    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {}
            AgentEvent::AgentEnd { .. } => {
                self.flush_streaming();
                self.agent_running = false;
                self.agent_state = "idle".into();
                self.pending_tools.clear();
            }

            AgentEvent::TurnStart { turn_index } => {
                self.flush_streaming();
                self.streaming = Some(Streaming {
                    turn: turn_index,
                    blocks: Vec::new(),
                });
            }
            AgentEvent::TurnEnd { .. } => {
                self.flush_streaming();
            }

            AgentEvent::MessageStart { .. } => {}
            AgentEvent::MessageUpdate { delta } => match delta {
                MessageDelta::Thinking(t) => {
                    self.push_stream_block(TurnBlock::Thought(String::new()));
                    if let Some(s) = &mut self.streaming {
                        if let Some(TurnBlock::Thought(text)) = s.blocks.last_mut() {
                            text.push_str(&t);
                        }
                    }
                }
                MessageDelta::Text(t) => {
                    self.push_stream_block(TurnBlock::Response(String::new()));
                    if let Some(s) = &mut self.streaming {
                        if let Some(TurnBlock::Response(text)) = s.blocks.last_mut() {
                            text.push_str(&t);
                        }
                    }
                }
            },
            AgentEvent::MessageEnd { message } => {
                if let Some(ref tool_calls) = message.tool_calls {
                    for tc in tool_calls {
                        self.pending_tools.insert(
                            tc.id.clone(),
                            PendingToolCall {
                                name: tc.function.name.clone(),
                                args: tc.function.arguments.clone(),
                            },
                        );
                    }
                }
            }

            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.pending_tools
                    .entry(tool_call_id)
                    .or_insert_with(|| PendingToolCall {
                        name: tool_name,
                        args: args.to_string(),
                    });
            }
            AgentEvent::ToolExecutionUpdate { .. } => {
                // Updates are intermediate — ToolExecutionEnd provides the final result
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let pending = self.pending_tools.remove(&tool_call_id);
                self.push_stream_block(TurnBlock::Tool {
                    name: pending
                        .as_ref()
                        .map_or_else(|| tool_name.clone(), |tool| tool.name.clone()),
                    args: pending.map_or_else(String::new, |tool| tool.args),
                    result: Some(ToolResult {
                        text: result,
                        is_error,
                    }),
                });
            }

            AgentEvent::Error(e) => {
                self.push_stream_block(TurnBlock::Error(e));
            }

            AgentEvent::ApprovalRequired {
                prompt_id,
                tool_name,
                explanation,
                ..
            } => {
                // Respond directly via the approvals Arc — avoids deadlock
                // with the tokio mutex on Agent (held by the spawned task).
                if let Some(ref approvals) = self.pending_approvals {
                    if let Ok(mut map) = approvals.lock() {
                        if let Some(tx) = map.remove(&prompt_id) {
                            let _ = tx.send(ApprovalChoice::AllowSession);
                        }
                    }
                }
                self.push_stream_block(TurnBlock::Notice(format!(
                    "[APPROVAL] {} — {} (auto-approved)",
                    tool_name, explanation
                )));
            }

            // ── Subagent events ──────────────────────────────────
            AgentEvent::SubagentStart { subagent_id, task } => {
                let block = TurnBlock::Subagent(SubagentState {
                    id: subagent_id,
                    task,
                    collapsed: true,
                    success: false,
                    focused: false,
                    children: Vec::new(),
                    done: false,
                    iterations: 0,
                });
                if let Some(s) = &mut self.streaming {
                    s.blocks.push(block);
                } else {
                    self.entries.push(Entry::Turn {
                        turn: 0,
                        blocks: vec![block],
                    });
                }
            }
            AgentEvent::SubagentTurnStart { .. } => {}
            AgentEvent::SubagentMessageUpdate { subagent_id, delta } => {
                // Find the subagent block and append to its children
                self.append_subagent_child(&subagent_id, &delta);
            }
            AgentEvent::SubagentToolStart {
                subagent_id,
                tool_name,
                args,
                ..
            } => {
                self.append_subagent_child_block(
                    &subagent_id,
                    TurnBlock::Tool {
                        name: tool_name,
                        args: args.to_string(),
                        result: None,
                    },
                );
            }
            AgentEvent::SubagentToolEnd {
                subagent_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                self.update_subagent_tool_result(&subagent_id, &tool_name, result, is_error);
            }
            AgentEvent::SubagentEnd {
                subagent_id,
                success,
                iterations_used,
            } => {
                self.finalize_subagent(&subagent_id, success, iterations_used);
            }
        }
    }

    // ── Streaming helpers ─────────────────────────────────────────

    fn push_stream_block(&mut self, block: TurnBlock) {
        if let Some(s) = &mut self.streaming {
            // Merge consecutive blocks of the same type
            match (&block, s.blocks.last()) {
                (TurnBlock::Thought(_), Some(TurnBlock::Thought(_))) => return, // already pushed
                (TurnBlock::Response(_), Some(TurnBlock::Response(_))) => return,
                _ => {}
            }
            s.blocks.push(block);
        } else {
            self.entries.push(Entry::Turn {
                turn: 0,
                blocks: vec![block],
            });
        }
    }

    fn flush_streaming(&mut self) {
        if let Some(s) = self.streaming.take() {
            if !s.blocks.is_empty() {
                self.entries.push(Entry::Turn {
                    turn: s.turn,
                    blocks: s.blocks,
                });
            }
        }
    }

    fn append_subagent_child(&mut self, id: &str, delta: &MessageDelta) {
        let block = match delta {
            MessageDelta::Thinking(t) => TurnBlock::Thought(t.clone()),
            MessageDelta::Text(t) => TurnBlock::Response(t.clone()),
        };
        self.append_subagent_child_block(id, block);
    }

    fn append_subagent_child_block(&mut self, id: &str, block: TurnBlock) {
        // Search streaming first, then entries
        if let Some(s) = &mut self.streaming {
            for b in &mut s.blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        sa.children.push(block);
                        return;
                    }
                }
            }
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                for b in blocks {
                    if let TurnBlock::Subagent(sa) = b {
                        if sa.id == id {
                            sa.children.push(block);
                            return;
                        }
                    }
                }
            }
        }
    }

    fn update_subagent_tool_result(
        &mut self,
        id: &str,
        tool_name: &str,
        result: String,
        is_error: bool,
    ) {
        let updater = |blocks: &mut Vec<TurnBlock>| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        for child in sa.children.iter_mut().rev() {
                            if let TurnBlock::Tool {
                                name, result: r, ..
                            } = child
                            {
                                if *name == tool_name && r.is_none() {
                                    *r = Some(ToolResult {
                                        text: result.clone(),
                                        is_error,
                                    });
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        };
        if let Some(s) = &mut self.streaming {
            updater(&mut s.blocks);
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                updater(blocks);
            }
        }
    }

    fn finalize_subagent(&mut self, id: &str, success: bool, iterations: usize) {
        let finalizer = |blocks: &mut Vec<TurnBlock>| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        sa.done = true;
                        sa.success = success;
                        sa.iterations = iterations;
                        return;
                    }
                }
            }
        };
        if let Some(s) = &mut self.streaming {
            finalizer(&mut s.blocks);
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                finalizer(blocks);
            }
        }
    }
}

// ── Conversation types ──────────────────────────────────────────────

#[derive(Clone)]
#[allow(dead_code)]
pub enum Entry {
    System { text: String },
    User { text: String },
    Turn { turn: usize, blocks: Vec<TurnBlock> },
}

#[derive(Clone)]
pub enum TurnBlock {
    Thought(String),
    Response(String),
    Tool {
        name: String,
        args: String,
        result: Option<ToolResult>,
    },
    Subagent(SubagentState),
    /// System messages (approval prompts, notifications, etc.)
    Notice(String),
    Error(String),
}

#[derive(Clone)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone)]
pub struct SubagentState {
    pub id: String,
    pub task: String,
    pub collapsed: bool,
    pub focused: bool,
    pub children: Vec<TurnBlock>,
    pub done: bool,
    pub success: bool,
    pub iterations: usize,
}

#[derive(Clone)]
pub struct Streaming {
    pub turn: usize,
    pub blocks: Vec<TurnBlock>,
}

struct PendingToolCall {
    name: String,
    args: String,
}
