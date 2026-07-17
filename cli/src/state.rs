use agent_core::{Brain, CancellationToken, McpClientManager, RunManager, SessionManager, SkillManager, TaskBoard, TodoList};
use parking_lot::Mutex;
use std::sync::Arc;

/// CLI session state — replaces the legacy `Agent` struct.
///
/// Holds the shared Brain + RunManager, plus per-session data
/// (context history, tool registrations, session info).
pub struct CliState {
    /// The shared Brain (config, memory, skills, etc.).
    pub brain: Brain,
    /// The RunManager for creating and managing Runs.
    pub run_manager: RunManager,
    /// Per-session context history (persists across Runs).
    pub context_history: Vec<agent_core::Message>,
    /// Current session ID.
    pub session_id: Option<String>,
    /// Currently active Run ID (for cancel/steer commands).
    pub current_run_id: Option<String>,
    /// Cancel token for the current Run.
    pub cancel_token: Option<CancellationToken>,
    /// Shared todo list.
    pub todo_list: Arc<Mutex<TodoList>>,
    /// Shared task board.
    pub task_board: Arc<Mutex<TaskBoard>>,
    /// Shared skill manager.
    pub skill_manager: Arc<Mutex<SkillManager>>,
    /// MCP client manager.
    pub mcp_mgr: Arc<tokio::sync::Mutex<McpClientManager>>,
    /// Session manager for persistence (shared with RunManager prompt lifecycle).
    pub session_mgr: Arc<SessionManager>,
}
