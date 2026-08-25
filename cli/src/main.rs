#![allow(deprecated)]
mod bootstrap;
mod commands;
mod completion;
mod config_cmd;
mod dry_run;
mod oneshot;
mod state;
mod tui;
mod workflow_authoring;

use agent_core::{
    ApprovalChoice, Message, MessageDelta, PermissionMode, Role, RunCommand, RunEvent,
    ToolExecutionMode,
};
use bootstrap::{
    bootstrap_runtime, parse_permission_mode, parse_tool_mode, resolve_config_path, BootstrapOptions,
};
use oneshot::{run_oneshot, OneshotArgs};
use state::CliState;

use anyhow::Context;
use argh::FromArgs;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, EditMode, Editor, Helper};
use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;

/// Hard cap for piped stdin to avoid OOM on accidental huge redirects.
const STDIN_MAX_BYTES: usize = 1024 * 1024;

// ── Tab completion ────────────────────────────────────────────────

/// Slash completion uses `commands::ALL_COMMANDS` (single source of truth).

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

        let mut matches: Vec<String> = commands::ALL_COMMANDS
            .iter()
            .map(|(cmd, _)| *cmd)
            .filter(|cmd| cmd.starts_with(prefix) && *cmd != prefix)
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

    /// enable logging hooks in REPL (default: off unless --interactive-setup)
    #[argh(switch)]
    hooks: bool,

    /// tool execution mode for REPL: parallel|sequential (default: parallel)
    #[argh(option)]
    tool_mode: Option<String>,

    /// ask permission/hooks/tool-mode interactively at REPL startup
    #[argh(switch)]
    interactive_setup: bool,

    /// oneshot: call LLM but veto all tool side effects
    #[argh(switch)]
    dry_run: bool,

    /// print version information and exit
    #[argh(switch, short = 'V')]
    version: bool,

    #[argh(subcommand)]
    nested: Option<SubCommand>,
}

/// Read piped stdin with a byte cap. Returns `None` when content is empty/whitespace-only.
fn read_piped_stdin() -> anyhow::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut stdin = io::stdin().lock();
    loop {
        let n = stdin
            .read(&mut chunk)
            .context("failed to read stdin")?;
        if n == 0 {
            break;
        }
        if buf.len().saturating_add(n) > STDIN_MAX_BYTES {
            anyhow::bail!("stdin exceeds {STDIN_MAX_BYTES} byte limit");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    if buf.is_empty() {
        return Ok(None);
    }
    let content = String::from_utf8(buf).context("stdin is not valid UTF-8")?;
    if content.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(content))
    }
}

/// Merge `-p` with optional piped stdin. `None` means enter REPL.
fn resolve_instruction(flag: Option<String>) -> anyhow::Result<Option<String>> {
    let stdin_is_tty = io::stdin().is_terminal();
    let piped = if stdin_is_tty {
        None
    } else {
        read_piped_stdin()?
    };
    merge_instruction(flag, piped, stdin_is_tty)
}

/// Pure merge used by [`resolve_instruction`] (and tests).
fn merge_instruction(
    flag: Option<String>,
    piped: Option<String>,
    stdin_is_tty: bool,
) -> anyhow::Result<Option<String>> {
    match (flag, piped, stdin_is_tty) {
        (Some(prompt), Some(stdin), _) => {
            let mut instruction = prompt;
            if !instruction.is_empty() {
                instruction.push_str("\n\n");
            }
            instruction.push_str(&stdin);
            Ok(Some(instruction))
        }
        (Some(prompt), None, _) => Ok(Some(prompt)),
        (None, Some(stdin), _) => Ok(Some(stdin)),
        (None, None, false) => anyhow::bail!(
            "stdin is not a terminal and no instruction provided; pass -p or pipe content"
        ),
        (None, None, true) => Ok(None),
    }
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Eval(EvalCommand),
    Config(ConfigCommand),
    Completion(CompletionCommand),
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "config")]
/// Show or validate configuration
struct ConfigCommand {
    #[argh(subcommand)]
    nested: ConfigSubCommand,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum ConfigSubCommand {
    Show(ConfigShowCommand),
    Validate(ConfigValidateCommand),
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "show")]
/// Print effective config with secrets redacted
struct ConfigShowCommand {
    /// path to config.toml (default: ~/.agverse/config.toml)
    #[argh(option)]
    config: Option<String>,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "validate")]
/// Validate config TOML locally; use --probe to check provider connectivity
struct ConfigValidateCommand {
    /// path to config.toml (default: ~/.agverse/config.toml)
    #[argh(option)]
    config: Option<String>,
    /// probe default provider with a tiny request (network)
    #[argh(switch)]
    probe: bool,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "completion")]
