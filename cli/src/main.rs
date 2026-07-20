#![allow(deprecated)]
mod bootstrap;
mod cli_completer;
mod oneshot;
mod state;

use agent_core::{
    ApprovalChoice, Message, MessageDelta, PermissionMode, Role, RunCommand, RunEvent,
    SkillManager, SkillManifest, TaskBoard, TaskStatus, TodoItem, TodoList, TodoStatus,
    ToolExecutionMode,
};
use bootstrap::{bootstrap_runtime, parse_permission_mode, resolve_config_path, BootstrapOptions};
use oneshot::{run_oneshot, OneshotArgs};
use state::CliState;

use argh::FromArgs;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, EditMode, Editor, Helper};
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use parking_lot::Mutex;
use std::sync::Arc;

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
    "/memory pending clear",
    "/memory pending promote",
    "/memory maintain",
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
/// CLI arguments for ageverse
#[derive(FromArgs)]
/// Ageverse agent CLI
struct Args {
    /// launch TUI mode
    #[argh(switch, short = 't')]
    tui: bool,

    /// one-shot prompt (non-interactive)
    #[argh(option, short = 'p')]
    instruction: Option<String>,

    /// model key from ~/.agverse/config.toml (e.g. hunyuan/tencent/hy3:free)
    #[argh(option)]
    model: Option<String>,

    /// permission mode: paranoid|standard|developer|permissive|yolo (oneshot default: yolo)
    #[argh(option)]
    permission: Option<String>,

    /// working directory for the agent run
    #[argh(option)]
    workdir: Option<String>,

    /// path to config.toml (default: ~/.agverse/config.toml)
    #[argh(option)]
    config: Option<String>,

    #[argh(subcommand)]
    nested: Option<SubCommand>,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Eval(EvalCommand),
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "eval")]
/// Run harness evaluation suites
struct EvalCommand {
    #[argh(subcommand)]
    nested: EvalSubCommand,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum EvalSubCommand {
    Run(EvalRunCommand),
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "run")]
/// Execute an eval suite and write a scorecard report
struct EvalRunCommand {
    /// suite name or path (e.g. contract_v1)
    #[argh(option, short = 's')]
    suite: String,

    /// mock | live
    #[argh(option, short = 'm', default = "String::from(\"mock\")")]
    mode: String,

    /// model key from config.toml (e.g. deepseek) or provider/model
    #[argh(option, default = "String::from(\"eval/mock\")")]
    model: String,

    /// path to config.toml (default: ./config.toml or EVAL_CONFIG)
    #[argh(option)]
    config: Option<String>,

    /// output directory (default: evals/out/<timestamp>)
    #[argh(option, short = 'o')]
    out: Option<String>,

    /// price table toml path
    #[argh(option)]
    price_profile: Option<String>,

    /// fail if harness_fail_rate > 0
    #[argh(switch)]
    gate: bool,

    /// permission mode override
    #[argh(option)]
    permission: Option<String>,

    /// max iterations override
    #[argh(option)]
    max_iterations: Option<u32>,

    /// harness variant label
    #[argh(option)]
    variant: Option<String>,

    /// comma-separated models for compare matrix (live)
    #[argh(option)]
    compare: Option<String>,

    /// comma-separated ablation axes: permission,compression,max_iterations
    #[argh(option)]
    ablate: Option<String>,
}


async fn run_tui_mode() -> anyhow::Result<()> {
    eprintln!("TUI mode not yet ported to CliState. Use CLI mode instead.");
    std::process::exit(1);
}

async fn run_eval_command(cmd: EvalCommand) -> anyhow::Result<()> {
    match cmd.nested {
        EvalSubCommand::Run(run) => run_eval_suite(run).await,
    }
}

