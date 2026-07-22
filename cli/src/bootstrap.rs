//! Shared runtime bootstrap for REPL and one-shot modes.

use crate::state::CliState;
use agent_core::{
    load_or_init_default, tasks, Brain, McpClientManager, PermissionMode, RunManager,
    SessionManager, TaskBoard, TodoList, ToolExecutionMode,
};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

pub struct BootstrapOptions {
    /// Override config path (default: `~/.agverse/config.toml`).
    pub config_path: Option<std::path::PathBuf>,
    /// Optional model key from config (`provider/model`).
    pub model: Option<String>,
    /// Optional permission mode override.
    pub permission: Option<PermissionMode>,
    /// Tool execution mode (defaults to Parallel).
    pub tool_mode: ToolExecutionMode,
    /// Register the logging hook.
    pub enable_hooks: bool,
    /// Register DryRunHook — LLM runs, tools are vetoed.
    pub dry_run: bool,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            config_path: None,
            model: None,
            permission: None,
            tool_mode: ToolExecutionMode::Parallel,
            enable_hooks: false,
            dry_run: false,
        }
    }
}

/// Load config (Tauri-aligned), build Brain + RunManager + shared subsystems.
pub async fn bootstrap_runtime(opts: BootstrapOptions) -> anyhow::Result<CliState> {
    let path_ref = opts.config_path.as_deref();
    let (mut config, loaded_path) = load_or_init_default(path_ref)?;
    eprintln!("Config: {}", loaded_path.display());

    if let Some(mode) = opts.permission {
        config.permissions.mode = mode;
    }

    let mut brain = Brain::from_config(config)?;
    brain.set_tool_execution_mode(opts.tool_mode);

    if let Some(ref model) = opts.model {
        brain.switch_model(model)?;
    }

    if opts.dry_run || opts.enable_hooks {
        let mut hooks = brain.hook_registry.lock();
        if opts.dry_run {
            hooks.register(Box::new(crate::dry_run::DryRunHook));
        }
        if opts.enable_hooks {
            hooks.register(Box::new(agent_core::hooks::LoggingHook));
        }
    }

    let skill_manager = brain
        .skill_manager
        .clone()
        .expect("Brain always initializes a SkillManager");

    let session_db_path = agent_core::paths::get_memory_db_path();
    let session_db = session_db_path.to_string_lossy();
    let session_storage =
        agent_core::memory::storage::Storage::new(&session_db).expect("Failed to open session DB");
    let session_mgr = Arc::new(SessionManager::new(session_storage));

    let run_manager = RunManager::new(brain).with_session_manager(session_mgr.clone());

    let todo_list: Arc<Mutex<TodoList>> = Arc::new(Mutex::new(TodoList::new()));
    let task_board: Arc<Mutex<TaskBoard>> = Arc::new(Mutex::new(TaskBoard::new()));

    {
        let brain = run_manager.brain();
        let tb = task_board.clone();
        let mc = brain.current_model_config().ok();
        let pc = brain.config.permissions.clone();
        // Todo tools are registered per-Run via Brain::build_tool_registry_for
        // (SessionPlanStore). Only extra task tools are injected here.
        brain.register_tool_fn(Box::new(move |reg: &mut agent_core::ToolRegistry| {
            tasks::register_task_tools(reg, tb.clone(), mc.clone().unwrap_or_default(), pc.clone());
        }));
    }

    let mcp_mgr = {
        let mut mgr = McpClientManager::from_config(&run_manager.brain().config.mcp);
        let errors = mgr.connect_all().await;
        for (name, errs) in &errors {
            for err in errs {
                eprintln!("[MCP] Server '{}' connection failed: {}", name, err);
            }
        }
        if mgr.tool_count() > 0 {
            eprintln!(
                "[MCP] {} tools from {} servers",
                mgr.tool_count(),
                mgr.connected_servers().len()
            );
            let mgr_arc = Arc::new(tokio::sync::Mutex::new(mgr));
            {
                let mut reg = run_manager.brain().display_registry();
                agent_core::McpTool::register_all(&mut reg, mgr_arc.clone());
            }
            let mgr_arc2 = mgr_arc.clone();
            run_manager.brain().register_tool_fn(Box::new({
                let _mgr = mgr_arc2.clone();
                move |_reg: &mut agent_core::ToolRegistry| {}
            }));
            mgr_arc
        } else {
            Arc::new(tokio::sync::Mutex::new(mgr))
        }
    };

    // Keep CliState.brain in sync with the RunManager Arc after model/permission setup.
    let brain_snapshot = (**run_manager.brain()).clone();

    Ok(CliState {
        brain: brain_snapshot,
        run_manager,
        context_history: Vec::new(),
        session_id: None,
        current_run_id: None,
        cancel_token: None,
        todo_list,
        task_board,
        skill_manager,
        mcp_mgr,
        session_mgr,
    })
}

pub fn parse_permission_mode(s: &str) -> anyhow::Result<PermissionMode> {
    match s.to_lowercase().as_str() {
        "paranoid" => Ok(PermissionMode::Paranoid),
        "standard" => Ok(PermissionMode::Standard),
        "developer" => Ok(PermissionMode::Developer),
        "permissive" => Ok(PermissionMode::Permissive),
        "yolo" => Ok(PermissionMode::Yolo),
        other => anyhow::bail!(
            "invalid permission mode '{other}'. Use: paranoid|standard|developer|permissive|yolo"
        ),
    }
}

pub fn parse_tool_mode(s: &str) -> anyhow::Result<ToolExecutionMode> {
    match s.to_lowercase().as_str() {
        "parallel" | "par" => Ok(ToolExecutionMode::Parallel),
        "sequential" | "seq" => Ok(ToolExecutionMode::Sequential),
        other => anyhow::bail!(
            "invalid tool mode '{other}'. Use: parallel|sequential"
        ),
    }
}

pub fn resolve_config_path(override_path: Option<&str>) -> Option<std::path::PathBuf> {
    override_path.map(|p| Path::new(p).to_path_buf())
}
