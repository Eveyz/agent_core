//! TUI application state — driven by [`agent_core::RunEvent`] via [`crate::state::CliState`].
//!
//! This replaces the legacy `Agent`/`AgentEvent`-based state. All agent
//! interaction goes through `RunManager` (create_run / command / subscribe),
//! and events arrive here as [`RunEvent`] wrapped in [`AppEvent::Run`].

use crate::commands::{CmdMessage, UiRequest};
use agent_core::permission::ApprovalChoice;
use agent_core::runtime::event::RunEvent;
use agent_core::types::MessageDelta;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::collections::HashSet;
use std::time::Instant;

// ── Conversation block cache ─────────────────────────────────────────

/// A single renderable block in the conversation.
#[derive(Clone)]
pub struct CachedBlock {
    pub kind: BlockKind,
    pub wrapped_height: usize,
    pub subagent_id: Option<String>,
    pub block_id: Option<u64>,
    pub lines: Vec<Line<'static>>,
}

impl CachedBlock {
    pub fn spacing() -> Self {
        Self {
            kind: BlockKind::Spacing,
            wrapped_height: 1,
            subagent_id: None,
            block_id: None,
            lines: vec![Line::raw("")],
        }
    }

    pub fn separator(label: &str) -> Self {
        Self {
            kind: BlockKind::Separator(label.to_string()),
            wrapped_height: 1,
            subagent_id: None,
            block_id: None,
            lines: vec![Line::raw("")],
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum BlockKind {
    Spacing,
    Separator(String),
    User(String),
    Thought { id: u64, text: String, expanded: bool },
    Response(String),
    Tool {
        id: u64,
        name: String,
        args: String,
        result: Option<ToolResult>,
        expanded: bool,
    },
    Subagent(SubagentState),
    Notice(String),
    Error(String),
    System(String),
    Working,
}

pub struct CachedConversation {
    pub entry_blocks: Vec<CachedBlock>,
    pub rendered_entry_count: usize,
    pub streaming_blocks: Vec<CachedBlock>,
    pub blocks: Vec<CachedBlock>,
    pub version: u64,
    pub width: u16,
    pub wrapped_height: usize,
    pub last_rebuild: Option<Instant>,
    /// Ids of Thought/Tool blocks currently visible, in display order — used
    /// for Alt+Up/Alt+Down block focus navigation.
    pub focusable_ids: Vec<u64>,
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
            focusable_ids: Vec::new(),
        }
    }
}

// ── Modal overlay ────────────────────────────────────────────────────

#[derive(Clone)]
pub enum ModalState {
    None,
    ModelPicker {
        models: Vec<String>,
        current: String,
        selected: usize,
        filter: String,
    },
    ModelForm {
        provider: String,
        base_url: String,
        api_key: String,
        model_id: String,
        active_field: usize,
        cursor: usize,
        show_api_key: bool,
    },
    /// Five-tier approval prompt: Deny / DenyPersistent / AllowOnce / AllowSession / AllowPersistent.
    Approval {
        prompt_id: String,
        subagent_id: Option<String>,
        tool_name: String,
        tool_input: serde_json::Value,
        danger_level: String,
        explanation: String,
        selected: usize,
    },
    /// Simple free-text answer to an `InputRequested` clarification.
    Answer {
        prompt_id: String,
        prompt: String,
        input: String,
        cursor: usize,
    },
    SessionList {
        sessions: Vec<(String, String)>, // (id, display line)
        selected: usize,
        filter: String,
    },
    Help,
    Pager {
        title: String,
        lines: Vec<String>,
        scroll: usize,
    },
    RewindList {
        points: Vec<(usize, String)>,
        selected: usize,
    },
    QuitConfirm,
}

/// Five approval choices in fixed UI order (matches `commands::approval_from_choice_key`).
pub const APPROVAL_CHOICES: &[(&str, &str)] = &[
    ("1", "Deny"),
    ("2", "Deny always"),
    ("3", "Allow once"),
    ("4", "Allow for session"),
    ("5", "Allow always"),
];

pub fn approval_choice_for_index(idx: usize) -> ApprovalChoice {
    crate::commands::approval_from_choice_key(APPROVAL_CHOICES[idx.min(APPROVAL_CHOICES.len() - 1)].0)
}

// ── Autocomplete ──────────────────────────────────────────────────────

pub struct AutocompleteState {
    pub filtered: Vec<(&'static str, &'static str)>,
    pub selected_index: usize,
    pub active: bool,
}

impl AutocompleteState {
    pub fn new() -> Self {
        Self {
            filtered: Vec::new(),
            selected_index: 0,
            active: false,
        }
    }

