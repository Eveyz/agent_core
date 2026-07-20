// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod preview;

use agent_core::{
    AgentMode, Brain, RunCommand, RunEvent, RunManager, RunState,
    permission::ApprovalChoice,
    McpClientManager, McpTool,
};
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use std::sync::Arc;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;
use std::time::Instant;

struct AppState {
    /// The RunManager owns the Brain and tracks all active Runs.
    run_manager: Arc<AsyncMutex<RunManager>>,
    config_path: String,
    project_manager: Arc<Mutex<agent_core::ProjectManager>>,
    session_manager: Arc<agent_core::SessionManager>,
    storage: agent_core::memory::storage::Storage,
    /// PLAN-0009: custom agent registry (CRUD over the `agents` table).
    agent_registry: agent_core::agent_registry::AgentRegistry,
    /// PLAN-0009: active workflow run cancel tokens (run_id -> token).
    workflow_cancels: Arc<AsyncMutex<std::collections::HashMap<String, agent_core::CancellationToken>>>,
    /// MCP client manager — connects to configured MCP servers.
    mcp_manager: Arc<AsyncMutex<McpClientManager>>,
    /// Snapshot of discovered MCP tool definitions, updated after every
    /// connect/reconnect.  Read synchronously by the per-Run registration
    /// callback so `build_tool_registry` never races with `connect_all`.
    mcp_tool_defs: Arc<parking_lot::RwLock<Vec<agent_core::McpToolDef>>>,
    /// Localhost preview subsystem (static + framework dev servers).
    preview_manager: Arc<preview::PreviewManager>,
}

// ── Frontend message type for session save/load ──────────────────────

#[derive(serde::Deserialize, serde::Serialize)]
struct FrontendMessage {
    role: String,
    content: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tool_calls: Option<serde_json::Value>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
struct FrontendSession {
    meta: agent_core::SessionMeta,
    messages: Vec<FrontendMessage>,
    prompts: Vec<agent_core::Prompt>,
}

/// Result of starting a user message run.
///
/// `prompt_id` is the canonical session prompt row id (source of truth for
/// transcript rewind / retry). `run_id` is only the ephemeral execution id.
#[derive(serde::Serialize)]
struct SendMessageResult {
    run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_id: Option<String>,
}

// ── Run lifecycle commands ───────────────────────────────────────────

/// Create a new Run for a user message.
/// Returns `{ run_id, prompt_id }` — use `prompt_id` for session identity
/// and `run_id` for run control / event routing.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    message: String,
    session_id: Option<String>,
    model: Option<String>,
) -> Result<SendMessageResult, String> {
    // Load history + session-level pinned goal
    let mut history = vec![];
    let mut working_dir = None;
    let mut initial_goal: Option<String> = None;
    let mut initial_goal_completed = false;
    if let Some(ref sid) = session_id {
        let sm = state.session_manager.clone();
        let sid_owned = sid.clone();
        if let Some(sess) = tokio::task::spawn_blocking(move || sm.resume(&sid_owned))
            .await
            .map_err(|e| format!("session resume task failed: {e}"))?
            .map_err(|e| format!("failed to resume session: {e}"))?
        {
            history = sess.messages;
            working_dir = Some(sess.meta.cwd);
            initial_goal = sess.meta.pinned_goal.clone();
            initial_goal_completed = sess.meta.goal_completed;
        }
    }

    // Persist /goal mutations on the session before creating the Run.
    let trimmed = message.trim();
    let is_goal_clear = trimmed == "/goal clear"
        || trimmed == "/goal stop"
        || trimmed == "/goal cancel"
        || trimmed == "/goal off";
    if let Some(ref sid) = session_id {
        let sm = state.session_manager.clone();
        let sid_owned = sid.clone();
        if is_goal_clear {
            let _ = tokio::task::spawn_blocking(move || sm.clear_pinned_goal(&sid_owned)).await;
            initial_goal = None;
            initial_goal_completed = false;
        } else if let Some(g) = message
            .strip_prefix("/goal ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            let goal_for_db = g.clone();
            let _ = tokio::task::spawn_blocking(move || {
                sm.set_pinned_goal(&sid_owned, &goal_for_db)
            })
            .await;
            initial_goal = Some(g);
            initial_goal_completed = false;
        }
    }

    // Validate and activate the requested model before creating a run, so an
    // invalid model cannot leave an orphaned lifecycle prompt row.
    {
        let mut manager = state.run_manager.lock().await;
        if let Some(ref m) = model {
            if let Err(error) = manager.switch_model(m) {
                persist_failed_user_message(
                    &state.session_manager,
                    session_id.as_deref(),
                    &history,
                    &message,
                    None,
                )
                .await;
                return Err(error.to_string());
            }
        }
    }

    let manager = state.run_manager.lock().await;

    // Prompt lifecycle (create / finish / persist) lives in RunManager so CLI
    // and Tauri share one prompts-table source of truth.
    let created = match manager
        .create_run_with_workdir(
            &message,
            session_id.clone(),
            working_dir,
            history.clone(),
            initial_goal,
            initial_goal_completed,
        )
        .await
    {
        Ok(created) => created,
        Err(e) => {
            persist_failed_user_message(
                &state.session_manager,
                session_id.as_deref(),
                &history,
                &message,
                None,
            )
            .await;
            return Err(e.to_string());
        }
    };
    let run_id = created.run_id;
    let prompt_id = created.prompt_id;

    // Subscribe to events BEFORE starting, so we don't miss any.
    let mut event_rx = match manager.subscribe(&run_id).await {
        Ok(rx) => rx,
        Err(e) => {
            let _ = manager.command(&run_id, RunCommand::Cancel).await;
            persist_failed_user_message(
                &state.session_manager,
                session_id.as_deref(),
                &history,
                &message,
                prompt_id.as_deref(),
            )
            .await;
            finish_setup_prompt(&state.session_manager, prompt_id.as_deref(), &e.to_string()).await;
            return Err(e.to_string());
        }
    };

    // Start the Run
    if let Err(e) = manager.command(&run_id, RunCommand::Start).await {
        let _ = manager.command(&run_id, RunCommand::Cancel).await;
        persist_failed_user_message(
            &state.session_manager,
            session_id.as_deref(),
            &history,
            &message,
            prompt_id.as_deref(),
        )
        .await;
        finish_setup_prompt(&state.session_manager, prompt_id.as_deref(), &e.to_string()).await;
        return Err(e.to_string());
    }

    // Drop the manager lock so other commands can proceed while we stream events.
    drop(manager);

    // Spawn a task to forward events to the frontend.
    // Transcript + prompt finish are already persisted by RunManager before
    // terminal events are broadcast.
    let app_handle_clone = app_handle.clone();
    let sm_for_task = state.session_manager.clone();
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let is_terminal = matches!(
                        event.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    );

                    // Persist goal completion on the session when the Run signals it.
                    if let RunEvent::GoalCompleted { .. } = &event.event {
                        if let Some(ref sid) = event.session_id {
                            let sm = sm_for_task.clone();
                            let sid_owned = sid.clone();
                            tokio::task::spawn_blocking(move || {
                                let _ = sm.set_goal_completed(&sid_owned, true);
                            });
                        }
                    }

                    if let Err(e) = app_handle_clone.emit("agent-event", &event) {
                        eprintln!("Failed to emit agent event: {}", e);
                    }
                    if is_terminal {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("Event stream lagged by {n} events");
                    continue;
                }
            }
        }
    });

    Ok(SendMessageResult {
        run_id,
        prompt_id,
    })
}

async fn finish_setup_prompt(
    session_manager: &Arc<agent_core::SessionManager>,
    prompt_id: Option<&str>,
    error: &str,
) {
    let Some(prompt_id) = prompt_id else { return };
    let sm = session_manager.clone();
    let prompt_id = prompt_id.to_string();
    let error = error.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = sm.finish_prompt(
            &prompt_id,
            "failed",
            &serde_json::json!({ "setup_error": error }),
        );
    })
    .await;
}

