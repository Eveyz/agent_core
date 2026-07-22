//! Non-interactive one-shot agent run (Harbor / CI).

use crate::bootstrap::{parse_permission_mode, resolve_config_path, BootstrapOptions};
use crate::bootstrap::bootstrap_runtime;
use agent_core::{
    ApprovalChoice, MessageDelta, PermissionMode, RunCommand, RunEvent, ToolExecutionMode,
};
use std::io::{self, Write};
use std::process::ExitCode;

pub struct OneshotArgs {
    pub instruction: String,
    pub model: Option<String>,
    pub permission: Option<String>,
    pub workdir: Option<String>,
    pub config: Option<String>,
    pub dry_run: bool,
}

/// Run a single instruction and return a process exit code.
pub async fn run_oneshot(args: OneshotArgs) -> anyhow::Result<ExitCode> {
    let permission = match args.permission.as_deref() {
        Some(s) => parse_permission_mode(s)?,
        None => PermissionMode::Yolo,
    };

    let mut state = bootstrap_runtime(BootstrapOptions {
        config_path: resolve_config_path(args.config.as_deref()),
        model: args.model.clone(),
        permission: Some(permission),
        tool_mode: ToolExecutionMode::Parallel,
        enable_hooks: false,
        dry_run: args.dry_run,
    })
    .await?;

    eprintln!(
        "One-shot: model={} permission={}{}",
        state.run_manager.brain().current_model_name(),
        permission,
        if args.dry_run {
            " dry_run=true (tools vetoed)"
        } else {
            ""
        }
    );

    let workdir = args.workdir.clone();
    let created = if let Some(dir) = workdir {
        state
            .run_manager
            .create_run_with_workdir(
                &args.instruction,
                None,
                Some(dir),
                Vec::new(),
                None,
                false,
            )
            .await?
    } else {
        state
            .run_manager
            .create_run(&args.instruction, None, Vec::new())
            .await?
    };

    let run_id = created.run_id;
    state.current_run_id = Some(run_id.clone());

    if let Err(e) = state.run_manager.command(&run_id, RunCommand::Start).await {
        eprintln!("Error starting run: {e}");
        return Ok(ExitCode::from(1));
    }

    let mut event_rx = match state.run_manager.subscribe(&run_id).await {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("Error subscribing: {e}");
            return Ok(ExitCode::from(1));
        }
    };

    let mut in_agent_text = false;
    let mut exit = ExitCode::SUCCESS;

    loop {
        match event_rx.recv().await {
            Ok(envelope) => match envelope.event {
                RunEvent::TurnStarted { index } => {
                    if index > 1 {
                        eprintln!("─── Turn {index} ───");
                    }
                }
                RunEvent::ModelStreaming { delta, .. } => match delta {
                    MessageDelta::Text(t) => {
                        if !in_agent_text {
                            in_agent_text = true;
                        }
                        print!("{t}");
                        let _ = io::stdout().flush();
                    }
                    MessageDelta::Thinking(t) => {
                        eprint!("{t}");
                        let _ = io::stderr().flush();
                    }
                },
                RunEvent::ToolEnded { name, result, .. } => {
                    let preview: String = result.chars().take(120).collect();
                    eprintln!("🔧 {name} → {preview}");
                }
                RunEvent::ApprovalRequired { prompt_id, .. } => {
                    let _ = state
                        .run_manager
                        .command(
                            &run_id,
                            RunCommand::Approve {
                                prompt_id,
                                choice: ApprovalChoice::AllowOnce,
                            },
                        )
                        .await;
                }
                RunEvent::RunCompleted { .. } => {
                    if in_agent_text {
                        println!();
                    }
                    break;
                }
                RunEvent::RunCancelled { .. } => {
                    eprintln!("Run cancelled.");
                    exit = ExitCode::from(130);
                    break;
                }
                RunEvent::RunFailed { error } => {
                    eprintln!("Error: {error}");
                    exit = ExitCode::from(1);
                    break;
                }
                _ => {}
            },
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("(skipped {n} events)");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    state.current_run_id = None;
    Ok(exit)
}