    pub fn update(&mut self, input: &str) {
        if input.starts_with('/') && !input.contains('\n') {
            self.filtered = crate::commands::ALL_COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(input) && *cmd != input)
                .copied()
                .collect();
            self.active = !self.filtered.is_empty();
            if self.selected_index >= self.filtered.len() {
                self.selected_index = self.filtered.len().saturating_sub(1);
            }
        } else {
            self.active = false;
        }
    }
}

// ── App state ───────────────────────────────────────────────────────

pub struct AppState {
    // ── Conversation ────────────────────────────────────────────────
    pub entries: Vec<Entry>,
    pub streaming: Option<Streaming>,
    pub scroll: usize,
    pub viewport_h: usize,

    // ── Input ───────────────────────────────────────────────────────
    pub input: String,
    pub cursor_pos: usize,
    pub input_scroll: usize,

    // ── Status ──────────────────────────────────────────────────────
    pub model: String,
    pub tokens: usize,
    pub max_context_tokens: usize,
    pub context_pct: f64,
    pub agent_state: String,
    pub tool_mode: String,
    pub permission_label: String,
    pub cwd_short: String,
    pub session_short: String,
    pub steer_queue_depth: usize,
    pub paused: bool,
    /// Whether the permission system is active (mode != Yolo) — passed to
    /// `commands::dispatch_*` and shown in `/status` / `/permission`.
    pub enable_permission: bool,
    /// Whether any hooks are registered.
    pub enable_hooks: bool,

    // ── Lifecycle ───────────────────────────────────────────────────
    pub should_quit: bool,
    pub quit_confirm_armed: bool,
    pub agent_running: bool,

    // ── Pending actions consumed by the async event loop (mod.rs) ──
    pub pending_command: Option<String>,
    pub pending_request: Option<String>,
    pub pending_workflow_request: Option<String>,
    pub pending_steer: Option<String>,
    /// (prompt_id, choice_key) — choice_key is resolved via
    /// `commands::approval_from_choice_key`.
    pub pending_approval: Option<(String, String)>,
    pub pending_answer: Option<(String, String)>, // (prompt_id, answer text)
    pub pending_abort: bool,
    pub pending_yank: Option<String>,
    pub last_notice: Option<String>,

    pub autocomplete: AutocompleteState,
    pub modal: ModalState,

    pub frame_count: u64,

    // ── Command history ─────────────────────────────────────────────
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub input_snapshot: String,

    // ── Subagent focus / detail view ────────────────────────────────
    pub subagent_view: Option<String>,
    pub subagent_scroll: usize,
    pub hovered_subagent: Option<String>,
    pub focused_subagent_idx: Option<usize>,

    // ── Block focus / expand ────────────────────────────────────────
    pub focused_block_id: Option<u64>,
    pub thought_expanded: bool,
    pub tool_expanded: HashSet<u64>,
    next_block_id: u64,