async fn persist_failed_user_message(
    session_manager: &Arc<agent_core::SessionManager>,
    session_id: Option<&str>,
    history: &[agent_core::Message],
    message: &str,
    prompt_id: Option<&str>,
) {
    let Some(session_id) = session_id else { return };
    let sm = session_manager.clone();
    let session_id = session_id.to_string();
    let mut messages = history.to_vec();
    messages.push(agent_core::Message::user(message));
    let bound = prompt_id.map(|p| p.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        match bound.as_deref() {
            Some(pid) => sm.save_canonical_transcript_for_prompt(&session_id, &messages, pid),
            None => sm.save_canonical_transcript(&session_id, &messages),
        }
    })
    .await;
}

// ── /btw & /learn: side-channel slash commands ─────────────────────

#[derive(Clone, serde::Serialize)]
struct BtwEvent {
    btw_id: String,
    session_id: String,
    event_type: &'static str,
    text: String,
}



/// Render a read-only context snapshot as a compact transcript for `/btw`.
fn render_context_snapshot(messages: &[agent_core::Message]) -> String {
    let start = messages.len().saturating_sub(20);
    let mut out = String::new();
    for m in &messages[start..] {
        if let Some(content) = &m.content {
            let role = match m.role {
                agent_core::Role::System => "system",
                agent_core::Role::User => "user",
                agent_core::Role::Assistant => "assistant",
                agent_core::Role::Tool => "tool",
            };
            out.push_str(&format!("[{role}]: {content}\n"));
        }
    }
    out
}

/// `/btw` — ephemeral side-channel Q&A. Runs in parallel with the main Run;
/// reads a best-effort context snapshot but never writes to the main context,
/// session, or event log. Streams deltas over the independent `btw-event` channel.
#[tauri::command]
async fn btw_query(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    session_id: String,
    question: String,
) -> Result<String, String> {
    let run_manager = state.run_manager.clone();
    let brain = run_manager.lock().await.brain().clone();
    let btw_id = uuid::Uuid::new_v4().to_string();
    let return_id = btw_id.clone();

    tokio::spawn(async move {
        let snapshot = run_manager
            .lock()
            .await
            .context_snapshot_for_session(&session_id)
            .await
            .unwrap_or_default();

        let client = match brain.build_client_for("btw") {
            Ok(c) => c,
            Err(e) => {
                let _ = app_handle.emit(
                    "btw-event",
                    BtwEvent {
                        btw_id: btw_id.clone(),
                        session_id: session_id.clone(),
                        event_type: "error",
                        text: e.to_string(),
                    },
                );
                return;
            }
        };

        let system_prompt = format!(
            "You are a helpful assistant. Answer concisely based on the project context.\n\
             Do not use tools. Keep your answer brief and focused.\n\n\
             --- Project Context ---\n{}",
            render_context_snapshot(&snapshot)
        );
        let messages = vec![
            agent_core::Message::system(&system_prompt),
            agent_core::Message::user(&question),
        ];

        match client.chat_completion_stream(&messages, &[]).await {
            Ok(stream) => {
                use futures::StreamExt;
                tokio::pin!(stream);
                while let Some(item) = stream.next().await {
                    if let Ok(agent_core::StreamEvent::TextDelta(text)) = item {
                        let _ = app_handle.emit(
                            "btw-event",
                            BtwEvent {
                                btw_id: btw_id.clone(),
                                session_id: session_id.clone(),
                                event_type: "delta",
                                text,
                            },
                        );
                    }
                }
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "btw-event",
                    BtwEvent {
                        btw_id: btw_id.clone(),
                        session_id: session_id.clone(),
                        event_type: "error",
                        text: e.to_string(),
                    },
                );
                return;
            }
        }

        let _ = app_handle.emit(
            "btw-event",
            BtwEvent {
                btw_id: btw_id.clone(),
                session_id: session_id.clone(),
                event_type: "done",
                text: String::new(),
            },
        );
    });

    Ok(return_id)
}



/// Replay events from a Run's persisted log that the frontend missed (resync).
/// Returns envelopes with seq > from_seq, serialized as JSON strings.
#[tauri::command]
async fn replay_since(
    state: State<'_, AppState>,
    run_id: String,
    from_seq: u64,
) -> Result<Vec<String>, String> {
    let manager = state.run_manager.lock().await;
    let envelopes = manager.replay_since(&run_id, from_seq).map_err(|e| e.to_string())?;
    Ok(envelopes
        .into_iter()
        .map(|env| serde_json::to_string(&env).unwrap_or_default())
        .collect())
}

/// Cancel a running Run. Kills all child processes and aborts all tasks.
#[tauri::command]
async fn abort_agent(state: State<'_, AppState>, run_id: Option<String>) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    if let Some(id) = run_id {
        manager.cancel_run(&id).await.map_err(|e| e.to_string())?;
    } else {
        // Cancel all active runs (backward compat with old abort_agent)
        manager.cancel_all().await;
    }
    Ok(())
}

/// Pause a running Run.
#[tauri::command]
async fn pause_run(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    manager
        .command(&run_id, RunCommand::Pause)
        .await
        .map_err(|e| e.to_string())
}

/// Resume a paused Run.
#[tauri::command]
async fn resume_run(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    manager
        .command(&run_id, RunCommand::Resume)
        .await
        .map_err(|e| e.to_string())
}

/// Inject a steering message into a running Run.
///
/// `steer_id` is a frontend-supplied unique id used to track the steer
/// message across its lifecycle (queued → injected / cancelled).
#[tauri::command]
async fn steer_run(
    state: State<'_, AppState>,
    run_id: String,
    steer_id: String,
    message: String,
) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    manager
        .command(&run_id, RunCommand::Steer { steer_id, message })
        .await
        .map_err(|e| e.to_string())
}

/// Cancel a pending steering message by its id.
///
/// If the message has already been injected (no longer in the queue) this
/// is a silent no-op on the backend.
#[tauri::command]
async fn cancel_steer(
    state: State<'_, AppState>,
    run_id: String,
    steer_id: String,
) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    manager
        .command(&run_id, RunCommand::CancelSteer { steer_id })
        .await
        .map_err(|e| e.to_string())
}

/// Approve a tool execution that's waiting for approval.
///
/// Resolution order:
/// 1. Run-scoped subagent approval map
/// 2. Per-Run `ApprovalResolver` (direct path — no actor deadlock)
/// 3. Command channel broadcast (legacy fallback for paused runs)
#[tauri::command]
async fn approve_tool(
    state: State<'_, AppState>,
    run_id: String,
    prompt_id: String,
    choice: String,
) -> Result<(), String> {
    let choice_enum = match choice.as_str() {
        "allow_once" => ApprovalChoice::AllowOnce,
        "allow_session" => ApprovalChoice::AllowSession,
        "allow_persistent" => ApprovalChoice::AllowPersistent,
        "deny_persistent" => ApprovalChoice::DenyPersistent,
        _ => ApprovalChoice::Deny,
    };

    let manager = state.run_manager.lock().await;

    eprintln!(
        "[approve_tool] pid={} choice={} run_id={:?}",
        prompt_id, choice, run_id
    );

    // 1. Subagent approvals share the parent Run id but have their own waiter.
    let subagent_waiter = {
        let pending_arc = agent_core::permission::pending_subagent_approvals();
        let waiter = pending_arc.lock().remove(&format!("{run_id}:{prompt_id}"));
        waiter
    };
    if let Some(tx) = subagent_waiter {
        eprintln!("[approve_tool] resolved via run-scoped subagent map");
        let _ = tx.send(choice_enum.clone());
        return Ok(());
    }

    // 2. Try per-Run resolver directly (no command channel, no actor deadlock)
    if manager
        .resolve_approval(Some(&run_id), &prompt_id, choice_enum.clone())
        .await
    {
        eprintln!("[approve_tool] resolved via per-Run resolver");
        return Ok(());
    }

    eprintln!("[approve_tool] NOT resolved, falling back to command channel");

    // 3. Run-scoped command-channel fallback (for paused/edge-case runs)
    manager
        .command(
            &run_id,
            RunCommand::Approve {
                prompt_id,
                choice: choice_enum,
            },
        )
        .await
        .map_err(|e| e.to_string())
}

