use agent_core::{AgentEvent, ApprovalChoice, MessageDelta};
use ratatui::text::Line;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

// ── Conversation line cache ──────────────────────────────────────────

/// Caches the rendered conversation lines so that scrolling doesn't
/// require re-parsing markdown / re-running syntect on every frame.
///
/// The cache is split into two parts:
/// - `entry_lines`: Pre-rendered lines for COMPLETED entries.
///   Only rebuilt when the number of entries changes (not during streaming).
/// - `streaming_lines`: Pre-rendered lines for the currently streaming blocks.
///   Rebuilt on every cache rebuild, but usually much smaller.
/// - `lines`: Combined view of entry_lines + streaming_lines + decorations.
pub struct CachedConversation {
    /// Pre-rendered lines for completed entries (cached across streaming updates).
    pub entry_lines: Vec<Line<'static>>,
    /// How many entries were used to build `entry_lines`.
    pub rendered_entry_count: usize,
    /// Pre-rendered lines for streaming blocks.
    pub streaming_lines: Vec<Line<'static>>,
    /// Combined lines (entry_lines + separator + streaming_lines + decorations).
    pub lines: Vec<Line<'static>>,
    /// The content version that produced this cache (incremented on mutation).
    pub version: u64,
    /// Terminal width used to produce this cache (invalidated on resize).
    pub width: u16,
    /// Total wrapped line height (computed once per rebuild).
    pub wrapped_height: usize,
    /// Cumulative wrapped row count: row_offsets[i] = total wrapped rows for lines[0..i].
    /// Used for binary-searching the visible line range during window rendering.
    pub row_offsets: Vec<usize>,
    /// Timestamp of the last cache rebuild — used for streaming throttle.
    pub last_rebuild: Option<Instant>,
}

impl CachedConversation {
    pub fn new() -> Self {
        Self {
            entry_lines: Vec::new(),
            rendered_entry_count: 0,
            streaming_lines: Vec::new(),
            lines: Vec::new(),
            version: 0,
            width: 0,
            wrapped_height: 0,
            row_offsets: Vec::new(),
            last_rebuild: None,
        }
    }
}

// ── Command mode (multi-step input) ─────────────────────────────────

#[derive(Clone)]
pub enum CommandMode {
    None,
    /// /models new — collecting provider name
    ModelNewProvider,
    /// /models new — collecting base_url
    ModelNewBaseUrl {
        provider: String,
    },
    /// /models new — collecting api_key
    ModelNewApiKey {
        provider: String,
        base_url: String,
    },
    /// /models new — collecting model_name
    ModelNewModelName {
        provider: String,
        base_url: String,
        api_key: String,
    },
}

impl CommandMode {
    pub fn prompt(&self) -> &str {
        match self {
            CommandMode::None => "",
            CommandMode::ModelNewProvider => "Provider name (e.g. ollama, openai):",
            CommandMode::ModelNewBaseUrl { .. } => "Base URL:",
            CommandMode::ModelNewApiKey { .. } => "API Key:",
            CommandMode::ModelNewModelName { .. } => "Model name (e.g. qwen2.5:7b):",
        }
    }
}

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
            "/quit".into(), "/exit".into(),
            "/help".into(), "/status".into(),
            "/models".into(), "/models new".into(), "/model".into(),
            "/clear".into(), "/new".into(), "/clear-queues".into(),
            "/memory".into(), "/memory search".into(), "/memory stats".into(),
            "/tokens".into(), "/temp".into(), "/max-tokens".into(),
            "/permission".into(), "/perm".into(), "/perm test".into(), "/perm mode".into(),
            "/hooks".into(),
            "/todo".into(), "/todo add".into(), "/todo start".into(),
            "/todo done".into(), "/todo clear".into(),
            "/tasks".into(), "/tasks add".into(), "/tasks start".into(),
            "/tasks done".into(), "/tasks clear".into(),
            "/skills".into(), "/skill".into(), "/skill active".into(),
            "/skill deactivate".into(), "/skill reload".into(),
            "/tool-mode".into(), "/steer".into(), "/follow-up".into(),
            "/abort".into(), "/state".into(), "/rewind".into(),
            "/sessions".into(), "/session".into(), "/session save".into(),
            "/session resume".into(), "/session delete".into(),
            "/session rename".into(), "/session archive".into(), "/session search".into(),
        ];
        Self {
            options,
            filtered_options: Vec::new(),
            selected_index: 0,
            active: false,
        }
    }
}

// ── Modal overlay (model picker, form) ──────────────────────────────