/// Generate shell completion scripts
struct CompletionCommand {
    /// shell: bash | zsh | fish
    #[argh(positional)]
    shell: String,
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


async fn run_tui_mode(args: &Args) -> anyhow::Result<()> {
    let permission = match args.permission.as_deref() {
        Some(s) => Some(parse_permission_mode(s)?),
        None => Some(agent_core::PermissionMode::Developer),
    };
    let tool_mode = match args.tool_mode.as_deref() {
        Some(s) => parse_tool_mode(s)?,
        None => agent_core::ToolExecutionMode::Parallel,
    };
    let state = bootstrap_runtime(BootstrapOptions {
        config_path: resolve_config_path(args.config.as_deref()),
        model: args.model.clone(),
        permission,
        tool_mode,
        enable_hooks: args.hooks,
        dry_run: args.dry_run,
    })
    .await?;
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));
    tui::run_tui(state).await
}

fn print_version() {
    let version = env!("CARGO_PKG_VERSION");
    let commit = option_env!("GIT_COMMIT_HASH").unwrap_or("unknown");
    let date = option_env!("GIT_COMMIT_DATE").unwrap_or("unknown");
    let profile = option_env!("BUILD_PROFILE").unwrap_or("unknown");
    println!("ageverse {version}");
    println!("commit: {commit} ({date})");
    println!("build: {profile}");
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

    if args.version {
        print_version();
        return Ok(ExitCode::SUCCESS);
    }

    match args.nested {
        Some(SubCommand::Eval(eval)) => {
            run_eval_command(eval).await?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(SubCommand::Config(cmd)) => {
            return match cmd.nested {
                ConfigSubCommand::Show(show) => {
                    config_cmd::run_config_show(config_cmd::ConfigShowArgs {
                        config: show.config,
                    })
                    .await
                }
                ConfigSubCommand::Validate(validate) => {
                    config_cmd::run_config_validate(config_cmd::ConfigValidateArgs {
                        config: validate.config,
                        probe: validate.probe,
                    })
                    .await
                }
            };
        }
        Some(SubCommand::Completion(cmd)) => {
            let script = completion::generate(&cmd.shell)?;
            print!("{script}");
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }

    if args.tui {
        run_tui_mode(&args).await?;
        return Ok(ExitCode::SUCCESS);
    }

    // ── One-shot mode (-p and/or piped stdin) ───────────────────────
    if let Some(instruction) = resolve_instruction(args.instruction)? {
        return run_oneshot(OneshotArgs {
            instruction,
            model: args.model,
            permission: args.permission,
            workdir: args.workdir,
            config: args.config,
            dry_run: args.dry_run,
        })
        .await;
    }

    if args.dry_run {
        anyhow::bail!("--dry-run requires oneshot mode (-p or piped stdin)");
    }

    // ── Interactive REPL ────────────────────────────────────────────
    let use_styles = std::io::stdout().is_terminal();
    println!("=== Ageverse CLI ===\n");

    let (permission, enable_hooks, tool_mode, enable_permission) =
        if args.interactive_setup {
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
            (permission, enable_hooks, tool_mode, enable_permission)
        } else {
            let permission = match args.permission.as_deref() {
                Some(s) => Some(parse_permission_mode(s)?),
                None => None,
            };
            let tool_mode = match args.tool_mode.as_deref() {
                Some(s) => parse_tool_mode(s)?,
                None => ToolExecutionMode::Parallel,
            };
            // Permission "enabled" when we did not force Yolo via --permission yolo
            // (config mode still applies when permission is None).
            let enable_permission = !matches!(permission, Some(PermissionMode::Yolo));
            (permission, args.hooks, tool_mode, enable_permission)
        };

    let mut state = bootstrap_runtime(BootstrapOptions {
        config_path: resolve_config_path(args.config.as_deref()),
        model: args.model.clone(),
        permission,
        tool_mode,
        enable_hooks,
        dry_run: false,
    })
    .await?;

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
    println!("Tool mode:   {:?}", state.brain().tool_execution_mode());
    println!(
        "Tools:       {}",
        state.brain().display_registry().list_names().len()
    );
    println!("Model:       {}", state.brain().current_model_name());
    println!("--------------\n");
    println!("Type /help for commands, /quit to exit\n");

    // ── Setup readline with tab-completion and history ──────────────
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();
    let mut rl = Editor::with_config(config).expect("Failed to create line editor");
    rl.set_helper(Some(CommandCompleter));
    let history_path = agent_core::paths::get_cli_history_dir();
    let _ = rl.load_history(&history_path);

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

        if input.starts_with('/') {
            let outcome = commands::dispatch_async(
                &mut state,
                &input,
                enable_permission,
                enable_hooks,
            )
            .await;
            let workflow_goal = state.pending_workflow_request.take();
            if apply_repl_outcome(&state, outcome, enable_permission, enable_hooks) {
                break;
            }
            if let Some(goal) = workflow_goal {
                run_agent(&mut state, &goal, use_styles, true).await;
            }
            continue;
        }
        run_agent(&mut state, &input, use_styles, false).await;
    }

    // ── Graceful shutdown (Ctrl+D, /quit, or input error) ───────────
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
        let model = state.brain().current_model_name().to_string();
        let mgr = &*session_mgr;
        let current_id = state.session_id.clone();
        if let Ok(id) = mgr.save(current_id.as_deref(), &messages, &cwd, &model) {
            let preview = if id.len() >= 8 { &id[..8] } else { &id };
            println!("Session auto-saved: {preview}");
        }
    }

    {
        let mut mgr = mcp_mgr.lock().await;
        if let Err(e) = mgr.shutdown_all().await {
            eprintln!("MCP shutdown warning: {e}");
        }
    }

    if let Err(e) = rl.save_history(&history_path) {
        eprintln!("History save warning: {e}");
    }

    println!("Bye!");
    Ok(ExitCode::SUCCESS)
}

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
    authoring_mode: bool,
) {
    let session_id = state.session_id.clone();
    let history = std::mem::take(&mut state.context_history);
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_default();
    let scoped_tool_factory = authoring_mode
        .then(|| workflow_authoring::scoped_tool_factory(state, session_id.clone(), workspace));
    let prompt = authoring_mode.then(|| workflow_authoring::authoring_prompt(input));
    let run_input = prompt.as_deref().unwrap_or(input);
    let created = match state
        .run_manager
        .create_run_with_workdir_and_images(
            run_input,
            session_id.clone(),
            None,
            history,
            None,
            false,
            Vec::new(),
            None,
            scoped_tool_factory,
        )
        .await
    {
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
    // Restore canonical transcript for multi-turn + session save.
    if let Some(msgs) = state.run_manager.context_snapshot_for_run(&run_id).await {
        state.context_history = msgs;
    }
    state.current_run_id = None;
}

/// Print slash-command outcome for REPL. Returns true if the REPL should quit.
fn apply_repl_outcome(
    state: &CliState,
    outcome: commands::CommandOutcome,
    enable_permission: bool,
    enable_hooks: bool,
) -> bool {
    use commands::{CmdMessage, CommandOutcome, UiRequest};
    match outcome {
        CommandOutcome::Quit => true,
        CommandOutcome::NotSlash => false,
        CommandOutcome::Unknown(cmd) => {
            eprintln!("Unknown command: {cmd}. Type /help for available commands.");
            false
        }
        CommandOutcome::Handled { messages } => {
            for m in messages {
                match m {
                    CmdMessage::Error(s) => eprintln!("{s}"),
                    CmdMessage::Warn(s) | CmdMessage::Info(s) => println!("{s}"),
                }
            }
            false
        }
        CommandOutcome::NeedsUi(ui) => {
            match ui {
                UiRequest::Help => println!("{}", commands::help_text()),
                UiRequest::Status => {
                    println!(
                        "{}",
                        commands::format_status(state, enable_permission, enable_hooks)
                    );
                }
                UiRequest::ModelPicker { models, current } => {
                    for m in models {
                        if m == current {
                            println!("  * {m} (current)");
                        } else {
                            println!("    {m}");
                        }
                    }
                }
                UiRequest::ModelForm => {
                    println!(
                        "Add models in ~/.agverse/config.toml, then /model <name>. (Form is TUI-only.)"
                    );
                }
                UiRequest::SessionList => match state.session_mgr.list(false) {
                    Ok(sessions) if sessions.is_empty() => {
                        println!("No sessions saved. Use /session save.");
                    }
                    Ok(sessions) => {
                        println!("--- Sessions ({}) ---", sessions.len());
                        for s in &sessions {
                            println!("  {}", s.display_line());
                        }
                    }
                    Err(e) => eprintln!("Failed to list sessions: {e}"),
                },
                UiRequest::ShowText { title, body } => {
                    println!("=== {title} ===\n{body}");
                }
                UiRequest::RewindList { points } => {
                    println!("=== Rewind points (user messages) ===");
                    for (idx, preview) in points {
                        println!("  [{idx}] {preview}");
                    }
                    println!("\nUse /rewind <index> to go back to that point.");
                }
            }
            false
        }
    }
}


#[cfg(test)]
mod instruction_merge_tests {
    use super::merge_instruction;

    #[test]
    fn tty_no_flag_enters_repl() {
        let got = merge_instruction(None, None, true).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn non_tty_empty_without_flag_errors() {
        let err = merge_instruction(None, None, false).unwrap_err();
        assert!(err.to_string().contains("not a terminal"));
    }

    #[test]
    fn pipe_only_becomes_instruction() {
        let got = merge_instruction(None, Some("hello".into()), false)
            .unwrap()
            .unwrap();
        assert_eq!(got, "hello");
    }

    #[test]
    fn flag_plus_pipe_concatenates() {
        let got = merge_instruction(Some("review".into()), Some("fn main() {}".into()), false)
            .unwrap()
            .unwrap();
        assert_eq!(got, "review\n\nfn main() {}");
    }

    #[test]
    fn flag_alone_on_tty() {
        let got = merge_instruction(Some("hi".into()), None, true)
            .unwrap()
            .unwrap();
        assert_eq!(got, "hi");
    }
}