/// Answer a pending `ask_user` clarification request.
///
/// Resolution order mirrors `approve_tool`:
/// 1. Per-Run `InputResolver` (direct — no actor deadlock)
/// 2. Command channel `Answer` fallback
#[tauri::command]
async fn answer_input(
    state: State<'_, AppState>,
    run_id: String,
    prompt_id: String,
    answer: String,
) -> Result<(), String> {
    let answers: agent_core::ClarificationAnswers = serde_json::from_str(&answer)
        .or_else(|_| {
            serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(&answer).map(
                |map| agent_core::ClarificationAnswers { answers: map },
            )
        })
        .map_err(|e| format!("invalid clarification answer JSON: {e}"))?;

    let manager = state.run_manager.lock().await;

    if manager
        .resolve_input(Some(&run_id), &prompt_id, answers.clone())
        .await
    {
        return Ok(());
    }

    // Run-scoped command-channel fallback (paused / edge-case runs)
    manager
        .command(
            &run_id,
            RunCommand::Answer {
                prompt_id,
                answer: serde_json::to_string(&answers).unwrap_or(answer),
            },
        )
        .await
        .map_err(|e| e.to_string())
}

/// Clear the session-level pinned goal (banner ×). Does not start a Run.
#[tauri::command]
async fn clear_session_goal(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let sm = state.session_manager.clone();
    let sid = session_id.clone();
    tokio::task::spawn_blocking(move || sm.clear_pinned_goal(&sid))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    // Clear in-memory session todos so the Plan panel empties immediately.
    {
        let manager = state.run_manager.lock().await;
        manager.brain().todo_lists.clear_session(&session_id);
    }
    Ok(())
}

