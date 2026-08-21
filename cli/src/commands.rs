//! Shared slash-command dispatch for REPL and TUI.

use crate::state::CliState;
use agent_core::{
    ApprovalChoice, Message, PermissionMode, Role, RunCommand, TaskStatus, TodoItem, TodoStatus,
    ToolExecutionMode,
};

/// Canonical slash commands: (command, short help).
pub const ALL_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show help"),
    ("/status", "Show agent status"),
    ("/quit", "Exit"),
    ("/exit", "Exit"),
    ("/models", "List / pick models"),
    ("/model", "Switch model"),
    ("/temp", "Set temperature"),
    ("/max-tokens", "Set max output tokens"),
    ("/tokens", "Show token estimate"),
    ("/context", "Dump context messages"),
    ("/clear", "Clear conversation"),
    ("/new", "Start fresh session"),
    ("/rewind", "Rewind to earlier message"),
    ("/abort", "Cancel current run"),
    ("/state", "Show idle/running"),
    ("/tool-mode", "parallel | sequential"),
    ("/steer", "Steer mid-run"),
    ("/follow-up", "Queue follow-up"),
    ("/clear-queues", "Clear steer/follow-up queues"),
    ("/memory", "Show core memory"),
    ("/memory search", "Search conversation memory"),
    ("/memory stats", "Memory stats"),
    ("/memory pending clear", "Clear pending notes"),
    ("/memory pending promote", "Promote pending notes"),
    ("/memory maintain", "Maintain agverse.md"),
    ("/permission", "Permission info"),
    ("/perm", "Permission info"),
    ("/perm test", "Test tool permission"),
    ("/perm mode", "Set permission mode"),
    ("/hooks", "Hook system info"),
    ("/mcp", "List MCP servers/tools"),
    ("/todo", "Show todos"),
    ("/todo add", "Add todo"),
    ("/todo start", "Start todo"),
    ("/todo done", "Complete todo"),
    ("/todo clear", "Clear todos"),
    ("/tasks", "Show task board"),
    ("/tasks add", "Add task"),
    ("/tasks start", "Start task"),
    ("/tasks done", "Complete task"),
    ("/tasks clear", "Clear tasks"),
    ("/sessions", "List sessions"),
    ("/session", "Session help"),
    ("/session save", "Save session"),
    ("/session resume", "Resume session"),
    ("/session delete", "Delete session"),
    ("/session rename", "Rename session"),
    ("/session archive", "Archive session"),
    ("/session search", "Search sessions"),
    ("/skills", "List skills"),
    ("/skill", "Show skill"),
    ("/skill active", "Active skills"),
    ("/skill deactivate", "Deactivate skill(s)"),
    ("/skill reload", "Rescan skills"),
    ("/workflow", "Create a durable workflow with the agent"),
];

#[derive(Debug, Clone)]
pub enum CmdMessage {
    Info(String),
    Warn(String),
    Error(String),
}

impl CmdMessage {
    pub fn text(&self) -> &str {
        match self {
            Self::Info(s) | Self::Warn(s) | Self::Error(s) => s,
        }
    }
}

#[derive(Debug, Clone)]
pub enum UiRequest {
    ModelPicker {
        models: Vec<String>,
        current: String,
    },
    ModelForm,
    SessionList,
    Help,
    Status,
    ShowText {
        title: String,
        body: String,
    },
    RewindList {
        points: Vec<(usize, String)>,
    },
}

#[derive(Debug, Clone)]
pub enum CommandOutcome {
    Handled { messages: Vec<CmdMessage> },
    Quit,
    NeedsUi(UiRequest),
    Unknown(String),
    /// Input is not a slash command.
    NotSlash,
}

impl CommandOutcome {
    fn info(s: impl Into<String>) -> Self {
        Self::Handled {
            messages: vec![CmdMessage::Info(s.into())],
        }
    }
    fn err(s: impl Into<String>) -> Self {
        Self::Handled {
            messages: vec![CmdMessage::Error(s.into())],
        }
    }
    fn msgs(messages: Vec<CmdMessage>) -> Self {
        Self::Handled { messages }
    }
}