    // ── Rendering cache ──────────────────────────────────────────────
    pub cache: CachedConversation,
    pub content_version: u64,
    pub cache_dirty: bool,
    pub force_cache_rebuild: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            streaming: None,
            scroll: 0,
            viewport_h: 24,
            input: String::new(),
            cursor_pos: 0,
            input_scroll: 0,
            model: String::from("?"),
            tokens: 0,
            max_context_tokens: 128_000,
            context_pct: 0.0,
            agent_state: String::from("idle"),
            tool_mode: String::from("parallel"),
            permission_label: String::from("standard"),
            cwd_short: String::new(),
            session_short: String::from("(new)"),
            steer_queue_depth: 0,
            paused: false,
            enable_permission: true,
            enable_hooks: false,
            should_quit: false,
            quit_confirm_armed: false,
            agent_running: false,
            pending_command: None,
            pending_request: None,
            pending_workflow_request: None,
            pending_steer: None,
            pending_approval: None,
            pending_answer: None,
            pending_abort: false,
            pending_yank: None,
            last_notice: None,
            autocomplete: AutocompleteState::new(),
            modal: ModalState::None,
            frame_count: 0,
            input_history: Vec::new(),
            history_index: None,
            input_snapshot: String::new(),
            subagent_view: None,
            subagent_scroll: 0,
            hovered_subagent: None,
            focused_subagent_idx: None,
            focused_block_id: None,
            thought_expanded: false,
            tool_expanded: HashSet::new(),
            next_block_id: 1,
            cache: CachedConversation::new(),
            content_version: 0,
            cache_dirty: true,
            force_cache_rebuild: false,
        }
    }

    fn alloc_block_id(&mut self) -> u64 {
        let id = self.next_block_id;
        self.next_block_id += 1;
        id
    }

    pub fn is_follow_mode(&self) -> bool {
        self.scroll == 0
    }

    pub fn recompute_context_pct(&mut self) {
        if self.max_context_tokens > 0 {
            self.context_pct = (self.tokens as f64 / self.max_context_tokens as f64 * 100.0).min(999.0);
        }
    }

    pub fn update_autocomplete(&mut self) {
        self.autocomplete.update(&self.input);
    }

    pub fn mark_dirty(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.cache_dirty = true;
    }

    pub fn mark_dirty_force(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.cache_dirty = true;
        self.force_cache_rebuild = true;
    }

    pub fn push_input_history(&mut self, text: String) {
        if self.input_history.last() == Some(&text) {
            return;
        }
        self.input_history.push(text);
        if self.input_history.len() > 500 {
            self.input_history.remove(0);
        }
        self.history_index = None;
        self.input_snapshot.clear();
    }

    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.input_snapshot = self.input.clone();
                self.history_index = Some(self.input_history.len() - 1);
            }
            Some(idx) if idx > 0 => self.history_index = Some(idx - 1),
            Some(_) => {}
        }
        let idx = self.history_index.unwrap();
        self.input = self.input_history[idx].clone();
        self.cursor_pos = self.input.len();
    }

    pub fn history_down(&mut self) {
        match self.history_index {
            None => {}
            Some(idx) if idx + 1 < self.input_history.len() => {
                self.history_index = Some(idx + 1);
                self.input = self.input_history[idx + 1].clone();
                self.cursor_pos = self.input.len();
            }
            Some(_) => {
                self.history_index = None;
                self.input = self.input_snapshot.clone();
                self.cursor_pos = self.input.len();
            }
        }
    }

    /// Submit user text. If the agent is running, queue as a steer message
    /// instead of starting a new run.
    pub fn submit(&mut self, text: String) {
        if self.agent_running {
            self.pending_steer = Some(text);
            return;
        }
        self.entries.push(Entry::User { text: text.clone() });
        self.pending_request = Some(text);
        self.agent_running = true;
        self.agent_state = "streaming".into();
        self.mark_dirty();
    }

    pub fn take_pending_request(&mut self) -> Option<String> {
        self.pending_request.take()
    }
    pub fn submit_workflow(&mut self, goal: String) {
        if self.agent_running {
            self.push_notice("Finish or abort the active run before starting workflow authoring.");
            return;
        }
        let display = if goal.trim().is_empty() {
            "/workflow".to_string()
        } else {
            format!("/workflow {}", goal.trim())
        };
        self.entries.push(Entry::User { text: display });
        self.pending_workflow_request = Some(goal);
        self.agent_running = true;
        self.agent_state = "streaming".into();
        self.mark_dirty();
    }
    pub fn take_pending_workflow_request(&mut self) -> Option<String> {
        self.pending_workflow_request.take()
    }
    pub fn take_pending_steer(&mut self) -> Option<String> {
        self.pending_steer.take()
    }
    pub fn take_pending_command(&mut self) -> Option<String> {
        self.pending_command.take()
    }
    pub fn take_pending_approval(&mut self) -> Option<(String, String)> {
        self.pending_approval.take()
    }
    pub fn take_pending_answer(&mut self) -> Option<(String, String)> {
        self.pending_answer.take()
    }
    pub fn take_pending_yank(&mut self) -> Option<String> {
        self.pending_yank.take()
    }

    pub fn push_notice(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let block = TurnBlock::Notice(msg.clone());
        self.push_block_to(None, block);
        self.last_notice = Some(msg);
        self.mark_dirty();
    }

    pub fn push_error_notice(&mut self, msg: impl Into<String>) {
        let block = TurnBlock::Error(msg.into());
        self.push_block_to(None, block);
        self.mark_dirty();
    }

    /// Apply the outcome of a slash command dispatched via `commands::dispatch_*`.
    pub fn apply_command_outcome(&mut self, outcome: crate::commands::CommandOutcome) {
        use crate::commands::CommandOutcome;
        match outcome {
            CommandOutcome::Quit => self.should_quit = true,
            CommandOutcome::Handled { messages } => {
                for m in messages {
                    match m {
                        CmdMessage::Info(s) => self.push_notice(s),
                        CmdMessage::Warn(s) => self.push_notice(format!("⚠ {s}")),
                        CmdMessage::Error(s) => self.push_error_notice(s),
                    }
                }
            }
            CommandOutcome::Unknown(cmd) => {
                self.push_error_notice(format!("Unknown command: {cmd}. Type /help."));
            }
            CommandOutcome::NotSlash => {}
            CommandOutcome::NeedsUi(req) => self.apply_ui_request(req),
        }
    }

    pub fn apply_ui_request(&mut self, req: UiRequest) {
        match req {
            UiRequest::ModelPicker { models, current } => {
                self.modal = ModalState::ModelPicker {
                    models,
                    current,
                    selected: 0,
                    filter: String::new(),
                };
            }
            UiRequest::ModelForm => {
                self.modal = ModalState::ModelForm {
                    provider: String::new(),
                    base_url: String::new(),
                    api_key: String::new(),
                    model_id: String::new(),
                    active_field: 0,
                    cursor: 0,
                    show_api_key: false,
                };
            }
            UiRequest::SessionList => {
                // Populated by caller before opening — see mod.rs `open_session_list`.
            }
            UiRequest::Help => self.modal = ModalState::Help,
            UiRequest::Status => {
                // Populated by caller (mod.rs) with `commands::format_status`.
            }
            UiRequest::ShowText { title, body } => {
                let lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
                self.modal = ModalState::Pager {
                    title,
                    lines,
                    scroll: 0,
                };
            }
            UiRequest::RewindList { points } => {
                self.modal = ModalState::RewindList { points, selected: 0 };
            }
        }
    }

    pub fn open_pager(&mut self, title: impl Into<String>, body: impl Into<String>) {
        let body = body.into();
        let lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();
        self.modal = ModalState::Pager {
            title: title.into(),
            lines,
            scroll: 0,
        };
    }

    pub fn open_session_list(&mut self, sessions: Vec<(String, String)>) {
        self.modal = ModalState::SessionList {
            sessions,
            selected: 0,
            filter: String::new(),
        };
    }

    pub fn open_model_picker(&mut self, models: Vec<String>, current: String) {
        self.modal = ModalState::ModelPicker {
            models,
            current,
            selected: 0,
            filter: String::new(),
        };
    }

    pub fn open_model_form(&mut self) {
        self.modal = ModalState::ModelForm {
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model_id: String::new(),
            active_field: 0,
            cursor: 0,
            show_api_key: false,
        };
    }

    pub fn open_help(&mut self) {
        self.modal = ModalState::Help;
    }

    pub fn cancel_modal(&mut self) {
        self.modal = ModalState::None;
    }

    // ── Subagent lookup ─────────────────────────────────────────────

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

    pub fn live_subagent_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        let mut collect = |blocks: &[TurnBlock]| {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    ids.push(sa.id.clone());
                }
            }
        };
        if let Some(ref s) = self.streaming {
            collect(&s.blocks);
        }
        for entry in &self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                collect(blocks);
            }
        }
        ids
    }

    pub fn focus_next_subagent(&mut self) {
        let ids = self.live_subagent_ids();
        if ids.is_empty() {
            self.focused_subagent_idx = None;
            return;
        }
        self.focused_subagent_idx = Some(match self.focused_subagent_idx {
            Some(i) if i + 1 < ids.len() => i + 1,
            _ => 0,
        });
    }

    pub fn focus_prev_subagent(&mut self) {
        let ids = self.live_subagent_ids();
        if ids.is_empty() {
            self.focused_subagent_idx = None;
            return;
        }
        self.focused_subagent_idx = Some(match self.focused_subagent_idx {
            Some(0) | None => ids.len() - 1,
            Some(i) => i - 1,
        });
    }

    pub fn open_focused_subagent_detail(&mut self) {
        let ids = self.live_subagent_ids();
        if let Some(idx) = self.focused_subagent_idx {
            if let Some(id) = ids.get(idx) {
                self.subagent_view = Some(id.clone());
                self.subagent_scroll = 0;
                self.mark_dirty();
            }
        }
    }

    // ── Block focus navigation ───────────────────────────────────────

    pub fn focus_next_block(&mut self) {
        let ids = &self.cache.focusable_ids;
        if ids.is_empty() {
            return;
        }
        let next = match self.focused_block_id.and_then(|id| ids.iter().position(|&x| x == id)) {
            Some(i) if i + 1 < ids.len() => ids[i + 1],
            Some(_) => *ids.last().unwrap(),
            None => *ids.last().unwrap(),
        };
        self.focused_block_id = Some(next);
    }

    pub fn focus_prev_block(&mut self) {
        let ids = &self.cache.focusable_ids;
        if ids.is_empty() {
            return;
        }
        let prev = match self.focused_block_id.and_then(|id| ids.iter().position(|&x| x == id)) {
            Some(i) if i > 0 => ids[i - 1],
            Some(_) => ids[0],
            None => *ids.first().unwrap(),
        };
        self.focused_block_id = Some(prev);
    }

    pub fn toggle_focused_tool_expand(&mut self) {
        if let Some(id) = self.focused_block_id {
            if self.tool_expanded.contains(&id) {
                self.tool_expanded.remove(&id);
            } else {
                self.tool_expanded.insert(id);
            }
            self.mark_dirty_force();
        }
    }

    pub fn toggle_thought_expanded(&mut self) {
        self.thought_expanded = !self.thought_expanded;
        self.mark_dirty_force();
    }

    /// Open a pager showing the full text of the focused block, or (if none
    /// focused) the most recent tool/response text.
    pub fn open_focused_or_last_pager(&mut self) {
        let text = self.focused_block_text().or_else(|| self.last_block_text());
        match text {
            Some((title, body)) => self.open_pager(title, body),
            None => self.push_notice("Nothing to show yet."),
        }
    }

    fn focused_block_text(&self) -> Option<(String, String)> {
        let id = self.focused_block_id?;
        let find = |blocks: &[TurnBlock]| -> Option<(String, String)> {
            for b in blocks {
                match b {
                    TurnBlock::Thought { id: bid, text, .. } if *bid == id => {
                        return Some(("thought".into(), text.clone()));
                    }
                    TurnBlock::Tool { id: bid, name, result, .. } if *bid == id => {
                        let body = result
                            .as_ref()
                            .map(|r| r.text.clone())
                            .unwrap_or_else(|| "(pending)".into());
                        return Some((format!("tool: {name}"), body));
                    }
                    _ => {}
                }
            }
            None
        };
        if let Some(ref s) = self.streaming {
            if let Some(hit) = find(&s.blocks) {
                return Some(hit);
            }
        }
        for entry in &self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                if let Some(hit) = find(blocks) {
                    return Some(hit);
                }
            }
        }
        None
    }

    fn last_block_text(&self) -> Option<(String, String)> {
        if let Some(ref s) = self.streaming {
            for b in s.blocks.iter().rev() {
                match b {
                    TurnBlock::Response(t) => return Some(("response".into(), t.clone())),
                    TurnBlock::Tool { name, result, .. } => {
                        let body = result.as_ref().map(|r| r.text.clone()).unwrap_or_default();
                        return Some((format!("tool: {name}"), body));
                    }
                    _ => {}
                }
            }
        }
        for entry in self.entries.iter().rev() {
            if let Entry::Turn { blocks, .. } = entry {
                for b in blocks.iter().rev() {
                    match b {
                        TurnBlock::Response(t) => return Some(("response".into(), t.clone())),
                        _ => continue,
                    }
                }
            }
        }
        None
    }

    /// Text to copy to the clipboard for 'y' (yank): focused block, else last response.
    pub fn yank_text(&self) -> Option<String> {
        self.focused_block_text()
            .or_else(|| self.last_block_text())
            .map(|(_, body)| body)
    }

    // ── RunEvent handling ─────────────────────────────────────────────

    pub fn handle_run_event(&mut self, event: RunEvent) {
        match event {
            RunEvent::RunStarted => {
                self.agent_running = true;
                self.agent_state = "streaming".into();
                self.mark_dirty();
            }
            RunEvent::RunPaused => {
                self.paused = true;
                self.mark_dirty();
            }
            RunEvent::RunResumed => {
                self.paused = false;
                self.mark_dirty();
            }
            RunEvent::RunCompleted { .. } => {
                self.flush_streaming();
                self.agent_running = false;
                self.paused = false;
                self.steer_queue_depth = 0;
                self.agent_state = "idle".into();
                self.mark_dirty_force();
            }
            RunEvent::RunCancelled { reason } => {
                self.flush_streaming();
                self.agent_running = false;
                self.paused = false;
                self.steer_queue_depth = 0;
                self.agent_state = "idle".into();
                self.push_notice(format!("⏹ Run cancelled: {reason}"));
                self.mark_dirty_force();
            }
            RunEvent::RunFailed { error } => {
                self.flush_streaming();
                self.agent_running = false;
                self.paused = false;
                self.agent_state = "idle".into();
                self.push_error_notice(format!("Run failed: {error}"));
                self.mark_dirty_force();
            }
            RunEvent::Notice { message, severity, .. } => {
                if severity == "error" {
                    self.push_error_notice(message);
                } else {
                    self.push_notice(message);
                }
            }
            RunEvent::StateChanged { .. } => {}

            RunEvent::TurnStarted { index } => {
                self.flush_streaming();
                self.streaming = Some(Streaming {
                    turn: index,
                    blocks: Vec::new(),
                });
                self.mark_dirty();
            }
            RunEvent::TurnEnded { .. } => {
                self.flush_streaming();
                self.mark_dirty();
            }

            RunEvent::ModelCallStarted | RunEvent::ModelCallEnded { .. } => {}

            RunEvent::ModelStreaming { subagent_id, delta, .. }
            | RunEvent::MessageUpdate { subagent_id, delta, .. } => {
                self.apply_delta(subagent_id.as_deref(), delta);
            }
            RunEvent::MessageStart { .. } | RunEvent::MessageEnd { .. } => {}

            RunEvent::ToolPreparing { .. } => {}
            RunEvent::ToolStarted { subagent_id, call_id, name, args } => {
                if self.agent_running {
                    self.agent_state = "running tools".into();
                }
                let id = self.alloc_block_id();
                self.push_block_to(
                    subagent_id.as_deref(),
                    TurnBlock::Tool {
                        id,
                        name: name.clone(),
                        args: args.to_string(),
                        result: None,
                        expanded: false,
                    },
                );
                if let Some(sid) = subagent_id.as_deref() {
                    let detail = tool_detail(&name, &args);
                    let activity = if detail.is_empty() {
                        format!("🔧 {name}")
                    } else {
                        format!("🔧 {name} → {detail}")
                    };
                    self.update_subagent_activity(sid, activity);
                }
                self.mark_dirty_force();
            }
            RunEvent::ToolUpdate { .. } => {}
            RunEvent::ToolEnded { subagent_id, call_id: _, name, result, is_error } => {
                let updated = self.update_tool_result(subagent_id.as_deref(), &name, result.clone(), is_error);
                if !updated {
                    let id = self.alloc_block_id();
                    self.push_block_to(
                        subagent_id.as_deref(),
                        TurnBlock::Tool {
                            id,
                            name: name.clone(),
                            args: String::new(),
                            result: Some(ToolResult { text: result.clone(), is_error }),
                            expanded: false,
                        },
                    );
                }
                if let Some(sid) = subagent_id.as_deref() {
                    let icon = if is_error { "✗" } else { "✓" };
                    let detail = tool_detail(&name, &serde_json::from_str(&result).unwrap_or_default());
                    let activity = if detail.is_empty() {
                        format!("🔧 {icon} {name}")
                    } else {
                        format!("🔧 {icon} {name} → {detail}")
                    };
                    self.update_subagent_activity(sid, activity);
                }
                self.mark_dirty_force();
            }

            RunEvent::ApprovalRequired {
                subagent_id,
                prompt_id,
                tool_name,
                tool_input,
                danger_level,
                explanation,
            } => {
                self.modal = ModalState::Approval {
                    prompt_id,
                    subagent_id,
                    tool_name,
                    tool_input,
                    danger_level,
                    explanation,
                    selected: 0,
                };
                self.mark_dirty();
            }
            RunEvent::ApprovalResolved { .. } => {}

            RunEvent::InputRequested { prompt_id, title, question, questions } => {
                let prompt = title
                    .or(question)
                    .or_else(|| questions.first().map(|q| q.prompt.clone()))
                    .unwrap_or_else(|| "Agent needs input".to_string());
                self.modal = ModalState::Answer {
                    prompt_id,
                    prompt,
                    input: String::new(),
                    cursor: 0,
                };
                self.mark_dirty();
            }
            RunEvent::InputResolved { .. } => {}

            RunEvent::ContextCompacted { summary } => {
                self.push_notice(format!("🗜 Context compacted: {summary}"));
            }
            RunEvent::Error { message } => {
                self.push_error_notice(message);
            }

            RunEvent::SteerQueued { queue_depth, .. } => {
                self.steer_queue_depth = queue_depth;
                self.mark_dirty();
            }
            RunEvent::SteerInjected { .. } => {
                self.steer_queue_depth = self.steer_queue_depth.saturating_sub(1);
                self.mark_dirty();
            }
            RunEvent::SteerCancelled { .. } | RunEvent::SteerFailed { .. } => {
                self.steer_queue_depth = self.steer_queue_depth.saturating_sub(1);
                self.mark_dirty();
            }

            RunEvent::SubagentStarted { subagent_id, role_name, task } => {
                if self.find_subagent(&subagent_id).is_some() {
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
                    current_activity: String::new(),
                    started_at: Some(Instant::now()),
                    elapsed_ms: 0,
                });
                self.push_block_to(None, block);
                self.mark_dirty();
            }
            RunEvent::SubagentEnded { subagent_id, success, iterations_used } => {
                self.finalize_subagent(&subagent_id, success, iterations_used);
                self.mark_dirty_force();
            }

            RunEvent::ProcessSpawned { label, .. } => self.push_notice(format!("▶ spawned: {label}")),
            RunEvent::ProcessKilled { reason, .. } => self.push_notice(format!("■ process killed: {reason}")),

            // Out of scope for A (goal/plan/workflow), or purely telemetry.
            RunEvent::RunCreated { .. }
            | RunEvent::TodoUpdated { .. }
            | RunEvent::GoalSet { .. }
            | RunEvent::GoalCompleted { .. }
            | RunEvent::GoalCleared
            | RunEvent::CacheInfo { .. }
            | RunEvent::CacheSummary { .. }
            | RunEvent::ModeChanged { .. } => {}
        }
    }

    #[allow(dead_code)]
    fn apply_delta(&mut self, subagent_id: Option<&str>, delta: MessageDelta) {
        match delta {
            MessageDelta::Thinking(t) => {
                if subagent_id.is_none() && self.agent_state == "streaming" {
                    self.agent_state = "thinking".into();
                }
                self.append_text_delta(subagent_id, true, &t);
                if let Some(sid) = subagent_id {
                    self.update_subagent_activity(sid, "💭 Thinking...".to_string());
                }
                self.mark_dirty();
            }
            MessageDelta::Text(t) => {
                if subagent_id.is_none()
                    && (self.agent_state == "streaming" || self.agent_state == "thinking")
                {
                    self.agent_state = "responding".into();
                }
                self.append_text_delta(subagent_id, false, &t);
                self.mark_dirty();
            }
        }
    }

    fn append_text_delta(&mut self, subagent_id: Option<&str>, thinking: bool, text: &str) {
        let make_block = |id: u64| {
            if thinking {
                TurnBlock::Thought { id, text: String::new(), expanded: false }
            } else {
                TurnBlock::Response(String::new())
            }
        };

        let can_merge = match subagent_id {
            None => {
                let last = self.streaming.as_ref().and_then(|s| s.blocks.last());
                matches!(
                    (thinking, last),
                    (true, Some(TurnBlock::Thought { .. })) | (false, Some(TurnBlock::Response(_)))
                )
            }
            Some(sid) => {
                let last = self.find_subagent(sid).and_then(|sa| sa.children.last());
                matches!(
                    (thinking, last),
                    (true, Some(TurnBlock::Thought { .. })) | (false, Some(TurnBlock::Response(_)))
                )
            }
        };

        let new_id = if can_merge {
            None
        } else {
            Some(self.alloc_block_id())
        };

        let target: &mut Vec<TurnBlock> = match subagent_id {
            None => {
                if self.streaming.is_none() {
                    self.streaming = Some(Streaming { turn: 0, blocks: Vec::new() });
                }
                &mut self.streaming.as_mut().unwrap().blocks
            }
            Some(sid) => {
                if let Some(blocks) = find_subagent_children_mut(&mut self.streaming, &mut self.entries, sid) {
                    blocks
                } else {
                    return;
                }
            }
        };

        if let Some(id) = new_id {
            target.push(make_block(id));
            match target.last_mut() {
                Some(TurnBlock::Thought { text: existing, .. }) if thinking => existing.push_str(text),
                Some(TurnBlock::Response(existing)) if !thinking => existing.push_str(text),
                _ => {}
            }
        } else {
            match target.last_mut() {
                Some(TurnBlock::Thought { text: existing, .. }) if thinking => existing.push_str(text),
                Some(TurnBlock::Response(existing)) if !thinking => existing.push_str(text),
                _ => {}
            }
        }
    }

    fn push_block_to(&mut self, subagent_id: Option<&str>, block: TurnBlock) {
        match subagent_id {
            None => {
                if let Some(s) = &mut self.streaming {
                    s.blocks.push(block);
                } else {
                    self.entries.push(Entry::Turn { turn: 0, blocks: vec![block] });
                }
            }
            Some(sid) => {
                if let Some(children) =
                    find_subagent_children_mut(&mut self.streaming, &mut self.entries, sid)
                {
                    children.push(block);
                }
            }
        }
    }

    fn flush_streaming(&mut self) {
        if let Some(s) = self.streaming.take() {
            if !s.blocks.is_empty() {
                self.entries.push(Entry::Turn { turn: s.turn, blocks: s.blocks });
            }
        }
    }

    /// Update the most recent unresolved tool block matching `name` for the
    /// given scope. Returns `true` if a slot was found and updated.
    fn update_tool_result(
        &mut self,
        subagent_id: Option<&str>,
        name: &str,
        result: String,
        is_error: bool,
    ) -> bool {
        let updater = |blocks: &mut Vec<TurnBlock>| -> bool {
            for b in blocks.iter_mut().rev() {
                if let TurnBlock::Tool { name: n, result: r, .. } = b {
                    if n == name && r.is_none() {
                        *r = Some(ToolResult { text: result.clone(), is_error });
                        return true;
                    }
                }
            }
            false
        };
        match subagent_id {
            None => {
                if let Some(s) = &mut self.streaming {
                    if updater(&mut s.blocks) {
                        return true;
                    }
                }
                for entry in self.entries.iter_mut().rev() {
                    if let Entry::Turn { blocks, .. } = entry {
                        if updater(blocks) {
                            return true;
                        }
                    }
                }
                false
            }
            Some(sid) => {
                if let Some(children) =
                    find_subagent_children_mut(&mut self.streaming, &mut self.entries, sid)
                {
                    updater(children)
                } else {
                    false
                }
            }
        }
    }

    fn update_subagent_activity(&mut self, id: &str, activity: String) {
        let updater = |blocks: &mut Vec<TurnBlock>| -> bool {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        sa.current_activity = activity.clone();
                        return true;
                    }
                }
            }
            false
        };
        if let Some(s) = &mut self.streaming {
            if updater(&mut s.blocks) {
                return;
            }
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                if updater(blocks) {
                    return;
                }
            }
        }
    }

    fn finalize_subagent(&mut self, id: &str, success: bool, iterations: usize) {
        let label = if success { "complete" } else { "incomplete" };
        let finalizer = |blocks: &mut Vec<TurnBlock>| -> bool {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        let elapsed = sa.started_at.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
                        sa.done = true;
                        sa.success = success;
                        sa.iterations = iterations;
                        sa.elapsed_ms = elapsed;
                        sa.current_activity = format_elapsed_activity(label, iterations, elapsed);
                        return true;
                    }
                }
            }
            false
        };
        if let Some(s) = &mut self.streaming {
            if finalizer(&mut s.blocks) {
                return;
            }
        }
        for entry in &mut self.entries {
            if let Entry::Turn { blocks, .. } = entry {
                if finalizer(blocks) {
                    return;
                }
            }
        }
    }

    // ── MPSC reducer ──────────────────────────────────────────────────

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
            AppEvent::Run(ev) => {
                self.handle_run_event(ev);
                true
            }
            AppEvent::Tick => {
                self.frame_count = self.frame_count.wrapping_add(1);
                // Only force a redraw when something is actually animating
                // (gear/spinner frames) — avoids a constant 60fps redraw.
                self.agent_running
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

/// Locate the children `Vec<TurnBlock>` of a subagent by id, searching the
/// streaming turn first, then flushed entries.
fn find_subagent_children_mut<'a>(
    streaming: &'a mut Option<Streaming>,
    entries: &'a mut Vec<Entry>,
    id: &str,
) -> Option<&'a mut Vec<TurnBlock>> {
    if let Some(s) = streaming {
        for b in &mut s.blocks {
            if let TurnBlock::Subagent(sa) = b {
                if sa.id == id {
                    return Some(&mut sa.children);
                }
            }
        }
    }
    for entry in entries {
        if let Entry::Turn { blocks, .. } = entry {
            for b in blocks {
                if let TurnBlock::Subagent(sa) = b {
                    if sa.id == id {
                        return Some(&mut sa.children);
                    }
                }
            }
        }
    }
    None
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
    Thought { id: u64, text: String, expanded: bool },
    Response(String),
    Tool {
        id: u64,
        name: String,
        args: String,
        result: Option<ToolResult>,
        expanded: bool,
    },
    Subagent(SubagentState),
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
    pub started_at: Option<Instant>,
    pub elapsed_ms: u64,
}

#[derive(Clone)]
pub struct Streaming {
    pub turn: usize,
    pub blocks: Vec<TurnBlock>,
}

pub fn truncate_activity(s: &str, max_chars: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        let truncated: String = cleaned.chars().take(max_chars).collect();
        format!("{truncated}…")
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
            if path.is_empty() { pat } else { format!("{pat} in {path}") }
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
        format!("{elapsed_ms}ms")
    };
    if label == "complete" {
        format!("✓ complete ({iterations} iter) {time}")
    } else {
        format!("✗ incomplete ({iterations} iter) {time}")
    }
}

// ── MPSC event loop ──────────────────────────────────────────────────

pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent, Rect),
    Resize,
    Run(RunEvent),
    Tick,
}
