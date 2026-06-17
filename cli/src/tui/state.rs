use agent_core::{AgentEvent, ApprovalChoice, MessageDelta};
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

// ── Conversation block cache ─────────────────────────────────────────

/// A single renderable block in the conversation.
/// Each block knows its own wrapped height and how to render itself.
#[derive(Clone)]
pub struct CachedBlock {
    pub kind: BlockKind,
    pub wrapped_height: usize,
    pub subagent_id: Option<String>,
    pub lines: Vec<Line<'static>>,
}

impl CachedBlock {
    pub fn spacing() -> Self {
        Self {
            kind: BlockKind::Spacing,
            wrapped_height: 1,
            subagent_id: None,
            lines: vec![Line::raw("")],
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum BlockKind {
    Spacing,
    User(String),
    Thought(String),
    Response(String),
    Tool {
        name: String,
        args: String,
        result: Option<ToolResult>,
    },
    Subagent(SubagentState),
    Notice(String),
    Error(String),
    System(String),
    Working,
}

/// Caches the conversation as a list of blocks so that scrolling can
/// be done at block granularity. Each block is rendered independently,
/// allowing different background colours without manual space padding.
pub struct CachedConversation {
    /// Pre-rendered blocks for completed entries (cached across streaming updates).
    pub entry_blocks: Vec<CachedBlock>,
    /// How many entries were used to build `entry_blocks`.
    pub rendered_entry_count: usize,
    /// Pre-rendered blocks for the currently streaming turn.
    pub streaming_blocks: Vec<CachedBlock>,
    /// Combined blocks (entry_blocks + separator + streaming_blocks + decorations).
    pub blocks: Vec<CachedBlock>,
    /// The content version that produced this cache (incremented on mutation).
    pub version: u64,
    /// Terminal width used to produce this cache (invalidated on resize).
    pub width: u16,
    /// Total wrapped line height (sum of all block wrapped_height).
    pub wrapped_height: usize,
    /// Timestamp of the last cache rebuild — used for streaming throttle.
    pub last_rebuild: Option<Instant>,
}

impl CachedConversation {
    pub fn new() -> Self {
        Self {
            entry_blocks: Vec::new(),
            rendered_entry_count: 0,
            streaming_blocks: Vec::new(),
            blocks: Vec::new(),
            version: 0,
            width: 0,
            wrapped_height: 0,
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
            "/quit".into(),
            "/help".into(),
            "/models".into(), "/models new".into(), "/model".into(),
            "/clear".into(), "/clear-queues".into(),
            "/memory".into(), "/memory search".into(), "/memory stats".into(),
            "/tokens".into(), "/temp".into(), "/max-tokens".into(),
            "/perm".into(), "/perm test".into(), "/perm mode".into(),
            "/hooks".into(),
            "/todo".into(), "/todo add".into(), "/todo start".into(),
            "/todo done".into(), "/todo clear".into(),
            "/skills".into(), "/skill".into(), "/skill active".into(),
            "/skill deactivate".into(), "/skill reload".into(),
            "/tool-mode".into(), "/steer".into(), "/follow-up".into(),
            "/abort".into(), "/status".into(), "/rewind".into(),
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
    // ── Mouse hover ─────────────────────────────────────────────
    /// ID of the subagent the mouse is currently hovering over.
    pub hovered_subagent: Option<String>,
    // ── Rendering cache ──────────────────────────────────────────
    /// Cached rendered lines — rebuilt only when content or width changes.
    pub cache: CachedConversation,
    /// Monotonically increasing content version — bumped on any entry/streaming mutation.
    pub content_version: u64,
    /// Whether the cache is stale and needs rebuilding before next draw.
    pub cache_dirty: bool,
    pub force_cache_rebuild: bool,
    /// Cloned from Agent — allows abort without locking the agent mutex.
    pub cancel_token: Option<CancellationToken>,
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
            should_quit: false,
            command_mode: CommandMode::None,
            pending_command: None,
            agent_running: false,
            pending_request: None,
            pending_approvals: None,
            autocomplete: AutocompleteState::new(),
            modal: ModalState::None,
            frame_count: 0,
            input_history: Vec::new(),
            history_index: None,
            input_snapshot: String::new(),
            subagent_view: None,
            subagent_scroll: 0,
            hovered_subagent: None,
            cache: CachedConversation::new(),
            content_version: 0,
            cache_dirty: true,
            force_cache_rebuild: false,
            cancel_token: None,
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

    pub fn mark_dirty_force(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.cache_dirty = true;
        self.force_cache_rebuild = true;
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
        "/clear" | "/new" | "/clear-queues" | "/abort" | "/status" |
        "/memory" | "/memory search" | "/memory stats" |
        "/tokens" | "/temp" | "/max-tokens" | "/tool-mode" |
        "/perm" | "/perm test" | "/perm mode" | "/hooks" |
        "/todo" | "/todo add" | "/todo start" | "/todo done" | "/todo clear" |
        "/skills" | "/skill" | "/skill active" | "/skill deactivate" | "/skill reload" |
        "/steer" | "/follow-up" | "/rewind" |
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
            AgentEvent::MessageEnd { .. } => {}

            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                // Transition to "running tools" state
                if self.agent_running && !matches!(self.agent_state.as_str(), "idle") {
                    self.agent_state = "running tools".into();
                }
                // Push the tool block immediately so the user sees it running
                self.push_stream_block(TurnBlock::Tool {
                    tool_call_id,
                    name: tool_name,
                    args: args.to_string(),
                    result: None,
                });
                self.mark_dirty_force();
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
                // Find the existing Tool block and update its result in-place.
                // Search streaming first, then entries — the block could have
                // been flushed between Start and End.
                let mut found = false;
                let do_update = |blocks: &mut Vec<TurnBlock>| {
                    for b in blocks.iter_mut().rev() {
                        if let TurnBlock::Tool { tool_call_id: id, result: r, .. } = b {
                            if *id == tool_call_id && r.is_none() {
                                *r = Some(ToolResult {
                                    text: result.clone(),
                                    is_error,
                                });
                                return true;
                            }
                        }
                    }
                    false
                };
                if let Some(s) = &mut self.streaming {
                    found = do_update(&mut s.blocks);
                }
                if !found {
                    for entry in &mut self.entries {
                        if let Entry::Turn { blocks, .. } = entry {
                            if do_update(blocks) {
                                found = true;
                                break;
                            }
                        }
                    }
                }
                if !found {
                    // Fallback: no matching start block found — push a new one
                    self.push_stream_block(TurnBlock::Tool {
                        tool_call_id: tool_call_id.clone(),
                        name: tool_name.clone(),
                        args: String::new(),
                        result: Some(ToolResult {
                            text: result.clone(),
                            is_error,
                        }),
                    });
                }
                self.mark_dirty_force();
            }

            AgentEvent::Error(e) => {
                self.push_stream_block(TurnBlock::Error(e));
                self.mark_dirty();
            }

            AgentEvent::ApprovalRequired { prompt_id, .. } | AgentEvent::SubagentApprovalRequired { prompt_id, .. } => {
                // Auto-approve silently for now. In the future this will
                // pause and ask the user via a modal.
                if let Some(ref approvals) = self.pending_approvals {
                    if let Ok(mut map) = approvals.lock() {
                        if let Some(tx) = map.remove(&prompt_id) {
                            let _ = tx.send(ApprovalChoice::AllowSession);
                        }
                    }
                }
            }

            // ── Subagent events ──────────────────────────────────
            AgentEvent::SubagentStart { subagent_id, role_name, task } => {
                let already_exists = self.find_subagent(&subagent_id).is_some();
                if already_exists {
                    return;
                }
                let block = TurnBlock::Subagent(SubagentState {
                    id: subagent_id,
                    role_name,
                    task,
                    children: Vec::new(),
                    done: false,
                    success: false,
                    iterations: 0,
                    turn_index: 0,
                    current_activity: String::new(),
                    started_at: Some(std::time::Instant::now()),
                    elapsed_ms: 0,
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
            AgentEvent::SubagentTurnStart { subagent_id, turn_index } => {
                self.update_subagent_activity(&subagent_id, Some(turn_index), "💭 Thinking...".to_string());
                self.mark_dirty_force();
            }
            AgentEvent::SubagentMessageUpdate { subagent_id, delta } => {
                match &delta {
                    MessageDelta::Thinking(_) => {
                        self.update_subagent_activity(&subagent_id, None, "💭 Thinking...".to_string());
                        self.mark_dirty_force();
                    }
                    MessageDelta::Text(_) => {}
                }
                self.append_subagent_child(&subagent_id, &delta);
                self.mark_dirty();
            }
            AgentEvent::SubagentToolStart {
                subagent_id,
                tool_call_id,
                tool_name,
                args,
                ..
            } => {
                let detail = tool_detail(&tool_name, &args);
                let activity = if detail.is_empty() {
                    format!("🔧 {}", tool_name)
                } else {
                    format!("🔧 {} → {}", tool_name, detail)
                };
                self.update_subagent_activity(&subagent_id, None, activity);
                self.mark_dirty_force();
                self.append_subagent_child_block(
                    &subagent_id,
                    TurnBlock::Tool {
                        tool_call_id,
                        name: tool_name,
                        args: args.to_string(),
                        result: None,
                    },
                );
                self.mark_dirty();
            }
            AgentEvent::SubagentToolEnd {
                subagent_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                let detail = tool_detail(&tool_name, &serde_json::from_str(&result).unwrap_or_default());
                let icon = if is_error { "✗" } else { "✓" };
                let activity = if detail.is_empty() {
                    format!("🔧 {} {}", icon, tool_name)
                } else {
                    format!("🔧 {} {} → {}", icon, tool_name, detail)
                };
                self.update_subagent_activity(&subagent_id, None, activity);
                self.mark_dirty_force();
                self.update_subagent_tool_result(&subagent_id, &tool_call_id, result, is_error);
                self.mark_dirty();
            }
            AgentEvent::SubagentEnd {
                subagent_id,
                role_name: _,
                success,
                iterations_used,
            } => {
                self.finalize_subagent(&subagent_id, success, iterations_used);
                self.mark_dirty_force();
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
        let updater = |blocks: &mut Vec<TurnBlock>, block: &TurnBlock| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        // Merge consecutive blocks of the same type so that
                        // streaming deltas don't create a new block per token.
                        match (block, sa.children.last_mut()) {
                            (TurnBlock::Thought(new_text), Some(TurnBlock::Thought(existing))) => {
                                existing.push_str(new_text);
                                return;
                            }
                            (TurnBlock::Response(new_text), Some(TurnBlock::Response(existing))) => {
                                existing.push_str(new_text);
                                return;
                            }
                            _ => {}
                        }
                        sa.children.push(block.clone());
                        return;
                    }
                }
            }
        };
        if let Some(s) = &mut self.streaming {
            updater(&mut s.blocks, &block);
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                updater(blocks, &block);
            }
        }
    }

    fn update_subagent_tool_result(
        &mut self,
        id: &str,
        tool_call_id: &str,
        result: String,
        is_error: bool,
    ) {
        let updater = |blocks: &mut Vec<TurnBlock>| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        for child in sa.children.iter_mut().rev() {
                            if let TurnBlock::Tool {
                                tool_call_id: cid, result: r, ..
                            } = child
                            {
                                if *cid == tool_call_id && r.is_none() {
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
        let label = if success { "complete" } else { "incomplete" };
        let finalizer = |blocks: &mut Vec<TurnBlock>| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        let elapsed = sa.started_at
                            .map(|t| t.elapsed().as_millis() as u64)
                            .unwrap_or(0);
                        sa.done = true;
                        sa.success = success;
                        sa.iterations = iterations;
                        sa.elapsed_ms = elapsed;
                        sa.current_activity = format_elapsed_activity(label, iterations, elapsed);
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

    fn update_subagent_activity(&mut self, id: &str, turn_index: Option<usize>, activity: String) {
        let updater = |blocks: &mut Vec<TurnBlock>| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        sa.current_activity = activity.clone();
                        if let Some(ti) = turn_index {
                            sa.turn_index = ti;
                        }
                        return;
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

    // ── MPSC Reducer ────────────────────────────────────────────────────

    /// Apply a single event to the state. Returns `true` if the UI needs a redraw.
    pub fn apply(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Key(key) => {
                let _ = crate::tui::input::handle_key(key, self);
                true
            }
            AppEvent::Mouse(mouse, main_area) => self.apply_mouse(mouse, main_area),
            AppEvent::Resize => {
                self.cache_dirty = true;
                true
            }
            AppEvent::Agent(ev) => {
                self.handle_agent_event(ev);
                true
            }
            AppEvent::Tick => {
                self.frame_count = self.frame_count.wrapping_add(1);
                true
            }
        }
    }

    fn apply_mouse(&mut self, mouse: MouseEvent, main_area: Rect) -> bool {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.subagent_view.is_some() {
                    self.subagent_scroll = self.subagent_scroll.saturating_add(3);
                } else {
                    self.scroll = self.scroll.saturating_add(3);
                }
                true
            }
            MouseEventKind::ScrollDown => {
                if self.subagent_view.is_some() {
                    self.subagent_scroll = self.subagent_scroll.saturating_sub(3);
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                }
                true
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if self.subagent_view.is_none() {
                    let new_hovered = self.find_hovered_subagent(mouse.row, main_area);
                    if new_hovered != self.hovered_subagent {
                        self.hovered_subagent = new_hovered;
                        self.cache_dirty = true;
                    }
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.subagent_view.is_none() {
                    if let Some(sa_id) = self.find_hovered_subagent(mouse.row, main_area) {
                        self.subagent_view = Some(sa_id);
                        self.subagent_scroll = 0;
                        self.hovered_subagent = None;
                        self.mark_dirty();
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Map a mouse row to a subagent ID if over a subagent block.
    fn find_hovered_subagent(&self, row: u16, main_area: Rect) -> Option<String> {
        if row < main_area.y || row >= main_area.y + main_area.height || main_area.height == 0 {
            return None;
        }
        let rel_y = (row - main_area.y) as usize;
        let visible_height = main_area.height as usize;
        let max_scroll = self.cache.wrapped_height.saturating_sub(visible_height);
        let scroll_from_top = max_scroll.saturating_sub(self.scroll);
        let abs_y = scroll_from_top + rel_y;

        let mut cumulative = 0;
        for block in &self.cache.blocks {
            let bottom = cumulative + block.wrapped_height;
            if abs_y >= cumulative && abs_y < bottom {
                return block.subagent_id.clone();
            }
            cumulative = bottom;
        }
        None
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
        tool_call_id: String,
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
#[allow(dead_code)]
pub struct SubagentState {
    pub id: String,
    pub role_name: String,
    pub task: String,
    pub children: Vec<TurnBlock>,
    pub done: bool,
    pub success: bool,
    pub iterations: usize,
    pub current_activity: String,
    pub started_at: Option<std::time::Instant>,
    pub elapsed_ms: u64,
    pub turn_index: usize,
}

#[derive(Clone)]
pub struct Streaming {
    pub turn: usize,
    pub blocks: Vec<TurnBlock>,
}

fn truncate_activity(s: &str, max_chars: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.len() <= max_chars {
        cleaned
    } else {
        let truncated: String = cleaned.chars().take(max_chars).collect();
        format!("{}…", truncated)
    }
}

fn tool_detail(tool_name: &str, args: &serde_json::Value) -> String {
    let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
    let first_non_empty = |a: &str, b: &str| -> String {
        if a.is_empty() { b.to_string() } else { a.to_string() }
    };
    match tool_name {
        "webfetch" => s(&args["url"]),
        "run_command" | "bash" => {
            let cmd = first_non_empty(&s(&args["command"]), &s(&args["script"]));
            truncate_activity(&cmd, 50)
        }
        "read_file" => first_non_empty(&s(&args["path"]), &s(&args["file_path"])),
        "write_file" => first_non_empty(&s(&args["path"]), &s(&args["file_path"])),
        "edit" => first_non_empty(&s(&args["file_path"]), &s(&args["path"])),
        "glob" => s(&args["pattern"]),
        "grep" => {
            let pat = s(&args["pattern"]);
            let path = s(&args["path"]);
            if path.is_empty() { pat } else { format!("{} in {}", pat, path) }
        }
        "subagent_spawn" | "subagent_spawn_all" => String::new(),
        _ => String::new(),
    }
}

fn format_elapsed_activity(label: &str, iterations: usize, elapsed_ms: u64) -> String {
    let time = if elapsed_ms >= 60_000 {
        format!("{:.1}m", elapsed_ms as f64 / 60_000.0)
    } else if elapsed_ms >= 1_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        format!("{}ms", elapsed_ms)
    };
    if label == "complete" {
        format!("✓ complete ({} iter) {}", iterations, time)
    } else {
        format!("✗ incomplete ({} iter) {}", iterations, time)
    }
}

// ── TUI Command definitions ─────────────────────────────────────────

pub const COMMAND_HELP: &str = r#"Available commands:
  /help          Show this help
  /quit          Exit the TUI
  /status        Show agent status
  /models        List / switch models
  /models new    Register a new model
  /model <name>  Switch to a model
  /clear         Clear conversation
  /clear-queues  Clear steering queues
  /abort         Abort current agent run
  /memory        Memory: list/search/stats
  /tokens        Show token count
  /temp <val>    Set temperature
  /max-tokens    Set max output tokens
  /tool-mode     parallel / sequential
  /perm          Permission controls
  /hooks         List registered hooks
  /todo          Todo list management
  /skills        Skill management
  /steer <msg>   Steer agent direction
  /follow-up     Queue follow-up message
  /rewind <idx>  Rewind to earlier turn
  /sessions      Session management"#;

// ── MPSC Event Loop ─────────────────────────────────────────────────

/// Unified event type for the TUI event loop.
/// All state mutations are driven by sending these through a channel.
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent, Rect),
    Resize,
    Agent(AgentEvent),
    Tick,
}