#[derive(Clone)]
pub enum ModalState {
    None,
    /// /models or /model — select from available models to switch
    ModelPicker {
        models: Vec<String>,
        selected: usize,
    },
    /// /models new — register new model form
    ModelForm {
        provider: String,
        base_url: String,
        api_key: String,
        model_id: String,
        active_field: usize,
    },
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
    /// Multi-step command input state (e.g., /models new)
    pub command_mode: CommandMode,
    /// Pending command result (e.g., "switch_model:name", "register_model:...")
    pub pending_command: Option<String>,
    pending_request: Option<String>,
    /// Direct handle to the agent's pending approvals map — used to
    /// auto-approve without going through the tokio mutex lock on Agent.
    pub pending_approvals:
        Option<Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalChoice>>>>>,
    pending_tools: HashMap<String, PendingToolCall>,
    pub autocomplete: AutocompleteState,
    pub modal: ModalState,
    /// Simple frame counter for UI animation (increments each render)
    pub frame_count: u64,
    // ── Command history ───────────────────────────────────────────
    /// Submitted user inputs (commands & messages), oldest first.
    pub input_history: Vec<String>,
    /// Current navigation index into `input_history`.
    /// `None` = not navigating (user is typing fresh input).
    /// `Some(i)` = browsing history at index i.
    pub history_index: Option<usize>,
    /// Snapshot of the user's in-progress input before they started
    /// pressing Up — restored when they press Down past the newest entry.
    pub input_snapshot: String,
    // ── Subagent detail view ─────────────────────────────────────
    /// If Some, we are showing the detail view for a specific subagent
    /// instead of the main conversation. The user pressed Enter on a
    /// subagent box and can press Esc to go back.
    pub subagent_view: Option<String>, // subagent_id
    /// Scroll position within the subagent detail view.
    pub subagent_scroll: usize,
    // ── Rendering cache ──────────────────────────────────────────
    /// Cached rendered lines — rebuilt only when content or width changes.
    pub cache: CachedConversation,
    /// Monotonically increasing content version — bumped on any entry/streaming mutation.
    pub content_version: u64,
    /// Whether the cache is stale and needs rebuilding before next draw.
    pub cache_dirty: bool,
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
            command_mode: CommandMode::None,
            pending_command: None,
            agent_running: false,
            pending_request: None,
            pending_approvals: None,
            pending_tools: HashMap::new(),
            autocomplete: AutocompleteState::new(),
            modal: ModalState::None,
            frame_count: 0,
            input_history: Vec::new(),
            history_index: None,
            input_snapshot: String::new(),
            subagent_view: None,
            subagent_scroll: 0,
            cache: CachedConversation::new(),
            content_version: 0,
            cache_dirty: true,
        }
    }

    pub fn update_autocomplete(&mut self) {
        if self.input.starts_with('/') {
            self.autocomplete.active = true;
            let filter_text = &self.input;
            self.autocomplete.filtered_options = self
                .autocomplete
                .options
                .iter()
                .filter(|opt| opt.starts_with(filter_text))
                .cloned()
                .collect();

            if self.autocomplete.filtered_options.is_empty() {
                self.autocomplete.active = false;
            } else if self.autocomplete.selected_index >= self.autocomplete.filtered_options.len() {
                self.autocomplete.selected_index =
                    self.autocomplete.filtered_options.len().saturating_sub(1);
            }
        } else {
            self.autocomplete.active = false;
        }
    }

    /// Mark the conversation cache as needing a rebuild.
    pub fn mark_dirty(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.cache_dirty = true;
    }

    /// Push the submitted text into input history (deduped, newest last).
    pub fn push_input_history(&mut self, text: String) {
        // Don't push duplicates of the very last entry
        if self.input_history.last() == Some(&text) {
            return;
        }
        self.input_history.push(text);
        // Cap history at 500 entries
        if self.input_history.len() > 500 {
            self.input_history.remove(0);
        }
        // Reset navigation
        self.history_index = None;
        self.input_snapshot.clear();
    }

    /// Navigate one step back in command history (Up arrow).
    /// Returns the text to show in the input box.
    pub fn history_up(&mut self) -> &str {
        if self.input_history.is_empty() {
            return &self.input;
        }
        match self.history_index {
            None => {
                // Save current input before entering history
                self.input_snapshot = self.input.clone();
                let idx = self.input_history.len() - 1;
                self.history_index = Some(idx);
            }
            Some(idx) if idx > 0 => {
                self.history_index = Some(idx - 1);
            }
            Some(_) => {
                // Already at oldest entry — stay
            }
        }
        let idx = self.history_index.unwrap();
        self.input = self.input_history[idx].clone();
        self.cursor_pos = self.input.len();
        &self.input
    }

    /// Navigate one step forward in command history (Down arrow).
    /// Returns the text to show in the input box.
    pub fn history_down(&mut self) -> &str {
        match self.history_index {
            None => {
                // Not navigating — nothing to do
                return &self.input;
            }
            Some(idx) if idx + 1 < self.input_history.len() => {
                self.history_index = Some(idx + 1);
            }
            Some(_) => {
                // Past the newest entry — restore snapshot
                self.history_index = None;
                self.input = self.input_snapshot.clone();
                self.cursor_pos = self.input.len();
                return &self.input;
            }
        }
        let idx = self.history_index.unwrap();
        self.input = self.input_history[idx].clone();
        self.cursor_pos = self.input.len();
        &self.input
    }

    /// Find a subagent block by ID and return a reference to it.
    pub fn find_subagent(&self, id: &str) -> Option<&SubagentState> {
        if let Some(ref streaming) = self.streaming {
            for b in &streaming.blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        return Some(sa);
                    }
                }
            }
        }
        for entry in &self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                for b in blocks {
                    if let TurnBlock::Subagent(sa) = b {
                        if sa.id == id {
                            return Some(sa);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn take_pending_request(&mut self) -> Option<String> {
        self.pending_request.take()
    }

    /// Submit a user message for the agent to process.
    /// Note: input history is recorded in the Enter handler in input.rs,
    /// so we don't double-push here.
    pub fn submit(&mut self, text: String) {
        if self.agent_running {
            return;
        }
        self.entries.push(Entry::User { text: text.clone() });
        self.pending_request = Some(text);
        self.agent_running = true;
        self.agent_state = "streaming".into();
        self.mark_dirty();
    }

    // ── Command handling ───────────────────────────────────────────

    /// Process a slash command or advance a multi-step command mode.
    /// Returns Some(text) to display as a Notice in chat, or None.
    pub fn handle_command(&mut self, input: &str) -> Option<String> {
        let input = input.trim();

        // If in multi-step mode, advance the flow
        if !matches!(self.command_mode, CommandMode::None) {
            return self.advance_command_step(input);
        }

        // Dispatch slash commands
        if input == "/quit" || input == "/exit" {
            self.should_quit = true;
            return None;
        }
        if input == "/help" {
            return Some(COMMAND_HELP.to_string());
        }
        if input == "/models" {
            self.pending_command = Some("list_models".to_string());
            return None;
        }
        if input.starts_with("/model ") {
            let name = input.strip_prefix("/model ").unwrap().trim().to_string();
            self.pending_command = Some(format!("switch_model:{}", name));
            return None;
        }
        if input == "/models new" {
            self.command_mode = CommandMode::ModelNewProvider;
            return None;
        }
        if input == "/clear" || input == "/new" {
            self.pending_command = Some("clear".to_string());
            return None;
        }
        let known = matches!(input,
        "/quit" | "/exit" | "/help" | "/models" | "/model" | "/models new" |
        "/clear" | "/new" | "/clear-queues" | "/abort" | "/state" |
        "/memory" | "/memory search" | "/memory stats" |
        "/tokens" | "/temp" | "/max-tokens" | "/tool-mode" |
        "/permission" | "/perm" | "/perm test" | "/perm mode" | "/hooks" |
        "/todo" | "/todo add" | "/todo start" | "/todo done" | "/todo clear" |
        "/tasks" | "/tasks add" | "/tasks start" | "/tasks done" | "/tasks clear" |
        "/skills" | "/skill" | "/skill active" | "/skill deactivate" | "/skill reload" |
        "/steer" | "/follow-up" | "/rewind" | "/status" |
        "/sessions" | "/session" | "/session save" | "/session resume" |
        "/session delete" | "/session rename" | "/session archive" | "/session search"
    );
    if input.starts_with('/') && !known {
        return Some(format!("Unknown command: {}. Type /help for available commands.", input));
    }
    if input.starts_with('/') {
        return Some(format!("Command '{}' not yet implemented in TUI.", input));
    }

        // Not a command — treat as user message
        self.submit(input.to_string());
        None
    }

    /// Advance through /models new multi-step flow.
    fn advance_command_step(&mut self, input: &str) -> Option<String> {
        let input = input.trim().to_string();
        if input.is_empty() {
            return None; // don't advance on empty input
        }

        let next = match &self.command_mode {
            CommandMode::ModelNewProvider => CommandMode::ModelNewBaseUrl { provider: input },
            CommandMode::ModelNewBaseUrl { provider } => CommandMode::ModelNewApiKey {
                provider: provider.clone(),
                base_url: input,
            },
            CommandMode::ModelNewApiKey { provider, base_url } => CommandMode::ModelNewModelName {
                provider: provider.clone(),
                base_url: base_url.clone(),
                api_key: input,
            },
            CommandMode::ModelNewModelName {
                provider,
                base_url,
                api_key,
            } => {
                let cmd = format!(
                    "register_model:{}|{}|{}|{}",
                    provider, base_url, api_key, input
                );
                self.command_mode = CommandMode::None;
                self.pending_command = Some(cmd);
                return None;
            }
            CommandMode::None => return None,
        };

        self.command_mode = next;
        None
    }

    /// Take the pending command for processing by mod.rs (which has agent access).
    pub fn take_pending_command(&mut self) -> Option<String> {
        self.pending_command.take()
    }

    pub fn cancel_command(&mut self) {
        self.command_mode = CommandMode::None;
        self.modal = ModalState::None;
    }

    pub fn open_model_picker(&mut self, models: Vec<String>) {
        self.modal = ModalState::ModelPicker { models, selected: 0 };
    }

    pub fn open_model_form(&mut self) {
        self.modal = ModalState::ModelForm {
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model_id: String::new(),
            active_field: 0,
        };
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
                self.mark_dirty();
            }

            AgentEvent::TurnStart { turn_index } => {
                self.flush_streaming();
                self.streaming = Some(Streaming {
                    turn: turn_index,
                    blocks: Vec::new(),
                });
                self.mark_dirty();
            }
            AgentEvent::TurnEnd { .. } => {
                self.flush_streaming();
                self.mark_dirty();
            }

            AgentEvent::MessageStart { .. } => {}
            AgentEvent::MessageUpdate { delta } => {
                match delta {
                    MessageDelta::Thinking(t) => {
                        // Transition from "working" to "thinking"
                        if self.agent_state == "streaming" {
                            self.agent_state = "thinking".into();
                        }
                        self.push_stream_block(TurnBlock::Thought(String::new()));
                        if let Some(s) = &mut self.streaming {
                            if let Some(TurnBlock::Thought(text)) = s.blocks.last_mut() {
                                text.push_str(&t);
                            }
                        }
                        self.mark_dirty();
                    }
                    MessageDelta::Text(t) => {
                        // Transition from "working"/"thinking" to "responding"
                        if self.agent_state == "streaming" || self.agent_state == "thinking" {
                            self.agent_state = "responding".into();
                        }
                        self.push_stream_block(TurnBlock::Response(String::new()));
                        if let Some(s) = &mut self.streaming {
                            if let Some(TurnBlock::Response(text)) = s.blocks.last_mut() {
                                text.push_str(&t);
                            }
                        }
                        self.mark_dirty();
                    }
                }
            }
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
                // Transition to "running tools" state
                if self.agent_running && !matches!(self.agent_state.as_str(), "idle") {
                    self.agent_state = "running tools".into();
                }
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
                self.mark_dirty();
            }

            AgentEvent::Error(e) => {
                self.push_stream_block(TurnBlock::Error(e));
                self.mark_dirty();
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
                self.mark_dirty();
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
                self.mark_dirty();
            }
            AgentEvent::SubagentTurnStart { .. } => {}
            AgentEvent::SubagentMessageUpdate { subagent_id, delta } => {
                // Find the subagent block and append to its children
                self.append_subagent_child(&subagent_id, &delta);
                self.mark_dirty();
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
                self.mark_dirty();
            }
            AgentEvent::SubagentToolEnd {
                subagent_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                self.update_subagent_tool_result(&subagent_id, &tool_name, result, is_error);
                self.mark_dirty();
            }
            AgentEvent::SubagentEnd {
                subagent_id,
                success,
                iterations_used,
            } => {
                self.finalize_subagent(&subagent_id, success, iterations_used);
                self.mark_dirty();
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
        // Note: mark_dirty is called by the callers that trigger mutations,
        // not here, because some callers (like MessageUpdate) already call it.
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

// ── TUI Command definitions ─────────────────────────────────────────

pub const COMMAND_HELP: &str = r#"Available commands:
  /help              Show this help
  /quit, /exit       Exit the TUI
  /status            Show agent status
  /models            List / switch models (modal)
  /models new        Register a new model (form)
  /model <name>      Switch to a model
  /clear, /new       Clear conversation
  /clear-queues      Clear steering queues
  /abort             Abort current agent run
  /state             Show agent state
  /memory            Memory: list / search / stats
  /tokens            Show token count
  /temp <val>        Set temperature
  /max-tokens <val>  Set max output tokens
  /tool-mode <mode>  parallel / sequential
  /permission, /perm Permission controls
  /hooks             List registered hooks
  /todo              Todo list management
  /tasks             Task board management
  /skills, /skill    Skill management
  /steer <msg>       Steer agent direction
  /follow-up <msg>   Queue follow-up message
  /rewind <idx>      Rewind to earlier turn
  /sessions, /session Session management"#;
