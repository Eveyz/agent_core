mod cli_completer;
mod tui;

use agent_core::{
    AgentBuilder, AgentEvent, ApprovalChoice, McpClientManager, Message, MessageDelta,
    PermissionPolicy, Role, SessionManager, SkillManager, SkillManifest, TaskBoard, TaskStatus,
    TodoItem, TodoList, TodoStatus, ToolExecutionMode, hooks::LoggingHook, tasks, tools,
};
use argh::FromArgs;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, EditMode, Editor, Helper};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

// ── Tab completion ────────────────────────────────────────────────

/// All slash-commands recognized by the CLI. Used for tab-completion and /help.
const ALL_COMMANDS: &[&str] = &[
    // General
    "/help",
    "/status",
    "/quit",
    "/exit",
    // Model & Context
    "/models",
    "/model",
    "/temp",
    "/max-tokens",
    "/tokens",
    "/context",
    "/clear",
    "/new",
    "/rewind",
    // Agent Control
    "/abort",
    "/state",
    "/tool-mode",
    "/steer",
    "/follow-up",
    "/clear-queues",
    // Memory
    "/memory",
    "/memory search",
    "/memory stats",
    // Permission & Hooks
    "/permission",
    "/perm",
    "/perm test",
    "/perm mode",
    "/hooks",
    // MCP
    "/mcp",
    // Planning
    "/todo",
    "/todo add",
    "/todo start",
    "/todo done",
    "/todo clear",
    // Task Board
    "/tasks",
    "/tasks add",
    "/tasks start",
    "/tasks done",
    "/tasks clear",
    // Sessions
    "/sessions",
    "/session",
    "/session save",
    "/session resume",
    "/session delete",
    "/session rename",
    "/session archive",
    "/session search",
    // Skills
    "/skills",
    "/skill",
    "/skill active",
    "/skill deactivate",
    "/skill reload",
];

struct CommandCompleter;

impl Highlighter for CommandCompleter {}
impl Hinter for CommandCompleter {
    type Hint = String;
}
impl Validator for CommandCompleter {}
impl Helper for CommandCompleter {}

impl Completer for CommandCompleter {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        // Only complete commands starting with /
        let prefix = &line[..pos];
        if !prefix.starts_with('/') {
            return Ok((0, vec![]));
        }

        let mut matches: Vec<String> = ALL_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(prefix) && **cmd != prefix)
            .map(|s| s.to_string())
            .collect();
        matches.sort();

        // Find the start position: we replace from the beginning of the / command
        let start = line[..pos].rfind('/').unwrap_or(0);

        Ok((start, matches))
    }
}