/// Map GUI / TUI choice keys to ApprovalChoice.
pub fn approval_from_choice_key(s: &str) -> ApprovalChoice {
    match s {
        "allow_once" | "3" => ApprovalChoice::AllowOnce,
        "allow_session" | "4" => ApprovalChoice::AllowSession,
        "allow_persistent" | "5" => ApprovalChoice::AllowPersistent,
        "deny_persistent" | "2" => ApprovalChoice::DenyPersistent,
        _ => ApprovalChoice::Deny, // "deny" | "1" | anything else
    }
}

pub fn command_names() -> Vec<String> {
    ALL_COMMANDS.iter().map(|(c, _)| (*c).to_string()).collect()
}

pub fn help_text() -> String {
    let mut lines = vec!["Available commands:".to_string()];
    for (cmd, help) in ALL_COMMANDS {
        lines.push(format!("  {:<28} {help}", cmd));
    }
    lines.push(String::new());
    lines.push("Keys: ? help · Esc layered cancel · G/End follow · y yank · t thought".into());
    lines.join("\n")
}

fn workflow_goal(input: &str) -> Option<&str> {
    if input == "/workflow" {
        Some("")
    } else {
        input.strip_prefix("/workflow ").map(str::trim)
    }
}

/// Sync slash dispatch (no await). Async-only commands return a placeholder
/// message directing the caller to use `dispatch_async`.
pub fn dispatch_sync(
    state: &mut CliState,
    cmd: &str,
    enable_permission: bool,
    enable_hooks: bool,
) -> CommandOutcome {
    let input = cmd.trim();
    if !input.starts_with('/') {
        return CommandOutcome::NotSlash;
    }
    if let Some(goal) = workflow_goal(input) {
        state.pending_workflow_request = Some(goal.to_string());
        return if goal.is_empty() {
            CommandOutcome::info("Starting workflow authoring…")
        } else {
            CommandOutcome::info("Creating a workflow draft…")
        };
    }

    match input {
        "/quit" | "/exit" => CommandOutcome::Quit,
        "/help" => CommandOutcome::NeedsUi(UiRequest::Help),
        "/status" => CommandOutcome::NeedsUi(UiRequest::Status),
        "/models" | "/model" => {
            let mut models: Vec<String> = state.brain.config().models.keys().cloned().collect();
            models.sort();
            let current = state.brain.current_model_name().to_string();
            CommandOutcome::NeedsUi(UiRequest::ModelPicker { models, current })
        }
        "/models new" => CommandOutcome::NeedsUi(UiRequest::ModelForm),
        "/clear" => {
            state.context_history.clear();
            state.session_id = None;
            CommandOutcome::info("Context cleared. New session started.")
        }
        "/new" => {
            state.context_history.clear();
            state.session_id = None;
            CommandOutcome::info("Fresh session started. Previous context cleared.")
        }
        "/tokens" => CommandOutcome::info(format!(
            "Current tokens (estimate): {}",
            state.context_history.len() as u32 * 4
        )),
        "/state" => {
            let s = if state.current_run_id.is_some() {
                "Running"
            } else {
                "Idle"
            };
            CommandOutcome::info(format!("State: {s}"))
        }
        "/tool-mode" => CommandOutcome::info(format!(
            "Tool mode: {:?}",
            state.brain.tool_execution_mode()
        )),
        "/permission" | "/perm" => CommandOutcome::info(format!(
            "Permission system: {}\nRules checked before each tool.\nUse /perm test <tool> [json] or /perm mode <mode>.",
            if enable_permission {
                "active"
            } else {
                "disabled"
            }
        )),
        "/hooks" => CommandOutcome::info(format!(
            "Hook system: {}\nHooks: PreToolUse, PostToolUse, SessionStart, SessionEnd",
            if enable_hooks { "active" } else { "disabled" }
        )),
        "/todo" => {
            let list = state.todo_list.lock();
            if list.items.is_empty() {
                CommandOutcome::info("Todo list is empty. Use /todo add <id> <description>")
            } else {
                CommandOutcome::info(list.to_context_string())
            }
        }
        "/tasks" => {
            let board = state.task_board.lock();
            CommandOutcome::info(board.summary())
        }
        "/skills" => list_skills(state),
        "/skill active" => {
            let mgr = state.skill_manager.lock();
            let active = mgr.active_skill_names();
            if active.is_empty() {
                CommandOutcome::info("No active skills.")
            } else {
                CommandOutcome::info(format!("Active: {}", active.join(", ")))
            }
        }
        "/skill reload" => {
            let mut mgr = state.skill_manager.lock();
            match mgr.scan() {
                Ok(n) => CommandOutcome::info(format!("Reloaded skills ({n} found).")),
                Err(e) => CommandOutcome::err(format!("Reload failed: {e}")),
            }
        }
        "/sessions" => CommandOutcome::NeedsUi(UiRequest::SessionList),
        "/session" => CommandOutcome::info(
            "Usage: /session save|resume <id>|delete <id>|rename <id> <title>|archive <id>|search <q>",
        ),
        "/memory" => memory_list(state),
        "/memory stats" => memory_stats(state),
        "/memory pending clear" => {
            let path = agent_core::paths::get_global_agverse_md_path();
            match agent_core::memory::agverse_md::clear_pending_notes_file(&path) {
                Ok(n) => CommandOutcome::info(format!("Cleared {n} pending note(s).")),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "/memory pending promote" => {
            let path = agent_core::paths::get_global_agverse_md_path();
            match agent_core::memory::agverse_md::promote_pending_notes_file(&path) {
                Ok(n) => CommandOutcome::info(format!("Promoted {n} pending note(s).")),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "/memory maintain" => {
            let path = agent_core::paths::get_global_agverse_md_path();
            match agent_core::memory::agverse_md::maintain_agverse_file(&path) {
                Ok(r) => CommandOutcome::info(format!(
                    "Maintained: expired={}, trimmed={}, sections={}",
                    r.pending_expired, r.trimmed_bullets, r.sections_ensured
                )),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "/context" => dump_context(state),
        "/abort" | "/steer" | "/follow-up" | "/clear-queues" | "/mcp" => {
            // Caller must use dispatch_async — return marker via NeedsUi? Better: empty Handled
            // with a special prefix. We'll just document that REPL/TUI call async for these.
            CommandOutcome::info("__ASYNC__")
        }
        _ => dispatch_sync_prefixed(state, input, enable_permission),
    }
}

fn dispatch_sync_prefixed(
    state: &mut CliState,
    input: &str,
    enable_permission: bool,
) -> CommandOutcome {
    if let Some(name) = input.strip_prefix("/model ").map(str::trim) {
        if name.is_empty() || name == "new" {
            return dispatch_sync(state, "/models", enable_permission, false);
        }
        return match state.run_manager.switch_model(name) {
            Ok(()) => {
                state.brain = (**state.run_manager.brain()).clone();
                CommandOutcome::info(format!("Switched to model: {name}"))
            }
            Err(e) => CommandOutcome::err(format!("Failed to switch model: {e}")),
        };
    }
    if let Some(rest) = input.strip_prefix("/temp ") {
        return match rest.trim().parse::<f64>() {
            Ok(t) => {
                state.run_manager.set_temperature(t);
                state.brain = (**state.run_manager.brain()).clone();
                CommandOutcome::info(format!("Temperature set to {t}"))
            }
            Err(_) => CommandOutcome::err("Usage: /temp <float>"),
        };
    }
    if let Some(rest) = input.strip_prefix("/max-tokens ") {
        return match rest.trim().parse::<u32>() {
            Ok(n) => {
                state.run_manager.set_max_tokens(n);
                state.brain = (**state.run_manager.brain()).clone();
                CommandOutcome::info(format!("Max tokens set to {n}"))
            }
            Err(_) => CommandOutcome::err("Usage: /max-tokens <u32>"),
        };
    }
    if let Some(rest) = input.strip_prefix("/tool-mode ") {
        let mode = match rest.trim() {
            "parallel" | "par" => ToolExecutionMode::Parallel,
            "sequential" | "seq" => ToolExecutionMode::Sequential,
            _ => {
                return CommandOutcome::err("Usage: /tool-mode parallel|sequential");
            }
        };
        state.run_manager.set_tool_execution_mode(mode);
        state.brain = (**state.run_manager.brain()).clone();
        return CommandOutcome::info(format!("Tool mode: {mode:?}"));
    }
    if let Some(rest) = input.strip_prefix("/rewind") {
        let rest = rest.trim();
        if rest.is_empty() {
            let points: Vec<(usize, String)> = state
                .context_history
                .iter()
                .enumerate()
                .filter(|(_, m)| m.role == Role::User)
                .map(|(i, m)| {
                    let c = m.content.as_deref().unwrap_or("");
                    let preview: String = c.chars().take(60).collect();
                    (i, preview)
                })
                .collect();
            if points.is_empty() {
                return CommandOutcome::info("No conversation history to rewind.");
            }
            return CommandOutcome::NeedsUi(UiRequest::RewindList { points });
        }
        return match rest.parse::<usize>() {
            Ok(idx) => {
                let total = state.context_history.len();
                state.context_history.truncate(idx);
                let removed = total.saturating_sub(state.context_history.len());
                CommandOutcome::info(format!(
                    "Rewound: kept first {idx} messages, removed {removed} (was {total})."
                ))
            }
            Err(_) => CommandOutcome::err(format!(
                "Invalid index '{rest}'. Use /rewind to see points."
            )),
        };
    }
    if let Some(rest) = input.strip_prefix("/memory search ") {
        let q = rest.trim();
        if q.is_empty() {
            return CommandOutcome::err("Usage: /memory search <query>");
        }
        let Some(memory) = state.brain.memory() else {
            return CommandOutcome::info("Memory is disabled.");
        };
        let mem = memory.lock();
        return match mem.search_conversation(q, 5) {
            Ok(hits) => {
                if hits.is_empty() {
                    CommandOutcome::info("No matches.")
                } else {
                    let body = hits
                        .iter()
                        .map(|h| {
                            format!(
                                "- [{:.2}] {}",
                                h.importance,
                                h.content.chars().take(80).collect::<String>()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    CommandOutcome::info(body)
                }
            }
            Err(e) => CommandOutcome::err(format!("{e}")),
        };
    }
    if let Some(rest) = input.strip_prefix("/perm mode ") {
        let mode = match parse_perm_mode(rest.trim()) {
            Ok(m) => m,
            Err(e) => return CommandOutcome::err(e),
        };
        state.run_manager.set_permission_mode(mode);
        state.brain = (**state.run_manager.brain()).clone();
        return CommandOutcome::info(format!("Permission mode set to {mode:?}"));
    }
    if let Some(rest) = input.strip_prefix("/perm test ") {
        return perm_test(state, rest.trim());
    }
    if let Some(rest) = input.strip_prefix("/todo ") {
        return todo_cmd(state, rest.trim());
    }
    if let Some(rest) = input.strip_prefix("/tasks ") {
        return tasks_cmd(state, rest.trim());
    }
    if let Some(rest) = input.strip_prefix("/skill deactivate ") {
        let name = rest.trim();
        let mut mgr = state.skill_manager.lock();
        if name == "all" {
            mgr.deactivate_all();
            return CommandOutcome::info("All skills deactivated.");
        }
        mgr.deactivate(name);
        return CommandOutcome::info(format!("Deactivated skill '{name}'."));
    }
    if let Some(rest) = input.strip_prefix("/skill ") {
        let name = rest.trim();
        if name.is_empty() || name == "active" || name == "reload" {
            return CommandOutcome::Unknown(input.to_string());
        }
        let mgr = state.skill_manager.lock();
        return match mgr.load_skill_context(name) {
            Ok(Some(body)) => CommandOutcome::NeedsUi(UiRequest::ShowText {
                title: format!("skill: {name}"),
                body,
            }),
            Ok(None) => CommandOutcome::err(format!("Skill '{name}' not found.")),
            Err(e) => CommandOutcome::err(format!("Skill '{name}' error: {e}")),
        };
    }
    if let Some(rest) = input.strip_prefix("/session ") {
        return session_sync(state, rest.trim());
    }
    if let Some(rest) = input.strip_prefix("/steer ") {
        let _ = rest;
        return CommandOutcome::info("__ASYNC__");
    }
    if let Some(rest) = input.strip_prefix("/follow-up ") {
        let _ = rest;
        return CommandOutcome::info("__ASYNC__");
    }

    // Known compound prefix but incomplete?
    if ALL_COMMANDS.iter().any(|(c, _)| input.starts_with(c) || c.starts_with(input)) {
        // fall through
    }
    CommandOutcome::Unknown(input.to_string())
}

/// Async slash commands (abort, steer, follow-up, clear-queues, mcp, and any sync fallback).
pub async fn dispatch_async(
    state: &mut CliState,
    cmd: &str,
    enable_permission: bool,
    enable_hooks: bool,
) -> CommandOutcome {
    let input = cmd.trim();

    if input == "/abort" {
        return abort_run(state).await;
    }
    if input == "/clear-queues" {
        return clear_queues(state).await;
    }
    if input == "/mcp" {
        return mcp_status(state).await;
    }
    if let Some(msg) = input.strip_prefix("/steer ") {
        return steer(state, msg.trim()).await;
    }
    if let Some(msg) = input.strip_prefix("/follow-up ") {
        return follow_up(state, msg.trim()).await;
    }

    // Session ops that are sync but live here for uniformity when called from async loop
    let sync = dispatch_sync(state, input, enable_permission, enable_hooks);
    if matches!(
        &sync,
        CommandOutcome::Handled { messages } if messages.first().map(|m| m.text() == "__ASYNC__").unwrap_or(false)
    ) {
        return CommandOutcome::err(format!(
            "Command '{input}' requires arguments. Try /help."
        ));
    }
    sync
}

async fn abort_run(state: &mut CliState) -> CommandOutcome {
    let Some(rid) = state.current_run_id.clone() else {
        return CommandOutcome::info("No active run to abort.");
    };
    match state.run_manager.cancel_run(&rid).await {
        Ok(()) => CommandOutcome::info("Run cancelled."),
        Err(e) => CommandOutcome::err(format!("Abort failed: {e}")),
    }
}

async fn steer(state: &mut CliState, message: &str) -> CommandOutcome {
    if message.is_empty() {
        return CommandOutcome::err("Usage: /steer <message>");
    }
    let Some(rid) = state.current_run_id.clone() else {
        return CommandOutcome::err("No active run. Start a conversation first.");
    };
    let steer_id = format!(
        "steer-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    match state
        .run_manager
        .command(
            &rid,
            RunCommand::Steer {
                steer_id,
                message: message.to_string(),
            },
        )
        .await
    {
        Ok(()) => CommandOutcome::info("Steer queued."),
        Err(e) => CommandOutcome::err(format!("Steer failed: {e}")),
    }
}

async fn follow_up(state: &mut CliState, message: &str) -> CommandOutcome {
    if message.is_empty() {
        return CommandOutcome::err("Usage: /follow-up <message>");
    }
    let Some(rid) = state.current_run_id.clone() else {
        return CommandOutcome::err("No active run for follow-up.");
    };
    match state
        .run_manager
        .command(
            &rid,
            RunCommand::FollowUp {
                message: message.to_string(),
            },
        )
        .await
    {
        Ok(()) => CommandOutcome::info("Follow-up queued."),
        Err(e) => CommandOutcome::err(format!("Follow-up failed: {e}")),
    }
}

async fn clear_queues(state: &mut CliState) -> CommandOutcome {
    let Some(rid) = state.current_run_id.clone() else {
        return CommandOutcome::info("No active run (queues already clear).");
    };
    match state
        .run_manager
        .command(&rid, RunCommand::ClearQueues)
        .await
    {
        Ok(()) => CommandOutcome::info("Steer/follow-up queues cleared."),
        Err(e) => CommandOutcome::err(format!("Clear queues failed: {e}")),
    }
}

async fn mcp_status(state: &mut CliState) -> CommandOutcome {
    let mgr = state.mcp_mgr.lock().await;
    let servers = mgr.connected_servers();
    if servers.is_empty() {
        return CommandOutcome::info(
            "No MCP servers connected. Configure in [mcp.servers] in config.toml.",
        );
    }
    let mut lines = vec![format!("=== MCP Servers ({}) ===", servers.len())];
    for s in &servers {
        lines.push(format!("  • {s} (connected)"));
    }
    let tools = mgr.all_tools();
    lines.push(format!("\n=== MCP Tools ({}) ===", tools.len()));
    for t in &tools {
        lines.push(format!(
            "  • mcp__{}__{} — {}",
            t.server, t.name, t.description
        ));
    }
    CommandOutcome::info(lines.join("\n"))
}

fn list_skills(state: &CliState) -> CommandOutcome {
    let mgr = state.skill_manager.lock();
    let skills = mgr.list_with_sources();
    if skills.is_empty() {
        let mut lines = vec!["No skills found. Searched:".to_string()];
        for dir in mgr.search_dirs() {
            lines.push(format!("  {}", dir.display()));
        }
        return CommandOutcome::info(lines.join("\n"));
    }
    let mut lines = vec!["=== Available Skills ===".to_string()];
    for (skill, source) in &skills {
        let desc: String = skill.description.chars().take(80).collect();
        lines.push(format!(
            "  {}  {}  ({})",
            skill.name,
            desc.replace('\n', " "),
            source.display()
        ));
    }
    CommandOutcome::info(lines.join("\n"))
}

fn memory_list(state: &CliState) -> CommandOutcome {
    let Some(memory) = state.brain.memory() else {
        return CommandOutcome::info("Memory is disabled.");
    };
    let mem = memory.lock();
    let mut lines = vec!["=== Core Memory ===".to_string()];
    lines.push(format!("Session: {}", mem.session_id()));
    for block in mem.core().list() {
        lines.push(format!("[{}]: {}", block.id, block.content));
    }
    CommandOutcome::info(lines.join("\n"))
}

fn memory_stats(state: &CliState) -> CommandOutcome {
    let Some(memory) = state.brain.memory() else {
        return CommandOutcome::info("Memory is disabled.");
    };
    let mem = memory.lock();
    match mem.stats() {
        Ok(s) => CommandOutcome::info(format!(
            "Total: {}\nAvg strength: {:.2}\nAvg importance: {:.2}",
            s.total_count, s.avg_strength, s.avg_importance
        )),
        Err(e) => CommandOutcome::err(format!("{e}")),
    }
}

fn dump_context(state: &CliState) -> CommandOutcome {
    if state.context_history.is_empty() {
        return CommandOutcome::info("Context is empty.");
    }
    let mut lines = vec![format!(
        "=== Context ({} messages) ===",
        state.context_history.len()
    )];
    for (i, m) in state.context_history.iter().enumerate() {
        let preview: String = m
            .content
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        lines.push(format!("[{i}] {:?}: {preview}", m.role));
    }
    CommandOutcome::NeedsUi(UiRequest::ShowText {
        title: "context".into(),
        body: lines.join("\n"),
    })
}

fn parse_perm_mode(s: &str) -> Result<PermissionMode, String> {
    match s {
        "paranoid" => Ok(PermissionMode::Paranoid),
        "standard" => Ok(PermissionMode::Standard),
        "developer" => Ok(PermissionMode::Developer),
        "permissive" => Ok(PermissionMode::Permissive),
        "yolo" => Ok(PermissionMode::Yolo),
        _ => Err(
            "Usage: /perm mode paranoid|standard|developer|permissive|yolo".into(),
        ),
    }
}

fn perm_test(state: &CliState, rest: &str) -> CommandOutcome {
    let mut parts = rest.splitn(2, ' ');
    let tool = parts.next().unwrap_or("").trim();
    if tool.is_empty() {
        return CommandOutcome::err("Usage: /perm test <tool> [json-input]");
    }
    let input = parts.next().unwrap_or("{}").trim();
    let json = if input.is_empty() { "{}" } else { input };
    let mut policy = state.brain.build_permission_policy();
    let decision = policy.check(tool, json, None, None, None);
    CommandOutcome::info(format!("Decision for '{tool}': {decision:?}"))
}

fn todo_cmd(state: &CliState, rest: &str) -> CommandOutcome {
    let mut parts = rest.splitn(3, ' ');
    let op = parts.next().unwrap_or("");
    let mut list = state.todo_list.lock();
    match op {
        "add" => {
            let id = parts.next().unwrap_or("").trim();
            let desc = parts.next().unwrap_or("").trim();
            if id.is_empty() || desc.is_empty() {
                return CommandOutcome::err("Usage: /todo add <id> <description>");
            }
            list.add(TodoItem::new(id, desc));
            CommandOutcome::info(format!("Added todo '{id}'."))
        }
        "start" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /todo start <id>");
            }
            match list.update_status(id, TodoStatus::InProgress) {
                Ok(()) => CommandOutcome::info(format!("Started todo '{id}'.")),
                Err(e) => CommandOutcome::err(e),
            }
        }
        "done" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /todo done <id>");
            }
            match list.update_status(id, TodoStatus::Completed) {
                Ok(()) => CommandOutcome::info(format!("Completed todo '{id}'.")),
                Err(e) => CommandOutcome::err(e),
            }
        }
        "clear" => {
            list.items.clear();
            CommandOutcome::info("Todo list cleared.")
        }
        _ => CommandOutcome::err("Usage: /todo add|start|done|clear"),
    }
}

fn tasks_cmd(state: &CliState, rest: &str) -> CommandOutcome {
    let mut parts = rest.splitn(3, ' ');
    let op = parts.next().unwrap_or("");
    let mut board = state.task_board.lock();
    match op {
        "add" => {
            let id = parts.next().unwrap_or("").trim();
            let desc = parts.next().unwrap_or("").trim();
            if id.is_empty() || desc.is_empty() {
                return CommandOutcome::err("Usage: /tasks add <id> <description>");
            }
            board.create(id, desc, vec![]);
            CommandOutcome::info(format!("Added task '{id}'."))
        }
        "start" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /tasks start <id>");
            }
            match board.update(id, TaskStatus::InProgress, None) {
                Ok(()) => CommandOutcome::info(format!("Started task '{id}'.")),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "done" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /tasks done <id>");
            }
            match board.update(id, TaskStatus::Completed, None) {
                Ok(()) => CommandOutcome::info(format!("Completed task '{id}'.")),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "clear" => {
            *board = agent_core::TaskBoard::new();
            CommandOutcome::info("Task board cleared.")
        }
        _ => CommandOutcome::err("Usage: /tasks add|start|done|clear"),
    }
}

fn session_sync(state: &mut CliState, rest: &str) -> CommandOutcome {
    let mut parts = rest.splitn(3, ' ');
    let op = parts.next().unwrap_or("");
    match op {
        "save" => save_session(state),
        "resume" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::NeedsUi(UiRequest::SessionList);
            }
            resume_session(state, id)
        }
        "delete" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /session delete <id>");
            }
            match state.session_mgr.delete(id) {
                Ok(true) => CommandOutcome::info(format!("Deleted session {id}.")),
                Ok(false) => CommandOutcome::err("Session not found."),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "rename" => {
            let id = parts.next().unwrap_or("").trim();
            let title = parts.next().unwrap_or("").trim();
            if id.is_empty() || title.is_empty() {
                return CommandOutcome::err("Usage: /session rename <id> <title>");
            }
            match state.session_mgr.rename(id, title) {
                Ok(true) => CommandOutcome::info(format!("Renamed session {id}.")),
                Ok(false) => CommandOutcome::err("Session not found."),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "archive" => {
            let id = parts.next().unwrap_or("").trim();
            if id.is_empty() {
                return CommandOutcome::err("Usage: /session archive <id>");
            }
            match state.session_mgr.archive(id) {
                Ok(true) => CommandOutcome::info(format!("Archived session {id}.")),
                Ok(false) => CommandOutcome::err("Session not found."),
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        "search" => {
            let q = parts.next().unwrap_or("").trim();
            if q.is_empty() {
                return CommandOutcome::err("Usage: /session search <keyword>");
            }
            match state.session_mgr.search(q, 20) {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        CommandOutcome::info("No matching sessions.")
                    } else {
                        let body = sessions
                            .iter()
                            .map(|s| s.display_line())
                            .collect::<Vec<_>>()
                            .join("\n");
                        CommandOutcome::info(body)
                    }
                }
                Err(e) => CommandOutcome::err(format!("{e}")),
            }
        }
        _ => CommandOutcome::err(
            "Usage: /session save|resume|delete|rename|archive|search",
        ),
    }
}

pub fn save_session(state: &mut CliState) -> CommandOutcome {
    let messages: Vec<Message> = state
        .context_history
        .iter()
        .filter(|m| m.role != Role::System)
        .cloned()
        .collect();
    if messages.is_empty() {
        return CommandOutcome::info("Nothing to save (empty conversation).");
    }
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    let model = state.brain.current_model_name().to_string();
    match state
        .session_mgr
        .save(state.session_id.as_deref(), &messages, &cwd, &model)
    {
        Ok(id) => {
            state.session_id = Some(id.clone());
            let short = id.chars().take(8).collect::<String>();
            CommandOutcome::info(format!("Session saved: {short}"))
        }
        Err(e) => CommandOutcome::err(format!("Save failed: {e}")),
    }
}

fn resume_session(state: &mut CliState, id: &str) -> CommandOutcome {
    match state.session_mgr.resume(id) {
        Ok(Some(session)) => {
            state.context_history.clear();
            state.context_history.extend(session.messages.clone());
            state.session_id = Some(id.to_string());
            CommandOutcome::info(format!(
                "Resumed session '{}' ({} messages).",
                session.meta.title,
                session.messages.len()
            ))
        }
        Ok(None) => CommandOutcome::err("Session not found."),
        Err(e) => CommandOutcome::err(format!("{e}")),
    }
}

pub fn format_status(
    state: &CliState,
    enable_permission: bool,
    enable_hooks: bool,
) -> String {
    let model = state.brain.current_model_name();
    let running = if state.current_run_id.is_some() {
        "Running"
    } else {
        "Idle"
    };
    let todos = state.todo_list.lock().items.len();
    let tasks = state.task_board.lock().all_tasks().len();
    let skills = state.skill_manager.lock().list_with_sources().len();
    format!(
        "=== Status ===\n\
         Model: {model}\n\
         State: {running}\n\
         Tool mode: {:?}\n\
         Tokens (est): {}\n\
         Permission: {}\n\
         Hooks: {}\n\
         Todos: {todos}  Tasks: {tasks}  Skills: {skills}\n\
         Session: {}",
        state.brain.tool_execution_mode(),
        state.context_history.len() as u32 * 4,
        if enable_permission {
            "active"
        } else {
            "disabled"
        },
        if enable_hooks { "active" } else { "disabled" },
        state
            .session_id
            .as_deref()
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "(none)".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_keys_map_correctly() {
        assert!(matches!(
            approval_from_choice_key("allow_once"),
            ApprovalChoice::AllowOnce
        ));
        assert!(matches!(
            approval_from_choice_key("allow_session"),
            ApprovalChoice::AllowSession
        ));
        assert!(matches!(
            approval_from_choice_key("allow_persistent"),
            ApprovalChoice::AllowPersistent
        ));
        assert!(matches!(
            approval_from_choice_key("deny_persistent"),
            ApprovalChoice::DenyPersistent
        ));
        assert!(matches!(
            approval_from_choice_key("deny"),
            ApprovalChoice::Deny
        ));
        assert!(matches!(
            approval_from_choice_key("1"),
            ApprovalChoice::Deny
        ));
        assert!(matches!(
            approval_from_choice_key("5"),
            ApprovalChoice::AllowPersistent
        ));
    }

    #[test]
    fn all_commands_nonempty() {
        assert!(!ALL_COMMANDS.is_empty());
        assert!(ALL_COMMANDS.iter().any(|(c, _)| *c == "/help"));
        assert!(ALL_COMMANDS.iter().any(|(c, _)| *c == "/mcp"));
        assert!(ALL_COMMANDS.iter().any(|(c, _)| *c == "/workflow"));
    }

    #[test]
    fn help_text_contains_quit() {
        assert!(help_text().contains("/quit"));
    }

    #[test]
    fn workflow_command_extracts_optional_goal() {
        assert_eq!(workflow_goal("/workflow"), Some(""));
        assert_eq!(
            workflow_goal("/workflow research then review"),
            Some("research then review")
        );
        assert_eq!(workflow_goal("/workflows"), None);
    }
}
