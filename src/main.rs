use agent_core::{
    AgentBuilder, AgentEvent, Message, MessageDelta, PermissionPolicy, SkillLoader, TaskBoard,
    TaskStatus, TodoItem, TodoList, TodoStatus, ToolExecutionMode, hooks::LoggingHook, tasks,
    tools,
};
use std::cell::Cell;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

// ── Terminal styling ───────────────────────────────────────────────

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
#[tokio::main]
async fn main() -> anyhow::Result<()> {
     let mut builder = match AgentBuilder::from_config("config.toml") {
         Ok(b) => b,
        Err(e) => {
            eprintln!("config.toml: {e}");
            eprintln!("Falling back to OPENAI_API_KEY environment variable...");
            AgentBuilder::from_env()?
        }
     };

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

    let mut agent = builder.build()?;

    // Optional subsystems
    let todo_list: Arc<Mutex<TodoList>> = Arc::new(Mutex::new(TodoList::new()));
    let task_board: Arc<Mutex<TaskBoard>> = Arc::new(Mutex::new(TaskBoard::new()));
    let mut skill_loader = SkillLoader::with_defaults();
    let _ = skill_loader.scan();
    let skill_loader = Arc::new(Mutex::new(skill_loader));

    // Register todo, skill, task, and subagent tools
    {
        let model_config = agent.current_model_config().clone();
        let reg = agent.tool_registry_mut();
        tools::todo::register_todo_tools(reg, todo_list.clone());
        tools::skill::register_skill_tools(reg, skill_loader.clone());
        let task_board_clone = task_board.clone();
        tasks::register_task_tools(reg, task_board_clone, model_config);
    }

    // Register subagent tool (needs model config + tool names)
    {
        let model_config = agent.current_model_config().clone();
        let tool_names: Vec<String> = agent
            .tool_registry()
            .list_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let reg = agent.tool_registry_mut();
        tools::subagent::register_subagent_tools(reg, model_config, tool_names);
    }

    // Share abort_flag so CLI can abort mid-run
    let abort_flag = agent.abort_flag.clone();

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

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        match input {
            "/quit" | "/exit" => {
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
                println!("Context cleared. New session started.");
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
                let loader = skill_loader.lock().unwrap();
                let skills = loader.list_with_sources();
                if skills.is_empty() {
                    println!("No skills found. Searched:");
                    for dir in loader.search_dirs() {
                        println!("  {}", dir.display());
                    }
                } else {
                    println!("=== Available Skills ===");
                    for (skill, source) in &skills {
                        let preview: String = skill
                            .description
                            .split_whitespace()
                            .take(50)
                            .collect::<Vec<_>>()
                            .join(" ");
                        let truncated = if skill.description.split_whitespace().count() > 50 {
                            format!("{}...", preview)
                        } else {
                            preview
                        };
                        println!("  {}: {}", skill.name, truncated);
                        println!("    source: {}", source.display());
                        if !skill.triggers.is_empty() {
                            println!("    triggers: {}", skill.triggers.join(", "));
                        }
                    }
                    println!("\nSearch dirs:");
                    for dir in loader.search_dirs() {
                        let exists = if dir.exists() { "ok" } else { "missing" };
                        println!("  {} [{}]", dir.display(), exists);
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
                    &skill_loader,
                );
            }
            "/abort" => {
                abort_flag.store(true, Ordering::Relaxed);
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
                let name = cmd.strip_prefix("/skill ").unwrap().trim();
                let loader = skill_loader.lock().unwrap();
                match loader.load_skill_context(name) {
                    Ok(Some(content)) => {
                        println!("{}", content);
                    }
                    Ok(None) => {
                        eprintln!("Skill '{}' not found. Use /skills to list.", name);
                    }
                    Err(e) => eprintln!("Error loading skill: {e}"),
                }
            }
            cmd if cmd.starts_with("/perm ") => {
                let args = cmd.strip_prefix("/perm ").unwrap().trim();
                if args.starts_with("test ") {
                    let rest = args.strip_prefix("test ").unwrap().trim();
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    let tool_name = parts[0];
                    let tool_input = parts.get(1).unwrap_or(&"{}");
                    let decision = agent.permission_policy().check(tool_name, tool_input);
                    println!(
                        "Permission check: {}({}) -> {:?}",
                        tool_name, tool_input, decision
                    );
                } else {
                    eprintln!("Usage: /perm test <tool_name> <input_json>");
                }
            }
            _ => {
                run_agent(&mut agent, input, use_styles).await;
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
    skill_loader: &Arc<Mutex<SkillLoader>>,
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
        let loader = skill_loader.lock().unwrap();
        println!("Skills:      {} loaded", loader.list().len());
    }
}

async fn run_agent(agent: &mut agent_core::Agent, input: &str, use_styles: bool) {
    print!(
        "\r  {}{}...{}{}",
        dim(use_styles),
        bold(use_styles),
        reset(use_styles),
        reset(use_styles)
    );
    io::stdout().flush().ok();

     let first_event = Cell::new(true);
     let in_thinking = Cell::new(false);
     let in_agent_text = Cell::new(false);
    let skin = termimad::MadSkin::default();
     match agent
        .run_with_events(input, |event| {
            if first_event.get() {
                print!("\r                                    \r");
                io::stdout().flush().ok();
                first_event.set(false);
            }
            match event {
                AgentEvent::AgentStart => {}
                AgentEvent::AgentEnd { .. } => {}
                AgentEvent::TurnStart { turn_index } => {
                    in_thinking.set(false);
                    in_agent_text.set(false);
                    if turn_index > 0 {
                        println!();
                        println!("  {}{}-- Turn {turn_index} --{}", bold(use_styles), reset(use_styles), reset(use_styles));
                    }
                }
                AgentEvent::TurnEnd { .. } => {
                    in_thinking.set(false);
                    in_agent_text.set(false);
                }
                AgentEvent::MessageStart { .. } => {}
                AgentEvent::MessageUpdate { delta } => match delta {
                    MessageDelta::Thinking(t) => {
                        if in_agent_text.get() {
                            println!();
                            in_agent_text.set(false);
                        }
                        if !in_thinking.get() {
                            print!("\n  {}{}...{}{} ", bold(use_styles), yellow(use_styles), dim(use_styles), reset(use_styles));
                            in_thinking.set(true);
                        }
                        print!("{}{}{}", dim(use_styles), t, reset(use_styles));
                        io::stdout().flush().ok();
                    }
                    MessageDelta::Text(t) => {
                        if in_thinking.get() {
                            println!("{}", reset(use_styles));
                            in_thinking.set(false);
                        }
                        if !in_agent_text.get() {
                            print!("  {}{}>> {}{}", bold(use_styles), green(use_styles), reset(use_styles), reset(use_styles));
                            in_agent_text.set(true);
                        }
                        if use_styles {
                            skin.print_inline(&t);
                        } else {
                            print!("{t}");
                        }
                        io::stdout().flush().ok();
                    }
                },
                AgentEvent::MessageEnd { message } => {
                    if in_thinking.get() {
                        println!("{}", reset(use_styles));
                        in_thinking.set(false);
                    }
                    if let Some(ref tool_calls) = message.tool_calls {
                        if in_agent_text.get() {
                            println!();
                            in_agent_text.set(false);
                        }
                        for tc in tool_calls {
                            let args_str = tc.function.arguments.clone();
                            println!("  {}{}@@ {}{}{}({}{}{}){}{}", bold(use_styles), cyan(use_styles), reset(use_styles), bold(use_styles), tc.function.name, reset(use_styles), dim(use_styles), truncate(&args_str, 80), reset(use_styles), reset(use_styles));
                        }
                    }
                }
                AgentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => {
                    if in_thinking.get() {
                        println!("{}", reset(use_styles));
                        in_thinking.set(false);
                    }
                    if in_agent_text.get() {
                        println!();
                        in_agent_text.set(false);
                    }
                    if tool_name.starts_with("[APPROVAL") {
                        println!("  {}{}!! {}{}{}", bold(use_styles), red(use_styles), reset(use_styles), tool_name, reset(use_styles));
                    } else {
                        let args_str = args.to_string();
                        println!("  {}{}@@ {}{}{}({}{}{}){}{}", bold(use_styles), cyan(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), dim(use_styles), truncate(&args_str, 80), reset(use_styles), reset(use_styles));
                    }
                    io::stdout().flush().ok();
                }
                AgentEvent::ToolExecutionUpdate {
                    tool_name,
                    partial_result,
                    ..
                } => {
                    println!("     {}{}.. {}{}{} {}{}{}{}", dim(use_styles), cyan(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), dim(use_styles), truncate(&partial_result, 80), reset(use_styles));
                    io::stdout().flush().ok();
                }
                AgentEvent::ToolExecutionEnd {
                    tool_name,
                    result,
                    is_error,
                    ..
                } => {
                    if is_error {
                        println!("     {}{}XX {}{}{} {}{}{}{}", bold(use_styles), red(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), red(use_styles), truncate(&result, 120), reset(use_styles));
                    } else {
                        println!("     {}{}>> {}{}{} {}{}{}{}", bold(use_styles), green(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), dim(use_styles), truncate(&result, 120), reset(use_styles));
                    }
                    io::stdout().flush().ok();
                }
                AgentEvent::Error(e) => {
                    eprintln!("  {}{}XX {}{}{}{}", bold(use_styles), red(use_styles), reset(use_styles), bold(use_styles), e, reset(use_styles));
                }

                // -- Subagent events --
                AgentEvent::SubagentStart { subagent_id, task } => {
                    println!();
                    println!("  {}{}|-{} Sub-agent {}{}'{subagent_id}'{}: {}{}{}", bold(use_styles), yellow(use_styles), reset(use_styles), bold(use_styles), yellow(use_styles), reset(use_styles), dim(use_styles), truncate(&task, 60), reset(use_styles));
                }
                AgentEvent::SubagentTurnStart {
                    subagent_id: _,
                    turn_index,
                } => {
                    println!("  {}{}|{}  {}{}-- Turn {turn_index} --{}", bold(use_styles), yellow(use_styles), reset(use_styles), dim(use_styles), reset(use_styles), reset(use_styles));
                }
                AgentEvent::SubagentMessageUpdate { subagent_id: _, delta } => {
                    match delta {
                        MessageDelta::Thinking(t) => {
                            print!("  {}{}|{}  {}{}... {}{}{}", bold(use_styles), yellow(use_styles), reset(use_styles), dim(use_styles), reset(use_styles), dim(use_styles), t, reset(use_styles));
                        }
                        MessageDelta::Text(t) => {
                            print!("  {}{}|{}  {}{}>> {}", bold(use_styles), yellow(use_styles), reset(use_styles), green(use_styles), reset(use_styles), reset(use_styles));
                            if use_styles {
                                skin.print_inline(&t);
                            } else {
                                print!("{t}");
                            }
                        }
                    }
                    io::stdout().flush().ok();
                }
                AgentEvent::SubagentToolStart {
                    subagent_id: _,
                    tool_name,
                    args,
                    ..
                } => {
                    let args_str = args.to_string();
                    println!("  {}{}|{}  {}{}@@ {}{}{}({}{}{}){}{}", bold(use_styles), yellow(use_styles), reset(use_styles), bold(use_styles), cyan(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), dim(use_styles), truncate(&args_str, 60), reset(use_styles), reset(use_styles));
                }
                AgentEvent::SubagentToolEnd {
                    subagent_id: _,
                    tool_name,
                    result,
                    is_error,
                    ..
                } => {
                    let preview = truncate(&result, 100);
                    if is_error {
                        println!("  {}{}|{}{}  {}{}XX {}{}{} {}{}{}{}", bold(use_styles), yellow(use_styles), reset(use_styles), reset(use_styles), bold(use_styles), red(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), red(use_styles), preview, reset(use_styles));
                    } else {
                        println!("  {}{}|{}{}  {}{}>> {}{}{} {}{}{}{}", bold(use_styles), yellow(use_styles), reset(use_styles), reset(use_styles), bold(use_styles), green(use_styles), reset(use_styles), bold(use_styles), tool_name, reset(use_styles), dim(use_styles), preview, reset(use_styles));
                    }
                }
                AgentEvent::SubagentEnd {
                    subagent_id,
                    success,
                    iterations_used,
                } => {
                    let (_status, icon, color): (&str, &str, fn(bool) -> &'static str) = if success {
                        ("done", "ok", green as fn(bool) -> &'static str)
                    } else {
                        ("incomplete", "err", red as fn(bool) -> &'static str)
                    };
                    println!("  {}{}|-{} Sub-agent {}{}'{subagent_id}'{}{}: {}{} {}{}({iterations_used} iterations){}{}", bold(use_styles), yellow(use_styles), reset(use_styles), bold(use_styles), yellow(use_styles), reset(use_styles), color(use_styles), bold(use_styles), icon, reset(use_styles), dim(use_styles), reset(use_styles), reset(use_styles));
                }
            }
        })
        .await
    {
        Ok(_answer) => {
            println!();
        }
        Err(e) => {
            eprintln!("\n  {}{}XX {}{}{}{}", bold(use_styles), red(use_styles), reset(use_styles), bold(use_styles), e, reset(use_styles));
        }
    }
}


fn print_help() {
    println!(
        r#"=== Agent Core CLI ===

  General
    /help              Show this help
    /status            Show agent subsystem status
    /quit, /exit       Exit

  Model & Context
    /models            List available models
    /model <name>      Switch model
    /temp <float>      Set temperature
    /max-tokens <int>  Set max output tokens
    /tokens            Show current token count
    /clear             Clear conversation context

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

  Permission & Hooks
    /permission        Show permission policy
    /perm test <tool> <json>   Test permission for a tool call
    /hooks             Show registered hooks

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

  Skills
    /skills            List available skills
    /skill <name>      Load a skill into context

Just type a message to chat with the agent."#
    );
}
