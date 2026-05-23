use agent_core::{
    AgentBuilder, AgentEvent, PermissionPolicy,
    TodoList, TodoItem, TodoStatus, TaskBoard, TaskStatus, SkillLoader,
    hooks::LoggingHook,
    tools, tasks,
};
use std::cell::Cell;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut builder = match AgentBuilder::from_config("config.toml") {
        Ok(b) => b,
        Err(e) => {
            eprintln!("config.toml load failed: {e}");
            AgentBuilder::from_env()?
        }
    };

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
        let tool_names: Vec<String> = agent.tool_registry().list_names().iter().map(|s| s.to_string()).collect();
        let reg = agent.tool_registry_mut();
        tools::subagent::register_subagent_tools(reg, model_config, tool_names);
    }

    // Print status
    println!("\n--- Status ---");
    println!("Memory:      {}", if enable_memory { "enabled" } else { "disabled" });
    println!("Permission:  {}", if enable_permission { "enabled" } else { "disabled" });
    println!("Hooks:       {}", if enable_hooks { "enabled" } else { "disabled" });
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
                println!("Permission system: {}", if enable_permission { "active" } else { "disabled" });
                println!("Rules are checked before each tool execution.");
                println!("Use /perm test <tool> <input> to check a specific call.");
            }
            "/hooks" => {
                println!("=== Hook Registry ===");
                println!("Hook system: {}", if enable_hooks { "active" } else { "disabled" });
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
                        println!("  {}: {}", skill.name, skill.description);
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
                println!("=== Agent Status ===");
                println!("Model:       {}", agent.current_model());
                println!("Tokens:      {}", agent.context_token_count());
                println!("Memory:      {}", if enable_memory { "on" } else { "off" });
                println!("Permission:  {}", if enable_permission { "on" } else { "off" });
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
                    println!("Permission check: {}({}) -> {:?}", tool_name, tool_input, decision);
                } else {
                    eprintln!("Usage: /perm test <tool_name> <input_json>");
                }
            }
            _ => {
                run_agent(&mut agent, input).await;
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
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    if args.starts_with("start ") {
        let id = args.strip_prefix("start ").unwrap().trim();
        let mut board = task_board.lock().unwrap();
        match board.update(id, TaskStatus::InProgress, None) {
            Ok(()) => println!("Task '{}' started.", id),
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    // Default: show board
    let board = task_board.lock().unwrap();
    println!("{}", board.summary());
}

async fn run_agent(agent: &mut agent_core::Agent, input: &str) {
    print!("\rThinking...");
    io::stdout().flush().ok();

    let first_event = Cell::new(true);
    let in_thinking = Cell::new(false);
    match agent
        .run_with_events(input, |event| {
            if first_event.get() {
                print!("\r                              \r");
                io::stdout().flush().ok();
                first_event.set(false);
            }
            match event {
                AgentEvent::Thinking(t) => {
                    if !in_thinking.get() {
                        print!("\nThinking: ");
                        in_thinking.set(true);
                    }
                    print!("{t}");
                    io::stdout().flush().ok();
                }
                AgentEvent::Thought(t) => {
                    if in_thinking.get() {
                        println!();
                        in_thinking.set(false);
                    }
                    print!("{t}");
                    io::stdout().flush().ok();
                }
                AgentEvent::ToolStart(name) => {
                    if in_thinking.get() {
                        println!();
                        in_thinking.set(false);
                    }
                    if name.starts_with("[APPROVAL") {
                        print!("\n  {name}");
                    } else {
                        print!("\n  [{name}]");
                    }
                    io::stdout().flush().ok();
                }
                AgentEvent::ToolResult(r) => {
                    let preview = if r.len() > 120 {
                        format!("{}...", &r[..120])
                    } else {
                        r
                    };
                    print!(" -> {preview}");
                    io::stdout().flush().ok();
                }
                AgentEvent::FinalAnswer(_) => {}
                AgentEvent::Error(e) => eprintln!("\n  Error: {e}"),
            }
        })
        .await
    {
        Ok(answer) => {
            println!("\n\n{answer}");
        }
        Err(e) => {
            eprintln!("\n  Error: {e}");
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
