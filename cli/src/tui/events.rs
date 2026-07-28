//! Async glue between the TUI event loop and [`agent_core::RunManager`].
//!
//! Kept separate from `state.rs` (pure reducer) and `mod.rs` (terminal loop)
//! so the "spawn a Run and forward its events" concern has a single home.

use crate::state::CliState;
use agent_core::runtime::event::RunEvent;
use agent_core::RunCommand;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex as TokioMutex;

use super::state::AppEvent;

/// Create + start a Run for `input` on a background task, forwarding every
/// event to the UI channel as `AppEvent::Run`. When the run reaches a
/// terminal state, restores the canonical context snapshot into
/// `CliState.context_history` and clears `current_run_id`.
pub fn spawn_run(
    cli: Arc<TokioMutex<CliState>>,
    tx: UnboundedSender<AppEvent>,
    input: String,
    workflow_authoring: bool,
) {
    tokio::spawn(async move {
        let run_id = {
            let mut state = cli.lock().await;
            let session_id = state.session_id.clone();
            let history = std::mem::take(&mut state.context_history);
            let workspace = std::env::current_dir()
                .ok()
                .and_then(|path| path.to_str().map(str::to_string))
                .unwrap_or_default();
            let scoped_tool_factory = workflow_authoring.then(|| {
                crate::workflow_authoring::scoped_tool_factory(
                    &state,
                    session_id.clone(),
                    workspace,
                )
            });
            let prompt = workflow_authoring
                .then(|| crate::workflow_authoring::authoring_prompt(&input));
            let run_input = prompt.as_deref().unwrap_or(&input);
            let created = match state
                .run_manager
                .create_run_with_workdir_and_images(
                    run_input,
                    session_id,
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
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(AppEvent::Run(RunEvent::RunFailed { error: e.to_string() }));
                    return;
                }
            };
            state.current_run_id = Some(created.run_id.clone());
            if let Err(e) = state
                .run_manager
                .command(&created.run_id, RunCommand::Start)
                .await
            {
                let _ = tx.send(AppEvent::Run(RunEvent::RunFailed { error: e.to_string() }));
                state.current_run_id = None;
                return;
            }
            created.run_id
        };

        let mut event_rx = {
            let state = cli.lock().await;
            match state.run_manager.subscribe(&run_id).await {
                Ok(rx) => rx,
                Err(e) => {
                    let _ = tx.send(AppEvent::Run(RunEvent::RunFailed { error: e.to_string() }));
                    drop(state);
                    let mut state = cli.lock().await;
                    state.current_run_id = None;
                    return;
                }
            }
        };

        loop {
            match event_rx.recv().await {
                Ok(envelope) => {
                    let terminal = matches!(
                        envelope.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    );
                    if tx.send(AppEvent::Run(envelope.event)).is_err() {
                        break;
                    }
                    if terminal {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }

        let mut state = cli.lock().await;
        if let Some(snapshot) = state.run_manager.context_snapshot_for_run(&run_id).await {
            state.context_history = snapshot;
        }
        state.current_run_id = None;
    });
}

/// Send a steer message to the active run (no-op if none active).
pub async fn send_steer(cli: &Arc<TokioMutex<CliState>>, message: String) -> Option<String> {
    let mut state = cli.lock().await;
    let run_id = state.current_run_id.clone()?;
    let steer_id = format!(
        "steer-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    match state
        .run_manager
        .steer_run(&run_id, steer_id, message)
        .await
    {
        Ok(()) => None,
        Err(e) => Some(format!("Steer failed: {e}")),
    }
}

/// Resolve a pending approval directly via the per-Run resolver (bypasses
/// the command channel to avoid deadlocking a Run blocked on the oneshot).
pub async fn resolve_approval(
    cli: &Arc<TokioMutex<CliState>>,
    prompt_id: &str,
    choice: agent_core::ApprovalChoice,
) -> bool {
    let state = cli.lock().await;
    let run_id = state.current_run_id.clone();
    state
        .run_manager
        .resolve_approval(run_id.as_deref(), prompt_id, choice)
        .await
}

/// Resolve a pending `InputRequested` with a single free-text answer.
pub async fn resolve_answer(cli: &Arc<TokioMutex<CliState>>, prompt_id: &str, answer: String) -> bool {
    let state = cli.lock().await;
    let run_id = state.current_run_id.clone();
    let mut answers = std::collections::HashMap::new();
    answers.insert("answer".to_string(), vec![answer]);
    state
        .run_manager
        .resolve_input(
            run_id.as_deref(),
            prompt_id,
            agent_core::ClarificationAnswers { answers },
        )
        .await
}

/// Abort the currently active run (fast cancel-token path + queued Cancel).
pub async fn abort_run(cli: &Arc<TokioMutex<CliState>>) -> Option<String> {
    let state = cli.lock().await;
    let Some(run_id) = state.current_run_id.clone() else {
        return Some("No active run to abort.".to_string());
    };
    match state.run_manager.cancel_run(&run_id).await {
        Ok(()) => None,
        Err(e) => Some(format!("Abort failed: {e}")),
    }
}