/// Get the state of a Run.
#[tauri::command]
async fn get_run_state(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<RunState, String> {
    let manager = state.run_manager.lock().await;
    manager.run_state(&run_id).await.map_err(|e| e.to_string())
}

// ── Filesystem commands ──────────────────────────────────────────────

#[tauri::command]
async fn list_directory(path: Option<String>) -> Result<Vec<std::collections::HashMap<String, String>>, String> {
    tokio::task::spawn_blocking(move || {
        let target_path = match path {
            Some(p) => std::path::PathBuf::from(p),
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };

        let mut entries = Vec::new();
        let dir_entries = std::fs::read_dir(&target_path).map_err(|e| e.to_string())?;

        for entry in dir_entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            let mut info = std::collections::HashMap::new();
            info.insert("name".to_string(), entry.file_name().to_string_lossy().to_string());
            info.insert("type".to_string(), if metadata.is_dir() { "dir".to_string() } else { "file".to_string() });
            info.insert("size".to_string(), metadata.len().to_string());
            entries.push(info);
        }

        entries.sort_by(|a, b| {
            let a_is_dir = a.get("type").map(|t| t == "dir").unwrap_or(false);
            let b_is_dir = b.get("type").map(|t| t == "dir").unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.get("name").cmp(&b.get("name")),
            }
        });

        Ok(entries)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn search_files(query: String, path: Option<String>) -> Result<Vec<std::collections::HashMap<String, String>>, String> {
    tokio::task::spawn_blocking(move || {
        let target_path = match path {
            Some(p) => std::path::PathBuf::from(p),
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };

        let mut entries = Vec::new();
        let mut stack = vec![target_path.clone()];
        let query_lower = query.to_lowercase();
        
        let ignore_dirs = vec![
            ".git", "node_modules", "target", "dist", "build", ".svelte-kit", ".next", ".vscode"
        ];

        while let Some(current) = stack.pop() {
            if entries.len() >= 50 {
                break;
            }

            if let Ok(dir_entries) = std::fs::read_dir(&current) {
                for entry in dir_entries.flatten() {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    
                    if let Ok(metadata) = entry.metadata() {
                        let is_dir = metadata.is_dir();
                        
                        if is_dir && (file_name.starts_with('.') && file_name != ".agverse" || ignore_dirs.contains(&file_name.as_str())) {
                            continue;
                        }

                        if !is_dir && file_name == ".DS_Store" {
                            continue;
                        }

                        let rel_path = entry.path().strip_prefix(&target_path)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or(file_name.clone());

                        if query.is_empty() || file_name.to_lowercase().contains(&query_lower) || rel_path.to_lowercase().contains(&query_lower) {
                            let mut info = std::collections::HashMap::new();
                            info.insert("name".to_string(), rel_path);
                            info.insert("type".to_string(), if is_dir { "dir".to_string() } else { "file".to_string() });
                            entries.push(info);
                            
                            if entries.len() >= 50 {
                                break;
                            }
                        }

                        if is_dir {
                            stack.push(entry.path());
                        }
                    }
                }
            }
        }

        entries.sort_by(|a, b| {
            let a_is_dir = a.get("type").map(|t| t == "dir").unwrap_or(false);
            let b_is_dir = b.get("type").map(|t| t == "dir").unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.get("name").cmp(&b.get("name")),
            }
        });

        Ok(entries)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Config commands ──────────────────────────────────────────────────

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<agent_core::config::Config, String> {
    let manager = state.run_manager.lock().await;
    Ok(manager.brain().config.clone())
}

#[tauri::command]
async fn save_config(state: State<'_, AppState>, mut config: agent_core::config::Config) -> Result<(), String> {
    config.rebuild_models();
    let mut manager = state.run_manager.lock().await;
    manager.update_config(config.clone()).map_err(|e| e.to_string())?;

    // Reconnect MCP servers with the new config (if MCP servers changed).
    // Tools from the old manager are still in-flight; new Runs will pick
    // up the new manager via the register_tool_fn callback.
    {
        let mut mcp_mgr = state.mcp_manager.lock().await;
        let mut new_mgr = McpClientManager::from_config(&config.mcp);
        let errors = new_mgr.connect_all().await;
        for (name, errs) in &errors {
            for err in errs {
                eprintln!("[MCP] Server '{}' connection failed: {}", name, err);
            }
        }
        *state.mcp_tool_defs.write() = new_mgr.all_tools();
        *mcp_mgr = new_mgr;
    }

    config.save(&state.config_path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn switch_model(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut manager = state.run_manager.lock().await;
    manager.switch_model(&name).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_context_usage(
    state: State<'_, AppState>,
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<agent_core::ContextUsageSnapshot, String> {
    {
        let manager = state.run_manager.lock().await;
        if let Some(rid) = run_id.as_deref() {
            if let Some(snap) = manager.context_usage_for_run(rid).await {
                return Ok(snap);
            }
        }
        if let Some(sid) = session_id.as_deref() {
            if let Some(snap) = manager.context_usage_for_session(sid).await {
                // Live / just-finished run has a real snapshot — use it.
                if snap.used_tokens > 0 || !snap.segments.is_empty() {
                    return Ok(snap);
                }
            }
        } else {
            return Ok(agent_core::ContextUsageSnapshot::empty(
                manager.current_max_context_tokens(),
            ));
        }
    }

    // Idle / old session: no in-memory Run — rebuild from persisted history
    // so the ring reflects prior messages (including thinking) instead of 0%.
    let sid = session_id.ok_or_else(|| "session_id required".to_string())?;
    let sm = state.session_manager.clone();
    let sid_owned = sid.clone();
    let messages = tokio::task::spawn_blocking(move || {
        sm.resume(&sid_owned)
            .ok()
            .flatten()
            .map(|s| s.messages)
            .unwrap_or_default()
    })
    .await
    .map_err(|e| format!("session resume task failed: {e}"))?;

    let manager = state.run_manager.lock().await;
    Ok(manager.estimate_usage_from_messages(&messages))
}

#[tauri::command]
async fn set_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let agent_mode = match mode.as_str() {
        "ask" => AgentMode::Ask,
        "plan" => AgentMode::Plan,
        "build" => AgentMode::Build,
        other => return Err(format!("unknown mode: {other}")),
    };
    let manager = state.run_manager.lock().await;
    manager.set_mode(agent_mode);
    Ok(())
}

#[tauri::command]
async fn get_mode(state: State<'_, AppState>) -> Result<String, String> {
    let manager = state.run_manager.lock().await;
    let mode_str = match manager.mode() {
        AgentMode::Ask => "ask",
        AgentMode::Plan => "plan",
        AgentMode::Build => "build",
    };
    Ok(mode_str.to_string())
}

// ── Session commands ─────────────────────────────────────────────────

#[tauri::command]
async fn create_session(state: State<'_, AppState>, project_id: String) -> Result<agent_core::SessionMeta, String> {
    let model = {
        let manager = state.run_manager.lock().await;
        manager.brain().current_model_name().to_string()
    };
    let pm = state.project_manager.clone();
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || {
        let messages: Vec<agent_core::types::Message> = vec![];
        let project = {
            let pm = pm.lock();
            pm.get(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Project not found".to_string())?
        };
        let session_id = uuid::Uuid::new_v4().to_string();
        let cwd = if project_id == "__adhoc_chat__" {
            let chat_dir = agent_core::paths::get_agverse_dir().join("chats").join(&session_id);
            std::fs::create_dir_all(&chat_dir)
                .map_err(|e| format!("Failed to create chat directory: {e}"))?;
            chat_dir.to_string_lossy().to_string()
        } else {
            project.path.clone()
        };
        let _ = sm
            .save_with_project(Some(&session_id), &messages, &cwd, &model, Some(&project_id))
            .map_err(|e| e.to_string())?;
        let _ = sm.rename(&session_id, "New Session");
        let meta = sm
            .get_meta(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Session not found after creation".to_string())?;
        Ok(meta)
    })
    .await
    .map_err(|e| format!("create_session task failed: {e}"))?
}

#[tauri::command]
async fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    {
        let manager = state.run_manager.lock().await;
        manager.brain().todo_lists.remove_session(&session_id);
        manager.brain().clear_skill_session(&session_id);
    }
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || {
        sm.delete(&session_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_session task failed: {e}"))?
}

#[tauri::command]
async fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    new_title: String,
) -> Result<bool, String> {
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || {
        sm.rename(&session_id, &new_title).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("rename_session task failed: {e}"))?
}

#[tauri::command]
async fn retry_session_from_prompt(
    state: State<'_, AppState>,
    session_id: String,
    prompt_id: String,
) -> Result<(), String> {
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || sm.truncate_before_prompt(&session_id, &prompt_id))
        .await
        .map_err(|e| format!("retry rewind task failed: {e}"))?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resume_session(state: State<'_, AppState>, session_id: String) -> Result<FrontendSession, String> {
    let sm = state.session_manager.clone();
    let session = tokio::task::spawn_blocking(move || {
        sm.resume(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Session not found".to_string())
    })
    .await
    .map_err(|e| format!("resume_session task failed: {e}"))??;
    let messages: Vec<FrontendMessage> = session
        .messages
        .iter()
        .map(|m| FrontendMessage {
            role: m.role.to_string(),
            content: m.content.clone().unwrap_or_default(),
            model: m.model.clone(),
            tool_calls: m.tool_calls.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
            metadata: m.metadata.clone(),
        })
        .collect();
    Ok(FrontendSession {
        meta: session.meta,
        messages,
        prompts: session.prompts,
    })
}

// ── Project commands ─────────────────────────────────────────────────

#[tauri::command]
async fn list_projects(state: State<'_, AppState>) -> Result<Vec<agent_core::Project>, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.list().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_projects task failed: {e}"))?
}

#[tauri::command]
async fn create_project(state: State<'_, AppState>, path: String) -> Result<agent_core::Project, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.create(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_project task failed: {e}"))?
}

#[tauri::command]
async fn create_new_project(
    state: State<'_, AppState>,
    name: String,
    path: String,
) -> Result<agent_core::Project, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.create_new(&name, &path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_new_project task failed: {e}"))?
}

#[tauri::command]
fn get_documents_dir() -> Result<String, String> {
    agent_core::documents_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_default_project_path(name: String) -> Result<String, String> {
    agent_core::default_project_path(&name).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_project_pinned(
    state: State<'_, AppState>,
    project_id: String,
    pinned: bool,
) -> Result<bool, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.set_pinned(&project_id, pinned).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_project_pinned task failed: {e}"))?
}

#[tauri::command]
async fn set_session_pinned(
    state: State<'_, AppState>,
    session_id: String,
    pinned: bool,
) -> Result<bool, String> {
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || {
        sm.set_pinned(&session_id, pinned).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_session_pinned task failed: {e}"))?
}

#[tauri::command]
async fn delete_project(state: State<'_, AppState>, project_id: String) -> Result<bool, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.delete(&project_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_project task failed: {e}"))?
}

#[tauri::command]
async fn rename_project(
    state: State<'_, AppState>,
    project_id: String,
    new_name: String,
) -> Result<bool, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.rename(&project_id, &new_name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("rename_project task failed: {e}"))?
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct GitBranchInfo {
    branches: Vec<String>,
    active: String,
}

#[tauri::command]
async fn list_git_branches(path: String) -> Result<GitBranchInfo, String> {
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("git")
            .args(["branch"])
            .current_dir(&path)
            .output()
            .map_err(|e| format!("Failed to run git branch: {}", e))?;
        if !output.status.success() {
            return Err(format!("git branch failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut branches = Vec::new();
        let mut active = String::new();
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Some(b) = line.strip_prefix("* ") {
                let name = b.to_string();
                active = name.clone();
                branches.push(name);
            } else {
                branches.push(line.to_string());
            }
        }
        Ok(GitBranchInfo { branches, active })
    })
    .await
    .map_err(|e| format!("list_git_branches task failed: {e}"))?
}

#[tauri::command]
async fn switch_git_branch(path: String, branch: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("git")
            .args(["checkout", &branch])
            .current_dir(&path)
            .output()
            .map_err(|e| format!("Failed to run git checkout: {}", e))?;
        if !output.status.success() {
            return Err(format!("git checkout failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("switch_git_branch task failed: {e}"))?
}

#[tauri::command]
async fn get_project_sessions(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<agent_core::session::SessionMeta>, String> {
    let pm = state.project_manager.clone();
    tokio::task::spawn_blocking(move || {
        let pm = pm.lock();
        pm.list_sessions(&project_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_project_sessions task failed: {e}"))?
}

#[tauri::command]
async fn get_agverse_md() -> Result<String, String> {
    let path = agent_core::paths::get_global_agverse_md_path();
    std::fs::read_to_string(&path).map_err(|e| format!("Failed to read agverse.md: {e}"))
}

#[tauri::command]
async fn get_reflection_status(
    state: State<'_, AppState>,
) -> Result<agent_core::memory::reflection::ReflectionStatus, String> {
    let mut status = agent_core::memory::reflection::reflection_status(state.storage.clone())
        .map_err(|e| e.to_string())?;
    let manager = state.run_manager.lock().await;
    status.enabled = manager.brain().reflection_daemon.is_some();
    Ok(status)
}

#[tauri::command]
async fn read_file(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let resolved_path = if path.starts_with("~/") {
            let home = agent_core::paths::get_agverse_dir()
                .parent()
                .ok_or_else(|| "Failed to get home dir".to_string())?
                .to_path_buf();
            home.join(&path[2..])
        } else {
            std::path::PathBuf::from(path)
        };
        std::fs::read_to_string(&resolved_path).map_err(|e| format!("Failed to read file: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Skills cache ───────────────────────────────────────────────────────

static SKILL_CACHE: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, (Instant, Vec<agent_core::skills::manifest::SkillManifest>)>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
const SKILL_CACHE_TTL: u64 = 30; // seconds

#[tauri::command]
async fn get_skills(
    state: State<'_, AppState>,
    session_id: Option<String>,
    workspace: Option<String>,
) -> Result<Vec<agent_core::skills::manifest::SkillManifest>, String> {
    let workspace = if let Some(session_id) = session_id {
        let session_manager = state.session_manager.clone();
        tokio::task::spawn_blocking(move || session_manager.resume(&session_id))
            .await
            .map_err(|e| format!("session resume task failed: {e}"))?
            .map_err(|e| format!("failed to resolve skill workspace: {e}"))?
            .map(|session| std::path::PathBuf::from(session.meta.cwd))
    } else if let Some(workspace) = workspace.filter(|value| !value.trim().is_empty()) {
        Some(std::path::PathBuf::from(workspace))
    } else {
        None
    };
    let scope_key = workspace
        .as_deref()
        .and_then(|path| path.canonicalize().ok())
        .or_else(|| workspace.clone())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "__global__".to_string());

    // Prefer the exact workspace manager used by Runs. Listing an already
    // scanned manager is cheap and avoids returning a stale TTL entry after a
    // runtime `skill_reload`.
    {
        let run_manager = state.run_manager.lock().await;
        if let Some(sm) = run_manager
            .brain()
            .skill_manager_for_workspace(workspace.as_deref())
            .map_err(|e| e.to_string())?
        {
            let mgr = sm.lock();
            let skills: Vec<_> = mgr.list().into_iter().cloned().collect();
            SKILL_CACHE
                .lock()
                .insert(scope_key, (Instant::now(), skills.clone()));
            return Ok(skills);
        }
    }

    // Fallback: independent scan (Brain has no skill_manager).
    if let Some((cached_at, cached)) = SKILL_CACHE.lock().get(&scope_key) {
        if cached_at.elapsed().as_secs() < SKILL_CACHE_TTL {
            return Ok(cached.clone());
        }
    }
    let mut manager = if workspace.is_some() {
        agent_core::skills::SkillManager::with_global_defaults()
    } else {
        agent_core::skills::SkillManager::with_defaults()
    };
    if let Some(workspace) = workspace.as_deref() {
        manager.add_workspace_root(workspace);
    }
    manager.scan().map_err(|e| e.to_string())?;
    let skills: Vec<agent_core::skills::manifest::SkillManifest> = manager.list().into_iter().cloned().collect();
    SKILL_CACHE
        .lock()
        .insert(scope_key, (Instant::now(), skills.clone()));
    Ok(skills)
}

#[tauri::command]
async fn invalidate_skills_cache(state: State<'_, AppState>) -> Result<(), String> {
    SKILL_CACHE.lock().clear();
    // Rescan global + every cached workspace manager so newly installed skills
    // are activatable everywhere without waiting for a new process.
    let run_manager = state.run_manager.lock().await;
    let _ = run_manager
        .brain()
        .reload_all_skill_managers()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Cronjob commands ───────────────────────────────────────────────────

#[tauri::command]
async fn list_cronjobs(state: State<'_, AppState>) -> Result<Vec<agent_core::CronJob>, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        let conn = storage.conn();
        agent_core::CronjobStore::list(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_cronjobs task failed: {e}"))?
}

#[tauri::command]
async fn create_cronjob(
    state: State<'_, AppState>,
    name: String,
    cadence_type: String,
    cadence_value: String,
    prompt: String,
    project: Option<String>,
    skills: Vec<String>,
    permission_level: String,
    max_concurrency: Option<u32>,
    model: Option<String>,
) -> Result<agent_core::CronJob, String> {
    let job = agent_core::CronJob {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        cadence_type,
        cadence_value,
        prompt,
        project,
        skills,
        permission_level,
        max_concurrency,
        model,
        enabled: true,
        created_at: chrono::Utc::now(),
    };
    let storage = state.storage.clone();
    tokio::task::spawn_blocking({
        let job = job.clone();
        move || {
            let conn = storage.conn();
            agent_core::CronjobStore::insert(&conn, &job).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| format!("create_cronjob task failed: {e}"))??;
    
    Ok(job)
}

#[tauri::command]
async fn update_cronjob(
    state: State<'_, AppState>,
    job: agent_core::CronJob,
) -> Result<(), String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        let conn = storage.conn();
        agent_core::CronjobStore::update(&conn, &job).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("update_cronjob task failed: {e}"))?
}

#[tauri::command]
async fn delete_cronjob(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        let conn = storage.conn();
        agent_core::CronjobStore::delete(&conn, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_cronjob task failed: {e}"))?
}

#[tauri::command]
async fn toggle_cronjob(state: State<'_, AppState>, id: String, enabled: bool) -> Result<(), String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        let conn = storage.conn();
        let jobs = agent_core::CronjobStore::list(&conn).map_err(|e| e.to_string())?;
        if let Some(mut job) = jobs.into_iter().find(|j| j.id == id) {
            job.enabled = enabled;
            agent_core::CronjobStore::update(&conn, &job).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("toggle_cronjob task failed: {e}"))?
}

// ── PLAN-0009: Custom Agent CRUD ────────────────────────────────────

/// Build an [`AgentMemoryStore`] from the Brain's embedding configuration.
fn build_agent_memory_store(
    brain: &Brain,
    storage: agent_core::memory::storage::Storage,
) -> agent_core::agent_registry::AgentMemoryStore {
    if let Some(ref mem) = brain.config.memory {
        if mem.embedding_enabled {
            if let Ok(model) =
                agent_core::memory::embedding::EmbeddingModel::new(&mem.embedding_model)
            {
                return agent_core::agent_registry::AgentMemoryStore::new(
                    storage,
                    std::sync::Arc::new(model),
                );
            }
        }
    }
    agent_core::agent_registry::AgentMemoryStore::without_embedding(storage)
}

/// List the names of all available tools (for the agent editor's tool picker).
#[tauri::command]
async fn list_available_tools(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let run_manager = state.run_manager.lock().await;
    let brain = run_manager.brain().clone();
    drop(run_manager);
    let registry = brain.build_tool_registry(agent_core::AgentMode::Build);
    Ok(registry.list_names().iter().map(|s| s.to_string()).collect())
}

#[tauri::command]
async fn create_agent(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
    system_prompt: Option<String>,
    model: Option<String>,
    skills: Option<Vec<String>>,
    tools: Option<Vec<String>>,
    permission_mode: Option<String>,
    max_iterations: Option<usize>,
    max_context_tokens: Option<usize>,
    memory_enabled: Option<u8>,
    memory_group: Option<String>,
    icon: Option<String>,
    color: Option<String>,
) -> Result<agent_core::agent_registry::AgentDef, String> {
    let registry = state.agent_registry.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let def = agent_core::agent_registry::AgentDef {
        id: String::new(),
        name,
        description: description.unwrap_or_default(),
        system_prompt: system_prompt.unwrap_or_default(),
        model: model.unwrap_or_default(),
        skills: skills.unwrap_or_default(),
        tools: tools.unwrap_or_default(),
        permission_mode: permission_mode.unwrap_or_else(|| "standard".to_string()),
        permission_rules: serde_json::Value::Array(Vec::new()),
        max_iterations: max_iterations.unwrap_or(50),
        max_context_tokens: max_context_tokens.unwrap_or(32000),
        memory_enabled: memory_enabled.unwrap_or(1),
        memory_group: memory_group.unwrap_or_default(),
        icon: icon.unwrap_or_default(),
        color: color.unwrap_or_default(),
        created_at: now.clone(),
        updated_at: now,
    };
    let storage = registry.storage().clone();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::create(&storage, &def).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_agent task failed: {e}"))?
}

#[tauri::command]
async fn list_agents(state: State<'_, AppState>) -> Result<Vec<agent_core::agent_registry::AgentDef>, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::list(&storage).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_agents task failed: {e}"))?
}

#[tauri::command]
async fn get_agent(state: State<'_, AppState>, id: String) -> Result<agent_core::agent_registry::AgentDef, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::get(&storage, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_agent task failed: {e}"))?
}

#[tauri::command]
async fn update_agent(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
    system_prompt: Option<String>,
    model: Option<String>,
    skills: Option<Vec<String>>,
    tools: Option<Vec<String>>,
    permission_mode: Option<String>,
    permission_rules: Option<serde_json::Value>,
    max_iterations: Option<usize>,
    max_context_tokens: Option<usize>,
    memory_enabled: Option<u8>,
    memory_group: Option<String>,
    icon: Option<String>,
    color: Option<String>,
) -> Result<agent_core::agent_registry::AgentDef, String> {
    let storage = state.storage.clone();
    let updates = agent_core::agent_registry::AgentDefUpdate {
        name,
        description,
        system_prompt,
        model,
        skills,
        tools,
        permission_mode,
        permission_rules,
        max_iterations,
        max_context_tokens,
        memory_enabled,
        memory_group,
        icon,
        color,
    };
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::update(&storage, &id, &updates).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("update_agent task failed: {e}"))?
}

#[tauri::command]
async fn delete_agent(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::delete(&storage, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_agent task failed: {e}"))?
}

#[tauri::command]
async fn search_agent_memory(
    state: State<'_, AppState>,
    agent_id: String,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let run_manager = state.run_manager.lock().await;
    let brain = run_manager.brain().clone();
    drop(run_manager);
    let storage = state.storage.clone();
    let def = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            agent_core::agent_registry::get(&storage, &agent_id).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("task failed: {e}"))??
    };
    let memory_key = def.memory_key().to_string();
    let store = build_agent_memory_store(&brain, storage);
    let top_k = top_k.unwrap_or(5);
    tokio::task::spawn_blocking(move || {
        let records = store.search(&memory_key, &query, top_k).map_err(|e| e.to_string())?;
        Ok(records
            .into_iter()
            .map(|r| serde_json::json!({
                "id": r.id,
                "role": r.role,
                "content": r.content,
                "importance": r.importance,
                "category": format!("{:?}", r.category),
                "created_at": r.created_at,
            }))
            .collect::<Vec<_>>())
    })
    .await
    .map_err(|e| format!("search_agent_memory task failed: {e}"))?
}

#[tauri::command]
async fn get_agent_history(
    state: State<'_, AppState>,
    agent_id: String,
    limit: Option<usize>,
) -> Result<Vec<agent_core::agent_registry::AgentHistoryEntry>, String> {
    let storage = state.storage.clone();
    let limit = limit.unwrap_or(50);
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::history::list(&storage, &agent_id, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_agent_history task failed: {e}"))?
}

/// Run a custom agent standalone (outside of a workflow).
#[tauri::command]
async fn run_agent_standalone(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    agent_id: String,
    input: String,
    session_id: Option<String>,
) -> Result<String, String> {
    use agent_core::runtime::supervisor::ProcessSupervisor;
    use agent_core::skills::SkillManager;
    use agent_core::subagent::Subagent;
    use agent_core::tools::subagent::{
        ApprovalRouting, re_wire_subagent_tools_with_skills,
    };
    use agent_core::CancellationToken;

    let run_manager = state.run_manager.lock().await;
    let brain = run_manager.brain().clone();
    drop(run_manager);

    // Fetch the agent definition.
    let storage = state.storage.clone();
    let def = {
        let s = storage.clone();
        tokio::task::spawn_blocking(move || {
            agent_core::agent_registry::get(&s, &agent_id).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("task failed: {e}"))??
    };

    // Build runtime components from the Brain.
    let mut subagent_config = agent_core::agent_registry::build_subagent_config(&def);
    let effective_skills = if let Some(ref sm) = brain.skill_manager {
        let mgr = sm.lock();
        mgr.resolve_subagent_skills(&def.skills, session_id.as_deref())
    } else {
        def.skills.clone()
    };
    subagent_config.skills = effective_skills.clone();
    // Inject skill content into the system prompt (content path).
    subagent_config.system_prompt = SkillManager::inject_skill_content_into(
        brain.skill_manager.as_ref(),
        &effective_skills,
        &subagent_config.system_prompt,
    );

    let model_config = agent_core::agent_registry::build_model_config(&def, &brain.config);
    let permission_config =
        agent_core::agent_registry::build_permission_config(&def, &brain.config.permissions);

    // Standalone agent gets its own ProcessSupervisor + cancel token so:
    //  - its shell children are process-group isolated and killed on cancel
    //  - any subagent it spawns (path D) inherits the supervisor+cancel
    //    via re_wire_subagent_tools (vs. the Brain-built registry's None,None)
    let supervisor = std::sync::Arc::new(parking_lot::Mutex::new(ProcessSupervisor::new()));
    let cancel_token = CancellationToken::new();

    // Build tool registry: inherit all if tools empty, else named subset.
    let mut registry = if def.tools.is_empty() {
        brain.build_tool_registry(agent_core::AgentMode::Build)
    } else {
        agent_core::ToolRegistry::from_names(&def.tools)
    };

    // Re-wire subagent meta tools with our supervisor + cancel so spawned
    // grand-subagents are cancellable. (depth 0: standalone agent itself).
    re_wire_subagent_tools_with_skills(
        &mut registry,
        model_config.clone(),
        None,
        permission_config.clone(),
        Some(supervisor.clone()),
        Some(cancel_token.clone()),
        0,
        brain.skill_manager.clone(),
        ApprovalRouting::LegacyScoped,
    );

    // Ensure ShellTool (when present) is the supervised version.
    if registry.has("shell") {
        registry.register(Box::new(
            agent_core::tools::shell::ShellTool::with_supervisor(supervisor.clone(), None),
        ));
    }

    // Register script tools for declared skills ∪ session actives.
    SkillManager::sync_skill_scripts_for_skills(
        brain.skill_manager.as_ref(),
        &effective_skills,
        &mut registry,
        Some(supervisor.clone()),
    );

    // Build the per-agent memory store (if enabled).
    let memory = if def.memory_enabled > 0 {
        let store = build_agent_memory_store(&brain, storage.clone());
        Some(std::sync::Arc::new(store))
    } else {
        None
    };

    let mut subagent = Subagent::new_with_memory(
        &def.name,
        subagent_config,
        &model_config,
        registry,
        permission_config,
        memory,
        Some(def.memory_identity()),
    )
    .with_supervisor(supervisor.clone())
    .with_cancel_token(cancel_token);

    let session = session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let started = std::time::Instant::now();
    let _ = app_handle; // (event emission wired later via event_tx)

    let result = subagent.run(&input).await.map_err(|e| e.to_string())?;
    let elapsed_ms = started.elapsed().as_millis() as i64;

    // Record execution history.
    let entry = agent_core::agent_registry::AgentHistoryEntry {
        agent_id: def.id.clone(),
        session_id: session,
        workflow_run_id: String::new(),
        trigger: "manual".to_string(),
        input: input.clone(),
        output: result.output.clone(),
        iterations_used: result.iterations_used as u32,
        success: result.success,
        model_used: model_config.model_id.clone(),
        process_time_ms: elapsed_ms,
        ..Default::default()
    };
    let history_storage = storage.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = agent_core::agent_registry::history::record(&history_storage, &entry);
    })
    .await;

    Ok(result.output)
}

/// Validate a workflow definition (cycle detection, orphan nodes, missing config).
/// Takes raw node/edge definitions (not a persisted workflow id) so the user
/// can validate before saving.
#[tauri::command]
async fn validate_workflow(
    nodes: Vec<agent_core::workflow::NodeDef>,
    edges: Vec<agent_core::workflow::EdgeDef>,
) -> Result<agent_core::workflow::ValidationResult, String> {
    let wf = agent_core::workflow::WorkflowDef {
        nodes,
        edges,
        ..Default::default()
    };
    Ok(agent_core::workflow::validate(&wf))
}

// ── PLAN-0009: Workflow CRUD + Execution ────────────────────────────

#[tauri::command]
async fn create_workflow(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
) -> Result<agent_core::workflow::WorkflowDef, String> {
    let storage = state.storage.clone();
    let wf = agent_core::workflow::WorkflowDef {
        name,
        description: description.unwrap_or_default(),
        ..Default::default()
    };
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::create(&storage, &wf).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_workflow task failed: {e}"))?
}

#[tauri::command]
async fn list_workflows(state: State<'_, AppState>) -> Result<Vec<agent_core::workflow::WorkflowDef>, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::list(&storage).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_workflows task failed: {e}"))?
}

#[tauri::command]
async fn get_workflow(state: State<'_, AppState>, id: String) -> Result<agent_core::workflow::WorkflowDef, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::get(&storage, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_workflow task failed: {e}"))?
}

#[tauri::command]
async fn save_workflow(
    state: State<'_, AppState>,
    id: String,
    name: String,
    description: Option<String>,
    nodes: Vec<agent_core::workflow::NodeDef>,
    edges: Vec<agent_core::workflow::EdgeDef>,
    trust_mode: Option<String>,
    max_concurrent: Option<usize>,
    on_node_failure: Option<String>,
    input_schema: Option<serde_json::Value>,
    config: Option<serde_json::Value>,
) -> Result<agent_core::workflow::WorkflowDef, String> {
    let storage = state.storage.clone();
    let wf = agent_core::workflow::WorkflowDef {
        id,
        name,
        description: description.unwrap_or_default(),
        input_schema: input_schema.unwrap_or_else(|| serde_json::json!({})),
        trust_mode: trust_mode
            .as_deref()
            .map(agent_core::workflow::TrustMode::from_str)
            .unwrap_or_default(),
        max_concurrent: max_concurrent.unwrap_or(3),
        on_node_failure: on_node_failure
            .as_deref()
            .map(agent_core::workflow::OnNodeFailure::from_str)
            .unwrap_or_default(),
        config: config.unwrap_or_else(|| serde_json::json!({})),
        nodes,
        edges,
        created_at: String::new(),
        updated_at: String::new(),
    };
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::save(&storage, &wf).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("save_workflow task failed: {e}"))?
}

#[tauri::command]
async fn delete_workflow(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::delete(&storage, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_workflow task failed: {e}"))?
}

/// Execute a workflow. Returns the run result.
///
/// Emits `workflow_event` Tauri events for real-time node status updates.
#[tauri::command]
async fn run_workflow(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    workflow_id: String,
    input: Option<serde_json::Value>,
    session_id: Option<String>,
) -> Result<serde_json::Value, String> {
    use agent_core::workflow::{WorkflowExecutor, get as get_workflow_def};

    // Load the workflow definition.
    let storage = state.storage.clone();
    let wf = {
        let s = storage.clone();
        let wid = workflow_id.clone();
        tokio::task::spawn_blocking(move || {
            get_workflow_def(&s, &wid).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("task failed: {e}"))??
    };

    // Access the Brain.
    let run_manager = state.run_manager.lock().await;
    let brain = run_manager.brain().clone();
    drop(run_manager);

    let input = input.unwrap_or_else(|| serde_json::json!({}));
    let session = session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Create a cancel token and register it.
    let cancel_token = agent_core::CancellationToken::new();
    let run_id_placeholder = uuid::Uuid::new_v4().to_string();
    {
        let mut cancels = state.workflow_cancels.lock().await;
        cancels.insert(run_id_placeholder.clone(), cancel_token.clone());
    }

    // Event channel: forward workflow events to the frontend.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<agent_core::types::AgentEvent>();
    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            let _ = app_handle_clone.emit("workflow_event", &ev);
        }
    });

    let executor = WorkflowExecutor::new(storage.clone(), brain);
    let result = executor
        .execute(&wf, input, &session, cancel_token, Some(event_tx))
        .await
        .map_err(|e| e.to_string())?;

    // Clean up the cancel token.
    {
        let mut cancels = state.workflow_cancels.lock().await;
        cancels.remove(&run_id_placeholder);
    }

    Ok(serde_json::json!({
        "run_id": result.run_id,
        "status": result.status,
        "output": result.output,
        "error": result.error,
        "total_token_input": result.total_token_input,
        "total_token_output": result.total_token_output,
    }))
}

/// Cancel a running workflow.
#[tauri::command]
async fn cancel_workflow_run(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let mut cancels = state.workflow_cancels.lock().await;
    if let Some(token) = cancels.remove(&run_id) {
        token.cancel();
        Ok(())
    } else {
        Err(format!("no active workflow run with id '{run_id}'"))
    }
}

#[tauri::command]
async fn list_workflow_runs(
    state: State<'_, AppState>,
    workflow_id: String,
    limit: Option<usize>,
) -> Result<Vec<agent_core::workflow::WorkflowRun>, String> {
    let storage = state.storage.clone();
    let limit = limit.unwrap_or(20);
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::list_runs(&storage, &workflow_id, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_workflow_runs task failed: {e}"))?
}

#[tauri::command]
async fn get_workflow_run_results(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<Vec<agent_core::workflow::WorkflowRunNodeResult>, String> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        agent_core::workflow::get_run_node_results(&storage, &run_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_workflow_run_results task failed: {e}"))?
}

// ── PLAN-0009 Phase 6: Skill Draft Generation (Experimental) ────────

/// Resolve the skill drafts directory (~/.agverse/skill_drafts/).
fn skill_drafts_dir() -> std::path::PathBuf {
    agent_core::paths::get_agverse_dir().join("skill_drafts")
}

/// Resolve the user skills directory (~/.agverse/skills/).
fn user_skills_dir() -> std::path::PathBuf {
    agent_core::paths::get_agverse_dir().join("skills")
}

#[tauri::command]
async fn generate_agent_skill_drafts(
    state: State<'_, AppState>,
    agent_id: String,
    limit: Option<usize>,
) -> Result<agent_core::agent_registry::DraftGenerationResult, String> {
    let storage = state.storage.clone();
    let drafts_dir = skill_drafts_dir();
    let limit = limit.unwrap_or(100);
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::generate_drafts(&storage, &agent_id, &drafts_dir, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("generate_agent_skill_drafts task failed: {e}"))?
}

#[tauri::command]
async fn list_skill_drafts() -> Result<Vec<agent_core::agent_registry::SkillDraft>, String> {
    let drafts_dir = skill_drafts_dir();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::list_drafts(&drafts_dir).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_skill_drafts task failed: {e}"))?
}

#[tauri::command]
async fn approve_skill_draft(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let drafts_dir = skill_drafts_dir();
    let skills_dir = user_skills_dir();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::approve_draft(&drafts_dir, &skills_dir, &name)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("approve_skill_draft task failed: {e}"))??;
    // Keep UI + Brain in sync after promoting a draft.
    invalidate_skills_cache(state).await
}

#[tauri::command]
async fn reject_skill_draft(name: String) -> Result<(), String> {
    let drafts_dir = skill_drafts_dir();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::reject_draft(&drafts_dir, &name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("reject_skill_draft task failed: {e}"))?
}

// ── App entry point ──────────────────────────────────────────────────

pub fn run() {
    // ── Initialize structured logging to ~/.agverse/logs/ ─────────
    // All `tracing::info!` / `debug!` / `warn!` / `error!` calls from
    // the Rust backend (including agent_core) are written here.
    {
        let log_dir = agent_core::paths::get_agverse_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("agent.log");
        // Append mode: single file, no rotation.  Set RUST_LOG=agent_core=debug
        // at runtime for verbose output.
        if let Ok(file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&log_path)
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::builder()
                        .from_env_lossy()
                        .add_directive(
                            "agent_core=info".parse().unwrap_or_default()
                        ),
                )
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
        }
    }

    tauri::Builder::default()        .setup(|app| {
            // Load config the same way as the CLI (`~/.agverse/config.toml`).
            let (config, config_path) = agent_core::load_or_init_default(None)
                .expect("Failed to load or init ~/.agverse/config.toml");
            let config_path_str = config_path.to_string_lossy().into_owned();

            // Build the Brain (reusable across all Runs)
            let brain = Brain::from_config(config)
                .expect("Failed to build brain from config");

            // Determine the SQLite path for project/session storage
            let db_path = if let Some(ref mem_config) = brain.config.memory {
                mem_config.db_path.clone()
            } else {
                agent_core::paths::get_memory_db_path().to_string_lossy().into_owned()
            };
            let storage = agent_core::memory::storage::Storage::new(&db_path)
                .expect("Failed to open storage database");
            let project_manager = Arc::new(Mutex::new(
                agent_core::ProjectManager::new(storage.clone())
            ));

            // Seed the default container project and migrate legacy sessions
            {
                let db = storage.conn();
                let now = chrono::Utc::now().to_rfc3339();
                let path = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string());
                let _ = db.execute(
                    "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at) VALUES ('__adhoc_chat__', 'Default', ?1, ?2, ?2)",
                    agent_core::rusqlite::params![&path, &now],
                );
                let _ = db.execute(
                    "UPDATE sessions SET project_id = '__adhoc_chat__' WHERE project_id IS NULL OR project_id = ''",
                    [],
                );
            }

            // ── Startup zombie repair ─────────────────────────────────
            // If the app crashed / was killed / lost power, any prompt
            // still in 'running' state is an orphan. Mark them as
            // 'interrupted' so the frontend doesn't show "Working..."
            // forever. The ended_at timestamp uses the last saved message
            // time (most accurate), falling back to started_at, then now.
            // Same for workflow runs.
            {
                let db = storage.conn();
                let now = chrono::Utc::now().to_rfc3339();

                let zombie_prompts = db.execute(
                    "UPDATE prompts SET status = 'interrupted', ended_at = COALESCE( \
                     (SELECT sm.created_at FROM session_messages sm \
                      WHERE sm.session_id = prompts.session_id \
                      ORDER BY sm.msg_index DESC LIMIT 1), \
                     prompts.started_at, ?1) \
                     WHERE status = 'running'",
                    agent_core::rusqlite::params![now],
                ).unwrap_or(0);

                let zombie_workflows = db.execute(
                    "UPDATE workflow_runs SET status = 'interrupted', finished_at = ?1, error = 'App restarted — run was interrupted' WHERE status = 'running'",
                    agent_core::rusqlite::params![now],
                ).unwrap_or(0);

                if zombie_prompts > 0 || zombie_workflows > 0 {
                    eprintln!(
                        "[startup] zombie repair: {} prompts, {} workflow runs interrupted",
                        zombie_prompts, zombie_workflows
                    );
                }
            }

            let session_manager = Arc::new(
                agent_core::SessionManager::new(storage.clone())
            );

            // Build the RunManager — prompt lifecycle is owned here so CLI and
            // Tauri share the same prompts-table source of truth.
            let run_manager = RunManager::new(brain).with_session_manager(session_manager.clone());

            // ── MCP: connect to configured servers and register tools ─
            // Tool definitions live in a parking_lot RwLock so the
            // per-Run registration callback (which runs synchronously
            // inside build_tool_registry) can always read them without
            // racing against the async connect_all task.
            let mcp_config = run_manager.brain().config.mcp.clone();
            let mcp_tool_defs: Arc<parking_lot::RwLock<Vec<agent_core::McpToolDef>>> =
                Arc::new(parking_lot::RwLock::new(Vec::new()));
            let mcp_manager = Arc::new(AsyncMutex::new(
                McpClientManager::from_config(&mcp_config),
            ));

            // Initial connect + populate defs (runs async, does not block setup).
            {
                let mgr = mcp_manager.clone();
                let defs = mcp_tool_defs.clone();
                tauri::async_runtime::spawn(async move {
                    let mut g = mgr.lock().await;
                    let errors = g.connect_all().await;
                    for (name, errs) in &errors {
                        for err in errs {
                            eprintln!("[MCP] Server '{}' connection failed: {}", name, err);
                        }
                    }
                    let count = g.tool_count();
                    *defs.write() = g.all_tools();
                    if count > 0 {
                        eprintln!(
                            "[MCP] {} tools from {} servers",
                            count,
                            g.connected_servers().len()
                        );
                    }
                });
            }

            // Per-Run callback: reads the tool-defs snapshot (always succeeds,
            // no try_lock) and registers McpTool wrappers into the fresh
            // ToolRegistry for this Run.
            {
                let defs = mcp_tool_defs.clone();
                let mgr = mcp_manager.clone();
                run_manager
                    .brain()
                    .register_tool_fn(Box::new(move |registry| {
                        for tool_def in defs.read().iter() {
                            let t = McpTool::new(
                                tool_def.server.clone(),
                                tool_def.name.clone(),
                                tool_def.description.clone(),
                                tool_def.parameters.clone(),
                                mgr.clone(),
                            );
                            registry.register(Box::new(t));
                        }
                    }));
            }

            let preview_manager = preview::create_manager();

            // Per-Run callback: register the localhost preview agent tool.
            {
                let pm = preview_manager.clone();
                let proj = project_manager.clone();
                run_manager.brain().register_tool_fn(Box::new(move |registry| {
                    registry.register(Box::new(preview::tool::PreviewTool::new(
                        pm.clone(),
                        proj.clone(),
                    )));
                }));
            }

            // Grab handles BEFORE run_manager is moved into AppState —
            // used for deferred background warmup and index building below.
            let embed_model = run_manager
                .brain()
                .memory
                .as_ref()
                .and_then(|mm| mm.lock().recall().embedding_model().cloned());
            let memory_mgr = run_manager
                .brain()
                .memory
                .clone();
            let reflection_daemon = run_manager
                .brain()
                .reflection_daemon
                .clone();

            app.manage(AppState {
                run_manager: Arc::new(AsyncMutex::new(run_manager)),
                config_path: config_path_str,
                project_manager,
                session_manager,
                storage: storage.clone(),
                agent_registry: agent_core::agent_registry::AgentRegistry::new(storage.clone()),
                workflow_cancels: Arc::new(AsyncMutex::new(std::collections::HashMap::new())),
                mcp_manager,
                mcp_tool_defs,
                preview_manager,
            });

            if let Some(daemon) = reflection_daemon {
                tauri::async_runtime::spawn(async move {
                    daemon.start();
                });
            }

            let preview_mgr = {
                let state = app.state::<AppState>();
                state.preview_manager.clone()
            };
            let app_handle = app.handle().clone();
            tauri::async_runtime::block_on(async {
                preview_mgr.set_app_handle(app_handle).await;
            });

            // Deferred warmup: wait for the UI to render, then preload the
            // embedding model so the first real embed() call doesn't stall.
            if let Some(model) = embed_model {
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    eprintln!("[warmup] preloading embedding model...");
                    match tokio::task::spawn_blocking(move || {
                        model.embed_single("warmup")
                    }).await {
                        Ok(Ok(_)) => eprintln!("[warmup] embedding model ready"),
                        Ok(Err(e)) => eprintln!("[warmup] failed: {e}"),
                        Err(e) => eprintln!("[warmup] task panicked: {e}"),
                    }
                });
            }

            // Deferred index building: build BM25 + HNSW in background so
            // the UI isn't blocked. Searches fall back to SQLite until done.
            //
            // We clone the Storage handle (brief lock) so the CPU-heavy build
            // runs on a dedicated OS thread WITHOUT holding the MemoryManager
            // lock — UI operations like chat / search / memory store can
            // proceed concurrently.
            if let Some(mm) = memory_mgr {
                let storage = mm.lock().recall().storage();
                // mm (Arc) is kept alive and moved into the thread below
                std::thread::Builder::new()
                    .name("index-builder".into())
                    .spawn(move || {
                        eprintln!("[index] building search indexes (BM25 + HNSW)...");
                        let start = std::time::Instant::now();
                        let bm25 = agent_core::memory::MemoryManager::build_bm25_from(&storage).ok();
                        let hnsw = agent_core::memory::MemoryManager::build_hnsw_from(&storage).ok();
                        eprintln!("[index] BM25 + HNSW ready in {:?}", start.elapsed());

                        // Brief lock to inject built indexes
                        let mut mm = mm.lock();
                        if let Some(b) = bm25 { mm.set_bm25(b); }
                        if let Some(h) = hnsw { mm.set_hnsw(h); }
                    })
                    .expect("failed to spawn index builder thread");
            }

            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            send_message, approve_tool, answer_input, clear_session_goal, abort_agent, replay_since,
            btw_query,
            pause_run, resume_run, steer_run, cancel_steer, get_run_state,
            list_directory, search_files,
            get_config, save_config, switch_model, get_context_usage, set_mode, get_mode,
            create_session, delete_session, rename_session,
            retry_session_from_prompt, resume_session,
            list_projects, create_project, create_new_project, get_documents_dir, get_default_project_path,
            set_project_pinned, set_session_pinned,
            delete_project, rename_project, open_in_explorer,
            list_git_branches, switch_git_branch, get_project_sessions,
            get_agverse_md, get_reflection_status, read_file, get_skills, invalidate_skills_cache,
            list_cronjobs, create_cronjob, update_cronjob, delete_cronjob, toggle_cronjob,
            list_available_tools,
            create_agent, list_agents, get_agent, update_agent, delete_agent,
            search_agent_memory, get_agent_history, run_agent_standalone,
            validate_workflow,
            create_workflow, list_workflows, get_workflow, save_workflow, delete_workflow,
            run_workflow, cancel_workflow_run, list_workflow_runs, get_workflow_run_results,
            generate_agent_skill_drafts, list_skill_drafts, approve_skill_draft, reject_skill_draft,
            preview::preview_start, preview::preview_stop, preview::preview_restart,
            preview::preview_get, preview::preview_list, preview::preview_set_visibility,
            preview::preview_open_popout, preview::preview_close_popout, preview::preview_logs,
            preview::preview_detect_framework
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                let mgr = state.preview_manager.clone();
                tauri::async_runtime::block_on(async {
                    mgr.shutdown_all().await;
                });
            }
        });
}