fn dim(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[2m" } else { "" }
}
fn bold(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[1m" } else { "" }
}
fn cyan(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[36m" } else { "" }
}
fn yellow(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[33m" } else { "" }
}
fn green(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[32m" } else { "" }
}
fn red(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[31m" } else { "" }
}
#[allow(dead_code)]
fn blue(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[34m" } else { "" }
}
#[allow(dead_code)]
fn magenta(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[35m" } else { "" }
}
fn reset(use_styles: bool) -> &'static str {
    if use_styles { "\x1b[0m" } else { "" }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_len);
        format!("{}...", &s[..end])
    }
}
/// CLI arguments for agent-cli
#[derive(FromArgs)]
struct Args {
    /// launch TUI mode
    #[argh(switch, short = 't')]
    tui: bool,
}

/// Try to load config.toml; if the file doesn't exist, generate a template and
/// still attempt the env-var fallback.
fn try_load_config() -> anyhow::Result<AgentBuilder> {
    match AgentBuilder::from_config("config.toml") {
        Ok(b) => return Ok(b),
        Err(e) => {
            if !Path::new("config.toml").exists() {
                generate_config_template("config.toml");
            } else {
                eprintln!("config.toml: {e}");
            }
            eprintln!("Falling back to OPENAI_API_KEY environment variable...");
            AgentBuilder::from_env()
        }
    }
}

/// Write a well-commented config.toml template so the user only needs to fill in their API keys.
fn generate_config_template(path: &str) {
    let template = r#"# =============================================================================
# Agent Core 配置文件
# 刚生成模板 — 请填入你的 API Key 后重新运行
# =============================================================================
# 语法说明:
#   api_key = "sk-xxx"             → 直接写明文
#   api_key = "${DEEPSEEK_KEY}"    → 从环境变量读取 (更安全)
# =============================================================================

# 默认使用的模型 (必须与下方 [models.xxx] 中的某一个一致)
default_model = "deepseek"

# ── 记忆系统 (可选，全部可省略使用默认值) ────────────────────────────
[memory]
# db_path = "~/.agent_core/memory.db"
# embedding_model = "BAAI/bge-small-en-v1.5"
# max_core_blocks = 5
# default_block_max_chars = 2000
# consolidation_enabled = true

# ── DeepSeek ──────────────────────────────────────────────────────────
[models.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_KEY}"              # 改成你的 key，或用环境变量 DEEPSEEK_KEY
model_id = "deepseek-chat"
max_context_tokens = 65536
# temperature = 0.7
# max_tokens = 4096
# react_enabled = true
# max_iterations = 10

# ── OpenAI GPT-4o ─────────────────────────────────────────────────────
[models.gpt4o]
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
model_id = "gpt-4o"
max_context_tokens = 128000
# temperature = 0.7

# ── 本地 Ollama (取消注释即可使用) ────────────────────────────────────
# [models.local]
# base_url = "http://localhost:11434/v1"
# api_key = "ollama"
# model_id = "qwen2.5:7b"
# max_context_tokens = 32768
"#;
    if let Err(e) = std::fs::write(path, template) {
        eprintln!("warning: could not write config template to {path}: {e}");
    } else {
        eprintln!("No config.toml found — a template has been generated at {path}");
        eprintln!("Please edit it with your API keys and re-run.\n");
    }
}

async fn run_tui_mode() -> anyhow::Result<()> {
    let builder = try_load_config()?;

    // Skill manager (created before build so it can be passed to agent)
    let mut skill_manager = SkillManager::with_defaults();
    let _ = skill_manager.scan();
    let skill_manager = Arc::new(Mutex::new(skill_manager));

    let builder = builder
        .with_memory(false)
        .with_skill_manager(skill_manager.clone())
        .with_tool_execution_mode(ToolExecutionMode::Parallel);

    let mut agent = builder.build()?;

    // Register tools
    let todo_list: Arc<Mutex<TodoList>> = Arc::new(Mutex::new(TodoList::new()));
    let task_board: Arc<Mutex<TaskBoard>> = Arc::new(Mutex::new(TaskBoard::new()));

    {
        let model_config = agent.current_model_config().clone();
        let reg = agent.tool_registry_mut();
        tools::todo::register_todo_tools(reg, todo_list.clone());
        tools::skill::register_skill_tools(reg, skill_manager.clone());
        let task_board_clone = task_board.clone();
        tasks::register_task_tools(reg, task_board_clone, model_config);
    }
    {
        let model_config = agent.current_model_config().clone();
        let tool_names: Vec<String> = agent
            .tool_registry()
            .list_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let reg = agent.tool_registry_mut();
        tools::subagent::register_subagent_tools(reg, model_config, tool_names, None);
    }

    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    tui::run_tui(agent).await
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let args: Args = argh::from_env();
    // ── TUI mode ──────────────────────────────────────────────────
    if args.tui {
        return run_tui_mode().await;
    }
    // ── CLI mode (existing) ───────────────────────────────────────
    let mut builder = try_load_config()?;

    // Detect whether stdout is a terminal for styled output
    let use_styles = std::io::stdout().is_terminal();
    println!("=== Agent Core CLI ===\n");

    // Memory
    print!("Enable memory system? (y/N): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let enable_memory = input.trim().to_lowercase() == "y";
    builder = builder.with_memory(enable_memory);

    // Permission
    print!("Enable permission system? (Y/n): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let enable_permission = input.trim().to_lowercase() != "n";
    if enable_permission {
        builder = builder.with_permission_policy(PermissionPolicy::new());
    }

    // Hooks
    print!("Enable hook system? (Y/n): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let enable_hooks = input.trim().to_lowercase() != "n";
    if enable_hooks {
        let mut hooks = agent_core::HookRegistry::new();
        hooks.register(Box::new(LoggingHook));
        builder = builder.with_hook_registry(hooks);
    }

    // Tool execution mode
    print!("Tool execution mode (parallel/sequential) [parallel]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let tool_mode = match input.trim().to_lowercase().as_str() {
        "sequential" | "seq" => ToolExecutionMode::Sequential,
        _ => ToolExecutionMode::Parallel,
    };
    builder = builder.with_tool_execution_mode(tool_mode);

    // Skill manager (created before build to pass to agent)
    let mut skill_manager = SkillManager::with_defaults();
    let _ = skill_manager.scan();
    let skill_manager = Arc::new(Mutex::new(skill_manager));
    builder = builder.with_skill_manager(skill_manager.clone());

    let mut agent = builder.build()?;

    // Optional subsystems
    let todo_list: Arc<Mutex<TodoList>> = Arc::new(Mutex::new(TodoList::new()));
    let task_board: Arc<Mutex<TaskBoard>> = Arc::new(Mutex::new(TaskBoard::new()));

    // Register todo, skill, task, and subagent tools
    {
        let model_config = agent.current_model_config().clone();
        let reg = agent.tool_registry_mut();
        tools::todo::register_todo_tools(reg, todo_list.clone());
        tools::skill::register_skill_tools(reg, skill_manager.clone());
        let task_board_clone = task_board.clone();
        tasks::register_task_tools(reg, task_board_clone, model_config);
    }

    // Session manager — create before tool registration for subagent sessions
    let session_db = "~/.agent_core/memory.db";
    let session_storage =
        agent_core::memory::storage::Storage::new(session_db).expect("Failed to open session DB");
    let session_mgr = Arc::new(Mutex::new(SessionManager::new(session_storage)));
    let _current_session_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Register subagent tool (needs model config + tool names + session_mgr)
    {
        let model_config = agent.current_model_config().clone();
        let tool_names: Vec<String> = agent
            .tool_registry()
            .list_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let reg = agent.tool_registry_mut();
        tools::subagent::register_subagent_tools(
            reg,
            model_config,
            tool_names,
            Some(session_mgr.clone()),
        );
    }

    // MCP — connect to configured servers
    let mcp_mgr = {
        let config = agent.config();
        let mut mgr = McpClientManager::from_config(&config.mcp);
        let errors = mgr.connect_all().await;
        for (name, errs) in &errors {
            for err in errs {
                eprintln!("[MCP] Server '{}' connection failed: {}", name, err);
            }
        }
        if mgr.tool_count() > 0 {
            println!(
                "[MCP] {} tools from {} servers",
                mgr.tool_count(),
                mgr.connected_servers().len()
            );
            // Register MCP tools
            let mgr_arc = Arc::new(tokio::sync::Mutex::new(mgr));
            agent_core::McpTool::register_all(agent.tool_registry_mut(), mgr_arc.clone());
            mgr_arc
        } else {
            Arc::new(tokio::sync::Mutex::new(mgr))
        }
    };

    // Share cancel_token so CLI can abort mid-run
    let cancel_token = agent.cancel_token.clone();

    // Print status
    println!("\n--- Status ---");
    println!(
        "Memory:      {}",
        if enable_memory { "enabled" } else { "disabled" }
    );
    println!(
        "Permission:  {}",
        if enable_permission {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "Hooks:       {}",
        if enable_hooks { "enabled" } else { "disabled" }
    );
    println!("Tool mode:   {:?}", agent.tool_execution_mode());
    println!("Tools:       {}", agent.tool_registry().list_names().len());
    println!("Model:       {}", agent.current_model());
    println!("--------------\n");
    println!("Type /help for commands, /quit to exit\n");

    // ── Setup readline with tab-completion and history ──────────────
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();
    let mut rl = Editor::with_config(config).expect("Failed to create line editor");
    rl.set_helper(Some(CommandCompleter));
    let _ = rl.load_history(".agent_core_history");

    loop {
        let input = match rl.readline("> ") {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                let _ = rl.add_history_entry(&line);
                trimmed
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                eprintln!("Input error: {err}");
                break;
            }
        };

        if input.is_empty() {
            continue;
        }

        match input.as_str() {
            "/quit" | "/exit" => {
                // ── Graceful shutdown ────────────────────────────────

                // 1. Auto-save session
                let all_messages = agent.context_messages();
                let messages: Vec<Message> = all_messages
                    .into_iter()
                    .filter(|m| m.role != Role::System)
                    .collect();
                if !messages.is_empty() {
                    let cwd = std::env::current_dir()
                        .ok()
                        .and_then(|p| p.to_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let model = agent.current_model();
                    if let Ok(mgr) = session_mgr.lock() {
                        let current_id = _current_session_id.lock().unwrap().clone();
                        if let Ok(id) = mgr.save(current_id.as_deref(), &messages, &cwd, &model) {
                            println!("Session auto-saved: {}", &id[..8]);
                        }
                    }
                }

                // 2. Shut down MCP connections (kill child processes)
                {
                    let mut mgr = mcp_mgr.lock().await;
                    if let Err(e) = mgr.shutdown_all().await {
                        eprintln!("MCP shutdown warning: {e}");
                    }
                }

                // 3. Save command history
                if let Err(e) = rl.save_history(".agent_core_history") {
                    eprintln!("History save warning: {e}");
                }

                println!("Bye!");
                break;
            }
            "/help" => {
                print_help();
            }
            "/models" => {
                for (name, is_current) in agent.list_models() {
                    let marker = if is_current { "* " } else { "  " };
                    println!("{marker}{name}");
                }
            }
            "/clear" => {
                agent.clear_context();
                *_current_session_id.lock().unwrap() = None;
                println!("Context cleared. New session started.");
            }
            "/new" => {
                agent.clear_context();
                *_current_session_id.lock().unwrap() = None;
                println!("Fresh session started. Previous context cleared.");
            }
            cmd if cmd.starts_with("/rewind") => {
                let rest = cmd.strip_prefix("/rewind").unwrap_or("").trim();
                if rest.is_empty() {
                    // Show rewindable points
                    let msgs = agent.context_messages();
                    let user_indices: Vec<(usize, &str)> = msgs
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.role == Role::User)
                        .map(|(i, m)| (i, m.content.as_deref().unwrap_or("")))
                        .collect();
                    if user_indices.is_empty() {
                        println!("No conversation history to rewind.");
                    } else {
                        println!("=== Rewind points (user messages) ===");
                        for (idx, content) in &user_indices {
                            let preview = truncate(content, 60);
                            println!("  [{idx}] {preview}");
                        }
                        println!("\nUse /rewind <index> to go back to that point.");
                    }
                } else {
                    match rest.parse::<usize>() {
                        Ok(idx) => {
                            let (removed, total) = agent.rewind_context_to(idx);
                            if removed > 0 {
                                println!(
                                    "Rewound: kept first {idx} messages, removed {removed} (was {total} total)."
                                );
                            } else {
                                println!(
                                    "No messages removed (index {idx} >= {total} total messages)."
                                );
                            }
                        }
                        Err(_) => eprintln!(
                            "Invalid index '{}'. Use /rewind to see available points.",
                            rest
                        ),
                    }
                }
            }
            "/tokens" => {
                println!("Current tokens: {}", agent.context_token_count());
            }
            "/permission" | "/perm" => {
                println!("=== Permission Policy ===");
                println!(
                    "Permission system: {}",
                    if enable_permission {
                        "active"
                    } else {
                        "disabled"
                    }
                );
                println!("Rules are checked before each tool execution.");
                println!("Use /perm test <tool> <input> to check a specific call.");
            }
            "/hooks" => {
                println!("=== Hook Registry ===");
                println!(
                    "Hook system: {}",
                    if enable_hooks { "active" } else { "disabled" }
                );
                println!("Hooks fire on: PreToolUse, PostToolUse, SessionStart, SessionEnd");
            }
            "/mcp" => {
                let mgr = mcp_mgr.lock().await;
                let servers = mgr.connected_servers();
                if servers.is_empty() {
                    println!(
                        "No MCP servers connected. Configure in [mcp.servers] in config.toml."
                    );
                } else {
                    println!("=== MCP Servers ({}) ===", servers.len());
                    for s in &servers {
                        println!("  • {} ({})", s, "connected");
                    }
                    let tools = mgr.all_tools();
                    if tools.is_empty() {
                        println!("\nNo tools discovered.");
                    } else {
                        println!("\n=== MCP Tools ({}) ===", tools.len());
                        for t in &tools {
                            println!("  • mcp__{}__{} — {}", t.server, t.name, t.description);
                        }
                    }
                }
            }
            "/todo" => {
                let list = todo_list.lock().unwrap();
                if list.items.is_empty() {
                    println!("Todo list is empty. Use /todo add <id> <description>");
                } else {
                    println!("{}", list.to_context_string());
                }
            }
            "/tasks" => {
                let board = task_board.lock().unwrap();
                println!("{}", board.summary());
            }
            "/skills" => {
                let mgr = skill_manager.lock().unwrap();
                let skills = mgr.list_with_sources();
                if skills.is_empty() {
                    println!("No skills found. Searched:");
                    for dir in mgr.search_dirs() {
                        println!("  {}", dir.display());
                    }
                } else {
                    println!("=== Available Skills ===");

                    // Group by source directory
                    let mut by_source: std::collections::BTreeMap<String, Vec<&SkillManifest>> =
                        std::collections::BTreeMap::new();
                    for (skill, source) in &skills {
                        by_source
                            .entry(source.to_string_lossy().to_string())
                            .or_default()
                            .push(skill);
                    }

                    for (source, group) in &by_source {
                        println!("\n{}", source);
                        for skill in group {
                            let desc: String = skill.description.chars().take(80).collect();
                            let truncated = if skill.description.chars().count() > 80 {
                                format!("{}...", desc)
                            } else {
                                desc
                            };
                            // Replace newlines in description for single-line display
                            let desc_one_line = truncated.replace('\n', " ");
                            println!("  {}  {}", skill.name, desc_one_line);
                        }
                    }
                }
            }
            "/status" => {
                print_status(
                    &agent,
                    enable_memory,
                    enable_permission,
                    enable_hooks,
                    &todo_list,
                    &task_board,
                    &skill_manager,
                );
            }
            "/sessions" => {
                let mgr = session_mgr.lock().unwrap();
                match mgr.list(false) {
                    Ok(sessions) => {
                        if sessions.is_empty() {
                            println!(
                                "No sessions saved. Use /session save to save the current session."
                            );
                        } else {
                            println!("--- Sessions ({}) ---", sessions.len());
                            for s in &sessions {
                                println!("  {}", s.display_line());
                            }
                        }
                    }
                    Err(e) => eprintln!("Failed to list sessions: {e}"),
                }
            }
            cmd if cmd.starts_with("/session") => {
                let mgr = session_mgr.lock().unwrap();
                let args: Vec<&str> = cmd.splitn(4, ' ').collect();

                match args.get(1).copied() {
                    Some("save") => {
                        let all_messages = agent.context_messages();
                        // Filter out system messages — they're generated from segments,
                        // not part of the conversation history
                        let messages: Vec<Message> = all_messages
                            .into_iter()
                            .filter(|m| m.role != Role::System)
                            .collect();
                        let cwd = std::env::current_dir()
                            .ok()
                            .and_then(|p| p.to_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let model = agent.current_model();
                        let current_id = _current_session_id.lock().unwrap().clone();
                        match mgr.save(current_id.as_deref(), &messages, &cwd, &model) {
                            Ok(id) => {
                                *_current_session_id.lock().unwrap() = Some(id.clone());
                                println!("Session saved: {}", &id[..8]);
                            }
                            Err(e) => eprintln!("Failed to save session: {e}"),
                        }
                    }
                    Some("resume") => {
                        let session_id = args.get(2).copied().unwrap_or("");
                        if session_id.is_empty() {
                            println!("Usage: /session resume <id>");
                        } else {
                            match mgr.resume(session_id) {
                                Ok(Some(session)) => {
                                    agent.clear_context();
                                    for msg in &session.messages {
                                        agent.context_mut().add(msg.clone());
                                    }
                                    *_current_session_id.lock().unwrap() =
                                        Some(session_id.to_string());
                                    println!(
                                        "Resumed session '{}' ({} messages).",
                                        session.meta.title,
                                        session.messages.len()
                                    );
                                }
                                Ok(None) => println!("Session not found: {session_id}"),
                                Err(e) => eprintln!("Failed to resume: {e}"),
                            }
                        }
                    }
                    Some("delete") => {
                        let session_id = args.get(2).copied().unwrap_or("");
                        if session_id.is_empty() {
                            println!("Usage: /session delete <id>");
                        } else {
                            match mgr.delete(session_id) {
                                Ok(true) => println!("Session deleted: {session_id}"),
                                Ok(false) => println!("Session not found: {session_id}"),
                                Err(e) => eprintln!("Failed to delete: {e}"),
                            }
                        }
                    }
                    Some("rename") => {
                        let session_id = args.get(2).copied().unwrap_or("");
                        let new_title = args.get(3).copied().unwrap_or("");
                        if session_id.is_empty() || new_title.is_empty() {
                            println!("Usage: /session rename <id> <new_title>");
                        } else {
                            match mgr.rename(session_id, new_title) {
                                Ok(true) => println!("Renamed to: {new_title}"),
                                Ok(false) => println!("Session not found: {session_id}"),
                                Err(e) => eprintln!("Failed to rename: {e}"),
                            }
                        }
                    }
                    Some("archive") => {
                        let session_id = args.get(2).copied().unwrap_or("");
                        if session_id.is_empty() {
                            println!("Usage: /session archive <id>");
                        } else {
                            match mgr.archive(session_id) {
                                Ok(true) => println!("Archived: {session_id}"),
                                _ => println!("Session not found."),
                            }
                        }
                    }
                    Some("search") => {
                        let keyword = args.get(2).copied().unwrap_or("");
                        if keyword.is_empty() {
                            println!("Usage: /session search <keyword>");
                        } else {
                            match mgr.search(keyword, 20) {
                                Ok(sessions) => {
                                    if sessions.is_empty() {
                                        println!("No sessions matching '{}'", keyword);
                                    } else {
                                        for s in &sessions {
                                            println!("  {}", s.display_line());
                                        }
                                    }
                                }
                                Err(e) => eprintln!("Search failed: {e}"),
                            }
                        }
                    }
                    _ => {
                        println!("Session commands:");
                        println!("  /sessions               — list all sessions");
                        println!("  /session save           — save current session");
                        println!("  /session resume <id>    — resume a session");
                        println!("  /session delete <id>    — delete a session");
                        println!("  /session rename <id> <t>— rename a session");
                        println!("  /session archive <id>   — archive a session");
                        println!("  /session search <kw>    — search sessions");
                    }
                }
            }
            "/abort" => {
                cancel_token.cancel();
                println!("Abort signal sent. The agent will stop at the next opportunity.");
            }
            "/state" => {
                println!("Agent state: {:?}", agent.state());
            }
            "/tool-mode" => {
                println!("Tool execution mode: {:?}", agent.tool_execution_mode());
            }
            "/clear-queues" => {
                agent.clear_all_queues();
                println!("Steering and follow-up queues cleared.");
            }
            cmd if cmd.starts_with("/model ") => {
                let name = cmd.strip_prefix("/model ").unwrap().trim();
                match agent.switch_model(name) {
                    Ok(()) => println!("Switched to model: {name}"),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }
            cmd if cmd.starts_with("/temp ") => {
                let val_str = cmd.strip_prefix("/temp ").unwrap().trim();
                match val_str.parse::<f64>() {
                    Ok(val) => {
                        agent.set_temperature(val);
                        println!("Temperature set to {val}");
                    }
                    Err(_) => eprintln!("Invalid temperature value"),
                }
            }
            cmd if cmd.starts_with("/max-tokens ") => {
                let val_str = cmd.strip_prefix("/max-tokens ").unwrap().trim();
                match val_str.parse::<u32>() {
                    Ok(val) => {
                        agent.set_max_tokens(val);
                        println!("Max tokens set to {val}");
                    }
                    Err(_) => eprintln!("Invalid max-tokens value"),
                }
            }
            cmd if cmd.starts_with("/tool-mode ") => {
                let mode_str = cmd.strip_prefix("/tool-mode ").unwrap().trim();
                match mode_str.to_lowercase().as_str() {
                    "parallel" | "par" => {
                        agent.set_tool_execution_mode(ToolExecutionMode::Parallel);
                        println!("Tool execution mode set to: parallel");
                    }
                    "sequential" | "seq" => {
                        agent.set_tool_execution_mode(ToolExecutionMode::Sequential);
                        println!("Tool execution mode set to: sequential");
                    }
                    _ => {
                        eprintln!("Usage: /tool-mode <parallel|sequential>");
                    }
                }
            }
            cmd if cmd.starts_with("/steer ") => {
                let msg = cmd.strip_prefix("/steer ").unwrap().trim();
                agent.steer(Message::user(msg));
                println!("Steering message queued. It will be injected after the current turn.");
            }
            cmd if cmd.starts_with("/follow-up ") => {
                let msg = cmd.strip_prefix("/follow-up ").unwrap().trim();
                agent.follow_up(Message::user(msg));
                println!(
                    "Follow-up message queued. It will be processed after the agent finishes."
                );
            }
            cmd if cmd.starts_with("/todo ") => {
                handle_todo_cmd(cmd, &todo_list);
            }
            cmd if cmd.starts_with("/tasks ") => {
                handle_tasks_cmd(cmd, &task_board);
            }
            cmd if cmd.starts_with("/skill ") => {
                let rest = cmd.strip_prefix("/skill ").unwrap().trim();

                if rest.starts_with("deactivate ") {
                    let name = rest.strip_prefix("deactivate ").unwrap().trim();
                    let mut mgr = skill_manager.lock().unwrap();
                    if name == "all" {
                        mgr.deactivate_all();
                        println!("All skills deactivated.");
                    } else if mgr.deactivate(name) {
                        println!("Skill '{}' deactivated.", name);
                    } else {
                        eprintln!("Skill '{}' is not active.", name);
                    }
                } else if rest == "reload" {
                    let mut mgr = skill_manager.lock().unwrap();
                    match mgr.scan() {
                        Ok(count) => println!("Reloaded {} skills from disk.", count),
                        Err(e) => eprintln!("Reload failed: {e}"),
                    }
                } else if rest == "active" {
                    let mgr = skill_manager.lock().unwrap();
                    let active = mgr.active_skill_names();
                    if active.is_empty() {
                        println!("No active skills.");
                    } else {
                        println!("Active skills: {}", active.join(", "));
                    }
                } else {
                    // Default: load skill
                    let mgr = skill_manager.lock().unwrap();
                    match mgr.load_skill_context(rest) {
                        Ok(Some(content)) => {
                            println!("{}", content);
                        }
                        Ok(None) => {
                            eprintln!("Skill '{}' not found. Use /skills to list.", rest);
                        }
                        Err(e) => eprintln!("Error loading skill: {e}"),
                    }
                }
            }
            cmd if cmd.starts_with("/perm ") => {
                let args = cmd.strip_prefix("/perm ").unwrap().trim();
                if args.starts_with("test ") {
                    let rest = args.strip_prefix("test ").unwrap().trim();
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    let tool_name = parts[0];
                    let tool_input = parts.get(1).unwrap_or(&"{}");
                    let decision = agent
                        .permission_policy_mut()
                        .check(tool_name, tool_input, None, None, None);
                    println!(
                        "Permission check: {}({}) -> {:?}",
                        tool_name, tool_input, decision
                    );
                } else if args.starts_with("mode ") {
                    let mode_str = args.strip_prefix("mode ").unwrap().trim();
                    use agent_core::PermissionMode;
                    let mode = match mode_str.to_lowercase().as_str() {
                        "paranoid" => PermissionMode::Paranoid,
                        "standard" => PermissionMode::Standard,
                        "permissive" => PermissionMode::Permissive,
                        "yolo" => PermissionMode::Yolo,
                        _ => {
                            eprintln!("Invalid mode. Use: paranoid | standard | permissive | yolo");
                            continue;
                        }
                    };
                    agent.permission_policy_mut().set_mode(mode);
                    println!("Permission mode set to: {:?}", mode);
                } else {
                    eprintln!("Usage:");
                    eprintln!("  /perm test <tool_name> <input_json>");
                    eprintln!("  /perm mode <paranoid|standard|permissive|yolo>");
                }
            }
            "/context" => {
                let msgs = agent.context_messages();
                let tokens = agent.context_token_count();
                println!("=== Context ({tokens} tokens, {} messages) ===", msgs.len());
                // Show the 7-segment breakdown
                if let Some(hint) = agent.context_cache_hint() {
                    println!(
                        "KV Cache: {} stable tokens, strategy={}",
                        hint.stable_prefix_tokens, hint.strategy
                    );
                }
                println!("\nMessages (newest last):");
                for (i, msg) in msgs.iter().enumerate() {
                    let role_str = match msg.role {
                        Role::System => "SYS",
                        Role::User => "USR",
                        Role::Assistant => "AST",
                        Role::Tool => "TOL",
                    };
                    let content = msg.content.as_deref().unwrap_or("");
                    let preview = truncate(content, 100);
                    if let Some(ref tc) = msg.tool_calls {
                        let tool_names: Vec<&str> =
                            tc.iter().map(|t| t.function.name.as_str()).collect();
                        println!(
                            "  [{i}] {role_str} [tools: {}] {}",
                            tool_names.join(","),
                            preview
                        );
                    } else {
                        println!("  [{i}] {role_str} {preview}");
                    }
                }
            }
            cmd if cmd.starts_with("/memory ") => {
                let rest = cmd.strip_prefix("/memory ").unwrap().trim();
                if rest.starts_with("search ") {
                    let query = rest.strip_prefix("search ").unwrap().trim();
                    if let Some(memory) = agent.memory() {
                        match memory.search_conversation(query, 5) {
                            Ok(results) => {
                                if results.is_empty() {
                                    println!("No results for '{}'", query);
                                } else {
                                    println!(
                                        "=== Memory search: '{}' ({}) ===",
                                        query,
                                        results.len()
                                    );
                                    for (i, r) in results.iter().enumerate() {
                                        let preview = truncate(&r.content, 80);
                                        println!(
                                            "  [{i}] importance={:.2} | {}",
                                            r.importance, preview
                                        );
                                    }
                                }
                            }
                            Err(e) => eprintln!("Search error: {e}"),
                        }
                    } else {
                        println!("Memory is disabled.");
                    }
                } else if rest == "stats" {
                    if let Some(memory) = agent.memory() {
                        match memory.stats() {
                            Ok(stats) => {
                                println!("=== Memory Stats ===");
                                println!("Total:       {}", stats.total_count);
                                println!("Avg strength:{:.2}", stats.avg_strength);
                                println!("Avg importance:{:.2}", stats.avg_importance);
                            }
                            Err(e) => eprintln!("Stats error: {e}"),
                        }
                    } else {
                        println!("Memory is disabled.");
                    }
                } else {
                    eprintln!("Usage: /memory search <query> | /memory stats");
                }
            }
            "/memory" => {
                if let Some(memory) = agent.memory() {
                    println!("=== Core Memory ===");
                    for block in memory.core().list() {
                        println!("[{}]: {}", block.id, block.content);
                    }
                    println!("\nSession: {}", memory.session_id());
                } else {
                    println!("Memory is disabled.");
                }
            }
            // Catch unknown /commands before they hit the agent
            input if input.starts_with('/') => {
                eprintln!(
                    "Unknown command: {}. Type /help for available commands.",
                    input
                );
            }
            _ => {
                // Reset abort flag before each run
                // The cancel_token is recreated by agent run loop on next start.
                // agent.cancel_token.cancel() would cancel it, but to clear it we do nothing.
                run_agent(&mut agent, &input, use_styles).await;
            }
        }
    }

    Ok(())
}

fn handle_todo_cmd(cmd: &str, todo_list: &Arc<Mutex<TodoList>>) {
    let args = cmd.strip_prefix("/todo ").unwrap().trim();

    if args == "clear" {
        let mut list = todo_list.lock().unwrap();
        list.items.clear();
        println!("Todo list cleared.");
        return;
    }

    if args.starts_with("add ") {
        let rest = args.strip_prefix("add ").unwrap().trim();
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() < 2 {
            eprintln!("Usage: /todo add <id> <description>");
            return;
        }
        let id = parts[0];
        let desc = parts[1];
        let mut list = todo_list.lock().unwrap();
        list.add(TodoItem::new(id, desc));
        println!("Added todo '{}': {}", id, desc);
        return;
    }

    if args.starts_with("done ") {
        let id = args.strip_prefix("done ").unwrap().trim();
        let mut list = todo_list.lock().unwrap();
        match list.update_status(id, TodoStatus::Completed) {
            Ok(()) => println!("Todo '{}' marked done.", id),
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    if args.starts_with("start ") {
        let id = args.strip_prefix("start ").unwrap().trim();
        let mut list = todo_list.lock().unwrap();
        match list.update_status(id, TodoStatus::InProgress) {
            Ok(()) => println!("Todo '{}' started.", id),
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    // Default: show list
    let list = todo_list.lock().unwrap();
    if list.items.is_empty() {
        println!("Todo list is empty. Use /todo add <id> <description>");
    } else {
        println!("{}", list.to_context_string());
    }
}

fn handle_tasks_cmd(cmd: &str, task_board: &Arc<Mutex<TaskBoard>>) {
    let args = cmd.strip_prefix("/tasks ").unwrap().trim();

    if args == "clear" {
        let mut board = task_board.lock().unwrap();
        *board = TaskBoard::new();
        println!("Task board cleared.");
        return;
    }

    if args.starts_with("add ") {
        let rest = args.strip_prefix("add ").unwrap().trim();
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() < 2 {
            eprintln!("Usage: /tasks add <id> <description>");
            return;
        }
        let id = parts[0];
        let desc = parts[1];
        let mut board = task_board.lock().unwrap();
        board.create(id, desc, vec![]);
        println!("Task '{}' created: {}", id, desc);
        return;
    }

    if args.starts_with("done ") {
        let id = args.strip_prefix("done ").unwrap().trim();
        let mut board = task_board.lock().unwrap();
        match board.update(id, TaskStatus::Completed, None) {
            Ok(()) => println!("Task '{}' completed.", id),
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    if args.starts_with("start ") {
        let id = args.strip_prefix("start ").unwrap().trim();
        let mut board = task_board.lock().unwrap();
        match board.update(id, TaskStatus::InProgress, None) {
            Ok(()) => println!("Task '{}' started.", id),
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    // Default: show board
    let board = task_board.lock().unwrap();
    println!("{}", board.summary());
}

fn print_status(
    agent: &agent_core::Agent,
    enable_memory: bool,
    enable_permission: bool,
    enable_hooks: bool,
    todo_list: &Arc<Mutex<TodoList>>,
    task_board: &Arc<Mutex<TaskBoard>>,
    skill_manager: &Arc<Mutex<SkillManager>>,
) {
    println!("=== Agent Status ===");
    println!("Model:       {}", agent.current_model());
    println!("State:       {:?}", agent.state());
    println!("Tool mode:   {:?}", agent.tool_execution_mode());
    println!("Tokens:      {}", agent.context_token_count());
    println!("Memory:      {}", if enable_memory { "on" } else { "off" });
    println!(
        "Permission:  {}",
        if enable_permission { "on" } else { "off" }
    );
    println!("Hooks:       {}", if enable_hooks { "on" } else { "off" });
    println!("Tools:       {}", agent.tool_registry().list_names().len());
    {
        let list = todo_list.lock().unwrap();
        println!("Todo:        {}", list.summary());
    }
    {
        let board = task_board.lock().unwrap();
        println!("Tasks:       {} total", board.all_tasks().len());
    }
    {
        let mgr = skill_manager.lock().unwrap();
        println!(
            "Skills:      {} loaded, {} active",
            mgr.count(),
            mgr.active_skill_names().len()
        );
    }
}

// ── UI helpers ─────────────────────────────────────────────────────

/// Build args preview: strip `{}` for empty, show key-value pairs for others.
fn fmt_tool_args(args: &serde_json::Value) -> String {
    let s = args.to_string();
    if s == "{}" {
        return String::new();
    }
    // Truncate early so we don't print huge JSON blobs
    truncate(&s, 100)
}

/// Output a single compact tool line:  🔧 tool(args) → result
fn print_tool_line(
    tool_name: &str,
    args: &serde_json::Value,
    result: &str,
    is_error: bool,
    use_styles: bool,
) {
    let args_str = fmt_tool_args(args);
    let result_preview = truncate(result, 120).replace('\n', " ");

    if is_error {
        println!(
            "  {bold}{cyan}🔧{reset} {bold}{tool}{reset}({dim}{args}{reset}) {red}✗{reset} {red}{result}{reset}",
            bold = bold(use_styles),
            cyan = cyan(use_styles),
            reset = reset(use_styles),
            tool = tool_name,
            dim = dim(use_styles),
            args = args_str,
            red = red(use_styles),
            result = result_preview,
        );
    } else {
        println!(
            "  {bold}{cyan}🔧{reset} {bold}{tool}{reset}({dim}{args}{reset}) {green}→{reset} {dim}{result}{reset}",
            bold = bold(use_styles),
            cyan = cyan(use_styles),
            reset = reset(use_styles),
            tool = tool_name,
            dim = dim(use_styles),
            args = args_str,
            green = green(use_styles),
            result = result_preview,
        );
    }
}

/// Run agent inline — aborts are handled via `cancel_token` which is
/// checked inside `collect_stream` on every chunk and between turns.
async fn run_agent(
    agent: &mut agent_core::Agent,
    input: &str,
    use_styles: bool,
) {
    let first_event = std::sync::atomic::AtomicBool::new(true);
    let in_thinking = std::sync::atomic::AtomicBool::new(false);
    let in_agent_text = std::sync::atomic::AtomicBool::new(false);
    let skin = termimad::MadSkin::default();
    let in_sub_thinking = std::sync::atomic::AtomicBool::new(false);
    let in_sub_text = std::sync::atomic::AtomicBool::new(false);

    // Deferred tool output: buffer ToolExecutionStart until ToolExecutionEnd arrives
    let pending_tool: std::sync::Mutex<Option<(String, serde_json::Value)>> = std::sync::Mutex::new(None);

    print!("\r  ⏳{}", reset(use_styles));
    io::stdout().flush().ok();

    match agent
        .run_with_events(input, |event| {
            if first_event.load(std::sync::atomic::Ordering::Relaxed) {
                print!("\r                                    \r");
                io::stdout().flush().ok();
                first_event.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            match event {
                AgentEvent::AgentStart => {}
                AgentEvent::AgentEnd { .. } => {}
                AgentEvent::TurnStart { turn_index } => {
                    // Flush any pending tool before new turn
                    in_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    in_agent_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    if turn_index > 0 {
                        println!(
                            "\n  {}─── Turn {turn_index} ───{}",
                            dim(use_styles),
                            reset(use_styles)
                        );
                    }
                }
                AgentEvent::MessageUpdate { delta } => match delta {
                    MessageDelta::Thinking(t) => {
                        // Flush pending tool output before thinking
                        pending_tool.lock().unwrap().take();
                        if in_agent_text.load(std::sync::atomic::Ordering::Relaxed) {
                            println!();
                            in_agent_text.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        if !in_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                            println!(); // visual separator before think block
                            print!(
                                "  {}{}💭 Think{} {}",
                                bold(use_styles),
                                yellow(use_styles),
                                reset(use_styles),
                                dim(use_styles),
                            );
                            in_thinking.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        print!("{}{}{}", dim(use_styles), t, reset(use_styles));
                        io::stdout().flush().ok();
                    }
                    MessageDelta::Text(t) => {
                        // Flush pending tool output before text
                        pending_tool.lock().unwrap().take();
                        if in_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                            println!("{}", reset(use_styles));
                            in_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        if !in_agent_text.load(std::sync::atomic::Ordering::Relaxed) {
                            println!(); // visual separator before model output
                            print!(
                                "  {}{}>>{} {}{}",
                                bold(use_styles),
                                green(use_styles),
                                reset(use_styles),
                                reset(use_styles),
                                reset(use_styles),
                            );
                            in_agent_text.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        if use_styles {
                            skin.print_inline(&t);
                        } else {
                            print!("{t}");
                        }
                        io::stdout().flush().ok();
                    }
                },
                AgentEvent::MessageEnd { .. } => {}
                AgentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => {
                    // Defer — buffer it, print when ToolExecutionEnd arrives
                    pending_tool.lock().unwrap().replace((tool_name, args));
                }
                AgentEvent::ToolExecutionUpdate { .. } => {
                    // Silently consume — end event will show final result
                }
                AgentEvent::ToolExecutionEnd {
                    tool_name,
                    result,
                    is_error,
                    ..
                } => {
                    // Flush thinking/text prefix before tool line
                    if in_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                        println!("{}", reset(use_styles));
                        in_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                    if in_agent_text.load(std::sync::atomic::Ordering::Relaxed) {
                        println!(); // end in-progress text line
                        in_agent_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                    println!(); // visual separator before tool output

                    let pt = pending_tool.lock().unwrap().take();
                    let args = pt
                        .as_ref()
                        .map(|(_, a)| a.clone())
                        .unwrap_or_else(|| serde_json::Value::String("?".to_string()));

                    print_tool_line(&tool_name, &args, &result, is_error, use_styles);
                    io::stdout().flush().ok();
                }
                AgentEvent::Error(e) => {
                    pending_tool.lock().unwrap().take();
                    eprintln!(
                        "  {}{}✗{}{} {}{}",
                        bold(use_styles),
                        red(use_styles),
                        reset(use_styles),
                        reset(use_styles),
                        e,
                        reset(use_styles),
                    );
                }
                AgentEvent::ApprovalRequired {
                    prompt_id,
                    tool_name,
                    danger_level,
                    explanation,
                    ..
                } | AgentEvent::SubagentApprovalRequired {
                    prompt_id,
                    tool_name,
                    danger_level,
                    explanation,
                    ..
                } => {
                    pending_tool.lock().unwrap().take();
                    println!(
                        "  {}⚠ APPROVAL{} {} ({})",
                        yellow(use_styles),
                        reset(use_styles),
                        tool_name,
                        danger_level
                    );
                    println!(
                        "  {}   Reason: {}{}",
                        dim(use_styles),
                        explanation,
                        reset(use_styles)
                    );
                    if let Ok(mut pending) = agent_core::permission::global_pending_approvals().lock() {
                        if let Some(tx) = pending.remove(&prompt_id) {
                            let _ = tx.send(agent_core::permission::ApprovalChoice::AllowSession);
                        }
                    }
                }

                // ── Subagent events ────────────────────────────────
                AgentEvent::SubagentStart { subagent_id, role_name, task } => {
                    pending_tool.lock().unwrap().take();
                    in_sub_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    in_sub_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "\n  {}=== Subagent '{}' ({}) Started ==={}{}\n  {}{}",
                        cyan(use_styles),
                        role_name,
                        subagent_id,
                        reset(use_styles),
                        dim(use_styles),
                        truncate(&task, 60),
                        reset(use_styles),
                    );
                }
                AgentEvent::SubagentTurnStart { turn_index, .. } => {
                    in_sub_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    in_sub_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    // Print turn line + immediate "waiting" indicator
                    print!(
                        "\n  {}│  ── Turn {turn_index} ──{}  {}⏳{}",
                        dim(use_styles),
                        reset(use_styles),
                        yellow(use_styles),
                        reset(use_styles),
                    );
                    io::stdout().flush().ok();
                }
                AgentEvent::SubagentMessageUpdate { delta, .. } => {
                    match delta {
                        MessageDelta::Thinking(t) => {
                            // Clear the ⏳ indicator on first thinking token
                            if !in_sub_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                                in_sub_text.store(false, std::sync::atomic::Ordering::Relaxed);
                                // Clear the ⏳ + newline + print prefix
                                print!("\r                                    \r");
                                print!(
                                    "  {}│  {}💭 Think{} {}",
                                    dim(use_styles),
                                    yellow(use_styles),
                                    reset(use_styles),
                                    dim(use_styles),
                                );
                                in_sub_thinking.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            print!("{}", t);
                            io::stdout().flush().ok();
                        }
                        MessageDelta::Text(t) => {
                            if in_sub_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                                println!("{}", reset(use_styles));
                                in_sub_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                            }
                            if !in_sub_text.load(std::sync::atomic::Ordering::Relaxed) {
                                println!(); // visual separator before sub-agent text
                                print!(
                                    "  {}│ {}{}",
                                    dim(use_styles),
                                    reset(use_styles),
                                    reset(use_styles),
                                );
                                in_sub_text.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            print!("{}", t);
                            io::stdout().flush().ok();
                        }
                    }
                }
                AgentEvent::SubagentToolStart {
                    tool_name, args, ..
                } => {
                    // Flush any pending thinking/text before tool line
                    if in_sub_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                        println!("{}", reset(use_styles));
                        in_sub_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                    in_sub_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    println!(); // visual separator before sub-agent tool
                    let args_str = fmt_tool_args(&args);
                    println!(
                        "  {}│  {}{}🔧{} {}({})",
                        dim(use_styles),
                        bold(use_styles),
                        cyan(use_styles),
                        reset(use_styles),
                        tool_name,
                        args_str,
                    );
                }
                AgentEvent::SubagentToolEnd {
                    tool_name,
                    result,
                    is_error,
                    ..
                } => {
                    let preview = truncate(&result, 100).replace('\n', " ");
                    if is_error {
                        println!(
                            "  {}│     {}✗{} {} {}",
                            dim(use_styles),
                            red(use_styles),
                            reset(use_styles),
                            tool_name,
                            preview,
                        );
                    } else {
                        println!(
                            "  {}│     {}→{} {} {}",
                            dim(use_styles),
                            green(use_styles),
                            reset(use_styles),
                            tool_name,
                            preview,
                        );
                    }
                }
                AgentEvent::SubagentEnd {
                    subagent_id,
                    role_name,
                    success,
                    iterations_used,
                } => {
                    in_sub_thinking.store(false, std::sync::atomic::Ordering::Relaxed);
                    in_sub_text.store(false, std::sync::atomic::Ordering::Relaxed);
                    if success {
                        println!(
                            "  {}├─ '{}' ({}){} {}✓{} ({} iterations){}",
                            yellow(use_styles),
                            role_name,
                            subagent_id,
                            reset(use_styles),
                            green(use_styles),
                            reset(use_styles),
                            iterations_used,
                            reset(use_styles),
                        );
                    } else {
                        println!(
                            "  {}├─ '{}' ({}){} {}✗{} ({} iterations){}",
                            yellow(use_styles),
                            role_name,
                            subagent_id,
                            reset(use_styles),
                            red(use_styles),
                            reset(use_styles),
                            iterations_used,
                            reset(use_styles),
                        );
                    }
                }
                _ => {} // TurnEnd, MessageStart, ToolExecutionUpdate — silently ignored
            }
        })
        .await
    {
        Ok(_answer) => {
            println!();
        }
        Err(e) => {
            eprintln!(
                "\n  {}{}✗{}{} {}{}",
                bold(use_styles),
                red(use_styles),
                reset(use_styles),
                reset(use_styles),
                e,
                reset(use_styles),
            );
        }
    }
}

fn print_help() {
    println!(
        r#"=== Agent Core CLI ===

  General
    /help              Show this help
    /status            Show agent subsystem status
    /quit, /exit       Exit (auto-saves current session)

  Model & Context
    /models            List available models
    /model <name>      Switch model
    /temp <float>      Set temperature
    /max-tokens <int>  Set max output tokens
    /tokens            Show current token count
    /context           Show message history with token breakdown
    /clear             Clear conversation context
    /new               Start a fresh session (clear + new session ID)
    /rewind            List rewindable conversation points
    /rewind <idx>      Rewind conversation to message index

  Agent Control
    /abort             Abort the current agent run
    /state             Show current agent state (Idle/Streaming/ExecutingTools/Aborted)
    /tool-mode         Show current tool execution mode
    /tool-mode <mode>  Set tool execution mode (parallel|sequential)
    /steer <message>   Inject a steering message mid-run
    /follow-up <msg>   Queue a follow-up message after agent finishes
    /clear-queues      Clear steering and follow-up queues

  Memory
    /memory            Show core memory blocks
    /memory search <q> Search conversation memory
    /memory stats      Show memory statistics (count, strength, importance)

  Permission & Hooks
    /permission        Show permission policy
    /perm test <tool> <json>   Test permission for a tool call
    /perm mode <mode>          Set mode: paranoid|standard|permissive|yolo
    /hooks             Show registered hooks

  MCP (Model Context Protocol)
    /mcp               Show connected MCP servers and discovered tools

  Planning (Todo)
    /todo              Show todo list
    /todo add <id> <desc>     Add a todo item
    /todo start <id>          Mark item in-progress
    /todo done <id>           Mark item completed
    /todo clear               Clear all items

  Task Board
    /tasks             Show task board
    /tasks add <id> <desc>    Add a task
    /tasks start <id>         Mark task in-progress
    /tasks done <id>          Mark task completed
    /tasks clear              Clear all tasks

  Sessions
    /sessions          List saved sessions
    /session save      Save current conversation
    /session resume <id>     Resume a previous session
    /session delete <id>     Delete a session
    /session rename <id> <t> Rename a session
    /session archive <id>    Archive a session
    /session search <kw>     Search sessions by title/summary

  Skills
    /skills            List available skills
    /skill <name>      Load a skill into context
    /skill active      Show currently active skills
    /skill deactivate <name|all>  Deactivate a skill
    /skill reload      Rescan skill directories

Just type a message to chat with the agent."#
    );
}