async fn run_eval_suite(run: EvalRunCommand) -> anyhow::Result<()> {
    use agent_core::{
        matrix_from_summaries, resolve_suite_dir, run_suite, write_matrix, EvalMode, EvalRunOptions,
        SuiteSummary,
    };
    use chrono::Utc;

    let mode: EvalMode = run.mode.parse().map_err(anyhow::Error::msg)?;
    let suite_dir = resolve_suite_dir(&run.suite)?;
    let config_path = run.config.as_ref().map(std::path::PathBuf::from);
    let stamp = Utc::now().format("%Y%m%d_%H%M%S");
    let out_root = run
        .out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(format!("evals/out/{stamp}")));

    // Multi-model compare
    if let Some(compare) = &run.compare {
        let models: Vec<_> = compare
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let mut summaries: Vec<SuiteSummary> = Vec::new();
        for model in &models {
            let out_dir = out_root.join(model.replace('/', "_"));
            eprintln!("==> compare model={model} out={}", out_dir.display());
            let result = run_suite(EvalRunOptions {
                suite_dir: suite_dir.clone(),
                out_dir: out_dir.clone(),
                mode,
                model: model.clone(),
                config_path: config_path.clone(),
                price_profile: run.price_profile.as_ref().map(std::path::PathBuf::from),
                git_sha: None,
                variant: Some(model.clone()),
                permission_mode: run.permission.clone(),
                max_iterations: run.max_iterations,
                compression: true,
                gate_harness: false,
            })
            .await?;
            eprintln!(
                "    pass@1={:.0}% harness_fail={:.0}%",
                result.summary.pass_at_1 * 100.0,
                result.summary.harness_health.harness_fail_rate * 100.0
            );
            summaries.push(result.summary);
        }
        let matrix = matrix_from_summaries(
            suite_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("suite"),
            "model_compare",
            &summaries,
        );
        write_matrix(&matrix, &out_root)?;
        eprintln!("Wrote matrix to {}", out_root.join("matrix.md").display());
        if run.gate {
            let bad = summaries
                .iter()
                .any(|s| s.harness_health.harness_fail_rate > 0.0);
            if bad {
                anyhow::bail!("compare gate failed: harness_fail_rate > 0 for at least one model");
            }
        }
        return Ok(());
    }

    // Harness ablation
    if let Some(ablate) = &run.ablate {
        let axes: Vec<_> = ablate
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let mut summaries = Vec::new();
        let baselines = vec![
            ("baseline", run.permission.clone().unwrap_or_else(|| "yolo".into()), true, run.max_iterations.unwrap_or(20)),
        ];
        let mut variants = baselines;
        for axis in &axes {
            match axis.as_str() {
                "permission" => {
                    variants.push(("permission=standard", "standard".into(), true, run.max_iterations.unwrap_or(20)));
                    variants.push(("permission=yolo", "yolo".into(), true, run.max_iterations.unwrap_or(20)));
                }
                "compression" => {
                    variants.push(("compress=off", run.permission.clone().unwrap_or_else(|| "yolo".into()), false, run.max_iterations.unwrap_or(20)));
                }
                "max_iterations" => {
                    variants.push(("max_iter=5", run.permission.clone().unwrap_or_else(|| "yolo".into()), true, 5));
                    variants.push(("max_iter=10", run.permission.clone().unwrap_or_else(|| "yolo".into()), true, 10));
                }
                other => eprintln!("unknown ablate axis: {other}"),
            }
        }
        // dedupe by label
        variants.sort_by(|a, b| a.0.cmp(b.0));
        variants.dedup_by(|a, b| a.0 == b.0);

        for (label, perm, compress, max_iter) in variants {
            let out_dir = out_root.join(label.replace('=', "_"));
            eprintln!("==> ablate {label} out={}", out_dir.display());
            let result = run_suite(EvalRunOptions {
                suite_dir: suite_dir.clone(),
                out_dir,
                mode,
                model: run.model.clone(),
                config_path: config_path.clone(),
                price_profile: run.price_profile.as_ref().map(std::path::PathBuf::from),
                git_sha: None,
                variant: Some(label.to_string()),
                permission_mode: Some(perm),
                max_iterations: Some(max_iter),
                compression: compress,
                gate_harness: false,
            })
            .await?;
            summaries.push(result.summary);
        }
        let matrix = matrix_from_summaries(
            suite_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("suite"),
            "harness_ablation",
            &summaries,
        );
        write_matrix(&matrix, &out_root)?;
        eprintln!("Wrote ablation matrix to {}", out_root.join("matrix.md").display());
        return Ok(());
    }

    // Single run
    std::fs::create_dir_all(&out_root)?;
    let result = run_suite(EvalRunOptions {
        suite_dir,
        out_dir: out_root.clone(),
        mode,
        model: run.model,
        config_path,
        price_profile: run.price_profile.map(std::path::PathBuf::from),
        git_sha: None,
        variant: run.variant,
        permission_mode: run.permission,
        max_iterations: run.max_iterations,
        compression: true,
        gate_harness: run.gate,
    })
    .await?;

    println!(
        "Eval done: pass@1={:.0}% ({}/{}) harness_fail={:.0}%\nReport: {}",
        result.summary.pass_at_1 * 100.0,
        result.summary.n_pass,
        result.summary.n_tasks,
        result.summary.harness_health.harness_fail_rate * 100.0,
        out_root.join("report.md").display()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run_main().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_main() -> anyhow::Result<ExitCode> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let args: Args = argh::from_env();

    if let Some(SubCommand::Eval(eval)) = args.nested {
        run_eval_command(eval).await?;
        return Ok(ExitCode::SUCCESS);
    }

    if args.tui {
        run_tui_mode().await?;
        return Ok(ExitCode::SUCCESS);
    }

    // ── One-shot mode (Harbor / CI) ─────────────────────────────────
    if let Some(instruction) = args.instruction {
        return run_oneshot(OneshotArgs {
            instruction,
            model: args.model,
            permission: args.permission,
            workdir: args.workdir,
            config: args.config,
        })
        .await;
    }

    // ── Interactive REPL ────────────────────────────────────────────
    let use_styles = std::io::stdout().is_terminal();
    println!("=== Ageverse CLI ===\n");

    print!("Enable permission system? (Y/n): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let enable_permission = input.trim().to_lowercase() != "n";

    print!("Enable hook system? (Y/n): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let enable_hooks = input.trim().to_lowercase() != "n";

    print!("Tool execution mode (parallel/sequential) [parallel]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let tool_mode = match input.trim().to_lowercase().as_str() {
        "sequential" | "seq" => ToolExecutionMode::Sequential,
        _ => ToolExecutionMode::Parallel,
    };

    let permission = if enable_permission {
        None
    } else {
        Some(PermissionMode::Yolo)
    };

    let mut state = bootstrap_runtime(BootstrapOptions {
        config_path: resolve_config_path(args.config.as_deref()),
        model: args.model.clone(),
        permission,
        tool_mode,
        enable_hooks,
    })
    .await?;

    let todo_list = state.todo_list.clone();
    let task_board = state.task_board.clone();
    let skill_manager = state.skill_manager.clone();
    let mcp_mgr = state.mcp_mgr.clone();
    let session_mgr = state.session_mgr.clone();

    println!("\n--- Status ---");
    println!("Memory:      enabled");
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
    println!("Tool mode:   {:?}", state.brain.tool_execution_mode);
    println!(
        "Tools:       {}",
        state.brain.display_registry().list_names().len()
    );
    println!("Model:       {}", state.brain.current_model_name());
    println!("--------------\n");
    println!("Type /help for commands, /quit to exit\n");

    // ── Setup readline with tab-completion and history ──────────────
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();
    let mut rl = Editor::with_config(config).expect("Failed to create line editor");
    rl.set_helper(Some(CommandCompleter));
    let _ = rl.load_history(&agent_core::paths::get_cli_history_dir());

    loop {
        let input = match rl.readline("> ") {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                let _ = rl.add_history_entry(&line);
                trimmed
            }
            Err(ReadlineError::Interrupted) => {
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
                let all_messages = state.context_history.clone();
                let messages: Vec<Message> = all_messages
                    .into_iter()
                    .filter(|m| m.role != Role::System)
                    .collect();
                if !messages.is_empty() {
                    let cwd = std::env::current_dir()
                        .ok()
                        .and_then(|p| p.to_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let model = state.brain.current_model_name().to_string();
                    let mgr = &*session_mgr;
                    let current_id = state.session_id.clone();
                    if let Ok(id) = mgr.save(current_id.as_deref(), &messages, &cwd, &model) {
                        println!("Session auto-saved: {}", &id[..8]);
                    }
                }

                {
                    let mut mgr = mcp_mgr.lock().await;
                    if let Err(e) = mgr.shutdown_all().await {
                        eprintln!("MCP shutdown warning: {e}");
                    }
                }

                if let Err(e) = rl.save_history(&agent_core::paths::get_cli_history_dir()) {
                    eprintln!("History save warning: {e}");
                }

                println!("Bye!");
                break;
            }
            "/help" => {
                print_help();
            }
            "/models" => {
                let current = state.brain.current_model_name();
                println!("  * {current} (current)");
            }
            "/clear" => {
                state.context_history.clear();
                state.session_id = None;
                println!("Context cleared. New session started.");
            }
            "/new" => {
                state.context_history.clear();
                state.session_id = None;
                println!("Fresh session started. Previous context cleared.");
            }
            cmd if cmd.starts_with("/rewind") => {
                let rest = cmd.strip_prefix("/rewind").unwrap_or("").trim();
                if rest.is_empty() {
                    let msgs = state.context_history.clone();
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
                            let total = state.context_history.len();
                            state.context_history.truncate(idx);
                            let removed = total.saturating_sub(state.context_history.len());
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
                println!("Current tokens: {}", state.context_history.len() as u32 * 4);
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
                let list = todo_list.lock();
                if list.items.is_empty() {
                    println!("Todo list is empty. Use /todo add <id> <description>");
                } else {
                    println!("{}", list.to_context_string());
                }
            }
            "/tasks" => {
                let board = task_board.lock();
                println!("{}", board.summary());
            }
            "/skills" => {
                let mgr = skill_manager.lock();
                let skills = mgr.list_with_sources();
                if skills.is_empty() {
                    println!("No skills found. Searched:");
                    for dir in mgr.search_dirs() {
                        println!("  {}", dir.display());
                    }
                } else {
                    println!("=== Available Skills ===");

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
                            let desc_one_line = truncated.replace('\n', " ");
                            println!("  {}  {}", skill.name, desc_one_line);
                        }
                    }
                }
            }
            "/status" => {
                print_status(
                    &state,
                    enable_permission,
                    enable_hooks,
                    &todo_list,
                    &task_board,
                    &skill_manager,
                );
            }
            "/sessions" => {
                let mgr = &*session_mgr;
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
                let mgr = &*session_mgr;
                let args: Vec<&str> = cmd.splitn(4, ' ').collect();

                match args.get(1).copied() {
                    Some("save") => {
                        let all_messages = state.context_history.clone();
                        let messages: Vec<Message> = all_messages
                            .into_iter()
                            .filter(|m| m.role != Role::System)
                            .collect();
                        let cwd = std::env::current_dir()
                            .ok()
                            .and_then(|p| p.to_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let model = state.brain.current_model_name().to_string();
                        let current_id = state.session_id.clone();
                        match mgr.save(current_id.as_deref(), &messages, &cwd, &model) {
                            Ok(id) => {
                                state.session_id = Some(id.clone());
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
                                    state.context_history.clear();
                                    state.session_id = None;
                                    for msg in &session.messages {
                                        state.context_history.extend_from_slice(&[msg.clone()]);
                                    }
                                    state.session_id = Some(session_id.to_string());
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
                if let Some(ref rid) = state.current_run_id {
                    let _ = state.run_manager.cancel_run(rid);
                };
                println!("Abort signal sent. The agent will stop at the next opportunity.");
            }
            "/state" => {
                println!(
                    "Agent state: {:?}",
                    state
                        .current_run_id
                        .as_ref()
                        .map(|_| "Running")
                        .unwrap_or("Idle")
                );
            }
            "/tool-mode" => {
                println!("Tool execution mode: {:?}", state.brain.tool_execution_mode);
            }
            "/clear-queues" => {
                println!("Steering and follow-up queues cleared.");
            }
            cmd if cmd.starts_with("/model ") => {
                let name = cmd.strip_prefix("/model ").unwrap().trim();
                match state.run_manager.switch_model(name) {
                    Ok(()) => {
                        state.brain = (**state.run_manager.brain()).clone();
                        println!("Switched to model: {name}");
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }
            }
            cmd if cmd.starts_with("/temp ") => {
                let val_str = cmd.strip_prefix("/temp ").unwrap().trim();
                match val_str.parse::<f64>() {
                    Ok(val) => {
                        state.brain.set_temperature(val);
                        println!("Temperature set to {val}");
                    }
                    Err(_) => eprintln!("Invalid temperature value"),
                }
            }
            cmd if cmd.starts_with("/max-tokens ") => {
                let val_str = cmd.strip_prefix("/max-tokens ").unwrap().trim();
                match val_str.parse::<u32>() {
                    Ok(val) => {
                        state.brain.set_max_tokens(val);
                        println!("Max tokens set to {val}");
                    }
                    Err(_) => eprintln!("Invalid max-tokens value"),
                }
            }
            cmd if cmd.starts_with("/tool-mode ") => {
                let mode_str = cmd.strip_prefix("/tool-mode ").unwrap().trim();
                match mode_str.to_lowercase().as_str() {
                    "parallel" | "par" => {
                        state.brain.set_tool_execution_mode(ToolExecutionMode::Parallel);
                        println!("Tool execution mode set to: parallel");
                    }
                    "sequential" | "seq" => {
                        state.brain.set_tool_execution_mode(ToolExecutionMode::Sequential);
                        println!("Tool execution mode set to: sequential");
                    }
                    _ => {
                        eprintln!("Usage: /tool-mode <parallel|sequential>");
                    }
                }
            }
            cmd if cmd.starts_with("/steer ") => {
                let msg = cmd.strip_prefix("/steer ").unwrap().trim();
                if let Some(ref rid) = state.current_run_id {
                    let _ = state.run_manager.command(
                        rid,
                        RunCommand::Steer {
                            steer_id: format!(
                                "steer-{}",
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_nanos()
                            ),
                            message: msg.to_string(),
                        },
                    );
                }
                println!("Steering message queued. It will be injected after the current turn.");
            }
            cmd if cmd.starts_with("/follow-up ") => {
                let msg = cmd.strip_prefix("/follow-up ").unwrap().trim();
                if let Some(ref rid) = state.current_run_id {
                    let _ = state.run_manager.command(
                        rid,
                        RunCommand::FollowUp {
                            message: msg.to_string(),
                        },
                    );
                }
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
                    let mut mgr = skill_manager.lock();
                    if name == "all" {
                        mgr.deactivate_all();
                        println!("All skills deactivated.");
                    } else if mgr.deactivate(name) {
                        println!("Skill '{}' deactivated.", name);
                    } else {
                        eprintln!("Skill '{}' is not active.", name);
                    }
                } else if rest == "reload" {
                    let mut mgr = skill_manager.lock();
                    match mgr.scan() {
                        Ok(count) => println!("Reloaded {} skills from disk.", count),
                        Err(e) => eprintln!("Reload failed: {e}"),
                    }
                } else if rest == "active" {
                    let mgr = skill_manager.lock();
                    let active = mgr.active_skill_names();
                    if active.is_empty() {
                        println!("No active skills.");
                    } else {
                        println!("Active skills: {}", active.join(", "));
                    }
                } else {
                    let mgr = skill_manager.lock();
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
                    let decision = state.brain.build_permission_policy().check(
                        tool_name,
                        tool_input,
                        None,
                        None,
                        None,
                    );
                    println!(
                        "Permission check: {}({}) -> {:?}",
                        tool_name, tool_input, decision
                    );
                } else if args.starts_with("mode ") {
                    let mode_str = args.strip_prefix("mode ").unwrap().trim();
                    match parse_permission_mode(mode_str) {
                        Ok(mode) => {
                            state.brain.build_permission_policy().set_mode(mode);
                            println!("Permission mode set to: {:?}", mode);
                        }
                        Err(e) => eprintln!("{e}"),
                    }
                } else {
                    eprintln!("Usage:");
                    eprintln!("  /perm test <tool_name> <input_json>");
                    eprintln!("  /perm mode <paranoid|standard|developer|permissive|yolo>");
                }
            }
            "/context" => {
                let msgs = state.context_history.clone();
                let tokens = state.context_history.len() as u32 * 4;
                println!("=== Context ({tokens} tokens, {} messages) ===", msgs.len());
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
                    if let Some(ref memory) = state.brain.memory {
                        let mem = memory.lock();
                        match mem.search_conversation(query, 5) {
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
                    if let Some(ref memory) = state.brain.memory {
                        let mem = memory.lock();
                        match mem.stats() {
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
                } else if rest == "pending clear" {
                    let path = agent_core::paths::get_global_agverse_md_path();
                    match agent_core::memory::agverse_md::clear_pending_notes_file(&path) {
                        Ok(n) => println!("Cleared {n} pending note(s) from {}", path.display()),
                        Err(e) => eprintln!("Clear pending failed: {e}"),
                    }
                } else if rest == "pending promote" {
                    let path = agent_core::paths::get_global_agverse_md_path();
                    match agent_core::memory::agverse_md::promote_pending_notes_file(&path) {
                        Ok(n) => println!("Promoted {n} pending note(s) in {}", path.display()),
                        Err(e) => eprintln!("Promote pending failed: {e}"),
                    }
                } else if rest == "maintain" {
                    let path = agent_core::paths::get_global_agverse_md_path();
                    match agent_core::memory::agverse_md::maintain_agverse_file(&path) {
                        Ok(r) => println!(
                            "Maintained {}: expired={}, trimmed={}, sections_ensured={}",
                            path.display(),
                            r.pending_expired,
                            r.trimmed_bullets,
                            r.sections_ensured
                        ),
                        Err(e) => eprintln!("Maintain failed: {e}"),
                    }
                } else {
                    eprintln!(
                        "Usage: /memory search <query> | /memory stats | /memory pending clear | /memory pending promote | /memory maintain"
                    );
                }
            }
            "/memory" => {
                if let Some(ref memory) = state.brain.memory {
                    let mem = memory.lock();
                    println!("=== Core Memory ===");
                    for block in mem.core().list() {
                        println!("[{}]: {}", block.id, block.content);
                    }
                    println!("\nSession: {}", mem.session_id());
                } else {
                    println!("Memory is disabled.");
                }
            }
            input if input.starts_with('/') => {
                eprintln!(
                    "Unknown command: {}. Type /help for available commands.",
                    input
                );
            }
            _ => {
                run_agent(&mut state, &input, use_styles).await;
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn handle_todo_cmd(cmd: &str, todo_list: &Arc<Mutex<TodoList>>) {
    let args = cmd.strip_prefix("/todo ").unwrap().trim();

    if args == "clear" {
        let mut list = todo_list.lock();
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
        let mut list = todo_list.lock();
        list.add(TodoItem::new(id, desc));
        println!("Added todo '{}': {}", id, desc);
        return;
    }

    if args.starts_with("done ") {
        let id = args.strip_prefix("done ").unwrap().trim();
        let mut list = todo_list.lock();
        match list.update_status(id, TodoStatus::Completed) {
            Ok(()) => println!("Todo '{}' marked done.", id),
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    if args.starts_with("start ") {
        let id = args.strip_prefix("start ").unwrap().trim();
        let mut list = todo_list.lock();
        match list.update_status(id, TodoStatus::InProgress) {
            Ok(()) => println!("Todo '{}' started.", id),
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    // Default: show list
    let list = todo_list.lock();
    if list.items.is_empty() {
        println!("Todo list is empty. Use /todo add <id> <description>");
    } else {
        println!("{}", list.to_context_string());
    }
}

fn handle_tasks_cmd(cmd: &str, task_board: &Arc<Mutex<TaskBoard>>) {
    let args = cmd.strip_prefix("/tasks ").unwrap().trim();

    if args == "clear" {
        let mut board = task_board.lock();
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
        let mut board = task_board.lock();
        board.create(id, desc, vec![]);
        println!("Task '{}' created: {}", id, desc);
        return;
    }

    if args.starts_with("done ") {
        let id = args.strip_prefix("done ").unwrap().trim();
        let mut board = task_board.lock();
        match board.update(id, TaskStatus::Completed, None) {
            Ok(()) => println!("Task '{}' completed.", id),
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    if args.starts_with("start ") {
        let id = args.strip_prefix("start ").unwrap().trim();
        let mut board = task_board.lock();
        match board.update(id, TaskStatus::InProgress, None) {
            Ok(()) => println!("Task '{}' started.", id),
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    // Default: show board
    let board = task_board.lock();
    println!("{}", board.summary());
}

fn print_status(
    state: &CliState,
    enable_permission: bool,
    enable_hooks: bool,
    todo_list: &Arc<Mutex<TodoList>>,
    task_board: &Arc<Mutex<TaskBoard>>,
    skill_manager: &Arc<Mutex<SkillManager>>,
) {
    println!("=== Agent Status ===");
    println!("Model:       {}", state.brain.current_model_name().to_string());
    println!("State:       {:?}", state.current_run_id.as_ref().map(|_| "Running").unwrap_or("Idle"));
    println!("Tool mode:   {:?}", state.brain.tool_execution_mode);
    println!("Tokens:      {}", state.context_history.len() as u32 * 4);
    println!("Memory:      {}", if state.brain.memory.is_some() { "on" } else { "off" });
    println!(
        "Permission:  {}",
        if enable_permission { "on" } else { "off" }
    );
    println!("Hooks:       {}", if enable_hooks { "on" } else { "off" });
    println!("Tools:       {}", state.brain.display_registry().list_names().len());
    {
        let list = todo_list.lock();
        println!("Todo:        {}", list.summary());
    }
    {
        let board = task_board.lock();
        println!("Tasks:       {} total", board.all_tasks().len());
    }
    {
        let mgr = skill_manager.lock();
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
    state: &mut CliState,
    input: &str,
    use_styles: bool,
) {
    let session_id = state.session_id.clone();
    let history = std::mem::take(&mut state.context_history);
    let created = match state.run_manager.create_run(input, session_id.clone(), history).await {
        Ok(result) => result,
        Err(e) => { eprintln!("Error creating run: {e}"); return; }
    };
    let run_id = created.run_id;
    // Canonical prompt id from the shared backend (same as Tauri).
    let _prompt_id = created.prompt_id;
    state.current_run_id = Some(run_id.clone());
    if let Err(e) = state.run_manager.command(&run_id, RunCommand::Start).await {
        eprintln!("Error starting run: {e}");
        state.current_run_id = None;
        return;
    }
    let mut event_rx = match state.run_manager.subscribe(&run_id).await {
        Ok(rx) => rx,
        Err(e) => { eprintln!("Error subscribing: {e}"); state.current_run_id = None; return; }
    };
    let in_thinking = std::sync::atomic::AtomicBool::new(false);
    let in_agent_text = std::sync::atomic::AtomicBool::new(false);
    let first_event = std::sync::atomic::AtomicBool::new(true);
    print!("\r  ⏳{}", reset(use_styles));
    io::stdout().flush().ok();
    loop {
        match event_rx.recv().await {
            Ok(envelope) => {
                if first_event.load(std::sync::atomic::Ordering::Relaxed) {
                    print!("\r                                    \r");
                    io::stdout().flush().ok();
                    first_event.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                match envelope.event {
                    RunEvent::TurnStarted { index } => {
                        if index > 1 {
                            println!("\n  {}─── Turn {index} ───{}", dim(use_styles), reset(use_styles));
                        }
                    }
                    RunEvent::ModelStreaming { delta, .. } => match delta {
                        MessageDelta::Text(t) => {
                            if !in_agent_text.load(std::sync::atomic::Ordering::Relaxed) {
                                println!();
                                print!("  {}{}>>{} {}", bold(use_styles), green(use_styles), reset(use_styles), reset(use_styles));
                                in_agent_text.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            print!("{}{}", t, reset(use_styles));
                            io::stdout().flush().ok();
                        }
                        MessageDelta::Thinking(t) => {
                            if !in_thinking.load(std::sync::atomic::Ordering::Relaxed) {
                                println!();
                                print!("  {}{}💭 Think{} {}", bold(use_styles), yellow(use_styles), reset(use_styles), dim(use_styles));
                                in_thinking.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            print!("{}{}{}", dim(use_styles), t, reset(use_styles));
                            io::stdout().flush().ok();
                        }
                    },
                    RunEvent::ToolEnded { name, result, .. } => {
                        let r = result;
                        if !r.is_empty() {
                            print_tool_line(&name, &serde_json::Value::Null, &r, false, use_styles);
                        }
                    }
                    RunEvent::ApprovalRequired { prompt_id, tool_name, tool_input: _, explanation, .. } => {
                        println!();
                        println!("  {}⚠  Approval required:{}", yellow(use_styles), reset(use_styles));
                        println!("  Tool: {}\n  Reason: {}", tool_name, explanation);
                        print!("  Allow? [y/N]: ");
                        io::stdout().flush().ok();
                        let mut answer = String::new();
                        if io::stdin().read_line(&mut answer).is_ok() {
                            let choice = if answer.trim().to_lowercase() == "y" {
                                ApprovalChoice::AllowOnce
                            } else {
                                ApprovalChoice::Deny
                            };
                            let _ = state.run_manager.command(&run_id, RunCommand::Approve { prompt_id, choice });
                        }
                    }
                    RunEvent::RunCompleted { .. } => { println!("\n"); break; }
                    RunEvent::RunCancelled { .. } => {
                        println!("\n  {}⏹  Run cancelled.{}", dim(use_styles), reset(use_styles));
                        break;
                    }
                    RunEvent::RunFailed { error } => {
                        eprintln!("\n  Error: {error}");
                        break;
                    }
                    _ => {}
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("  (skipped {n} events)");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
    state.context_history = Vec::new();
    state.current_run_id = None;
}

fn print_help() {
    println!(
        r#"=== Ageverse CLI ===

  Usage (one-shot / Harbor):
    ageverse -p "instruction" --model <key> --permission yolo
    ageverse --instruction "..." --model hunyuan/tencent/hy3:free

  Config: ~/.agverse/config.toml (same as desktop app)

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
    /memory pending clear    Discard Pending Notes from ~/.agverse/agverse.md
    /memory pending promote  Move tagged Pending Notes into standard sections
    /memory maintain         Expire old Pending + trim oversized agverse.md

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

Just type a message to chat with the state."#
    );
}
