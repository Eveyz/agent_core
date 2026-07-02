// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use agent_core::{
    AgentMode, Brain, RunCommand, RunEvent, RunManager, RunState,
    permission::ApprovalChoice,
};
use tauri::{AppHandle, Emitter, Manager, State};
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
}

// ── Frontend message type for session save/load ──────────────────────

#[derive(serde::Deserialize, serde::Serialize)]
struct FrontendMessage {
    role: String,
    content: String,
}

impl FrontendMessage {
    fn to_agent_message(&self) -> agent_core::types::Message {
        let role = match self.role.as_str() {
            "system" => agent_core::types::Role::System,
            "assistant" => agent_core::types::Role::Assistant,
            "tool" => agent_core::types::Role::Tool,
            _ => agent_core::types::Role::User,
        };
        agent_core::types::Message {
            role,
            content: Some(self.content.clone()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(serde::Serialize)]
struct FrontendSession {
    meta: agent_core::SessionMeta,
    messages: Vec<FrontendMessage>,
    event_log: Vec<agent_core::EventLogEntry>,
}

// ── Run lifecycle commands ───────────────────────────────────────────

/// Create a new Run for a user message. Returns the run_id.
/// The Run starts in `Created` state — the frontend should call
/// `start_run(run_id)` to begin execution.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    message: String,
    session_id: Option<String>,
) -> Result<String, String> {
    let manager = state.run_manager.lock().await;

    // Load history if resuming a session
    let mut history = vec![];
    let mut working_dir = None;
    if let Some(ref sid) = session_id {
        if let Ok(Some(sess)) = state.session_manager.resume(sid) {
            history = sess.messages;
            working_dir = Some(sess.meta.cwd);
        }
    }

    // Create the Run
    let run_id = manager
        .create_run_with_workdir(&message, session_id.clone(), working_dir, history)
        .await
        .map_err(|e| e.to_string())?;

    // Subscribe to events BEFORE starting, so we don't miss any.
    let mut event_rx = manager
        .subscribe(&run_id)
        .await
        .map_err(|e| e.to_string())?;

    // Start the Run
    manager
        .command(&run_id, RunCommand::Start)
        .await
        .map_err(|e| e.to_string())?;

    // Drop the manager lock so other commands can proceed while we stream events.
    drop(manager);

    // Spawn a task to forward events to the frontend.
    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if let Err(e) = app_handle_clone.emit("agent-event", &event) {
                        eprintln!("Failed to emit agent event: {}", e);
                    }
                    // Prevent WKWebView IPC flood which can drop the first events of a burst
                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                    
                    // Check if this is a terminal event
                    if matches!(
                        event.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    ) {
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

    Ok(run_id)
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
/// 1. Global pending approvals map (backward compat + subagent)
/// 2. Per-Run `ApprovalResolver` (direct path — no actor deadlock)
/// 3. Command channel broadcast (legacy fallback for paused runs)
#[tauri::command]
async fn approve_tool(
    state: State<'_, AppState>,
    run_id: Option<String>,
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

    // 1. Try global pending approvals first (used by subagents + legacy paths)
    {
        let pending_arc = agent_core::permission::global_pending_approvals();
        let mut pending = pending_arc.lock();
        if let Some(tx) = pending.remove(&prompt_id) {
            eprintln!("[approve_tool] resolved via global map");
            let _ = tx.send(choice_enum.clone());
            return Ok(());
        }
    }

    // 2. Try per-Run resolver directly (no command channel, no actor deadlock)
    if manager
        .resolve_approval(run_id.as_deref(), &prompt_id, choice_enum.clone())
        .await
    {
        eprintln!("[approve_tool] resolved via per-Run resolver");
        return Ok(());
    }

    eprintln!("[approve_tool] NOT resolved, falling back to command channel");

    // 3. Legacy command-channel fallback (for paused/edge-case runs)
    if let Some(id) = run_id {
        manager
            .command(&id, RunCommand::Approve {
                prompt_id,
                choice: choice_enum,
            })
            .await
            .map_err(|e| e.to_string())
    } else {
        let runs = manager.list_runs().await;
        for id in runs {
            let _ = manager
                .command(&id, RunCommand::Approve {
                    prompt_id: prompt_id.clone(),
                    choice: choice_enum.clone(),
                })
                .await;
        }
        Ok(())
    }
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
    config.save(&state.config_path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn switch_model(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut manager = state.run_manager.lock().await;
    manager.switch_model(&name).map_err(|e| e.to_string())
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
        let model = "default";
        let _ = sm
            .save_with_project(Some(&session_id), &messages, &cwd, model, Some(&project_id))
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
async fn save_session_messages(
    state: State<'_, AppState>,
    session_id: String,
    messages_json: String,
    cwd: String,
    model_used: String,
    process_time_ms: Option<u64>,
    thought_time_ms: Option<u64>,
    event_log_json: Option<String>,
) -> Result<(), String> {
    let sm = state.session_manager.clone();
    tokio::task::spawn_blocking(move || {
        let frontend_msgs: Vec<FrontendMessage> = serde_json::from_str(&messages_json)
            .map_err(|e| format!("Invalid messages JSON: {}", e))?;
        let messages: Vec<agent_core::types::Message> = frontend_msgs
            .iter()
            .map(|m| m.to_agent_message())
            .collect();
        let project_id = sm.get_project_id(&session_id).ok().flatten();
        let final_cwd = if project_id.as_deref() == Some("__adhoc_chat__") {
            let chat_dir = agent_core::paths::get_agverse_dir().join("chats").join(&session_id);
            chat_dir.to_string_lossy().to_string()
        } else {
            cwd
        };
        sm.save(Some(&session_id), &messages, &final_cwd, &model_used)
            .map_err(|e| e.to_string())?;
        if let (Some(pt), Some(tt)) = (process_time_ms, thought_time_ms) {
            sm.save_timing(&session_id, pt, tt)
                .map_err(|e| e.to_string())?;
        }
        if let Some(log_json) = event_log_json {
            sm.clear_event_log(&session_id)
                .map_err(|e| e.to_string())?;
            let events: Vec<serde_json::Value> = serde_json::from_str(&log_json)
                .map_err(|e| format!("Invalid event log JSON: {}", e))?;
            for event in &events {
                let turn_index = event["turn_index"].as_u64().unwrap_or(0) as usize;
                let event_type = event["event_type"].as_str().unwrap_or("unknown");
                let payload = event.get("payload").cloned().unwrap_or(serde_json::json!({}));
                let started_at = event["started_at"].as_str();
                let ended_at = event["ended_at"].as_str();
                sm.log_event(
                    &session_id, turn_index, event_type, &payload, started_at, ended_at,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("save_session_messages task failed: {e}"))?
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
        })
        .collect();
    Ok(FrontendSession {
        meta: session.meta,
        messages,
        event_log: session.event_log,
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

static SKILL_CACHE: Mutex<Option<(Instant, Vec<agent_core::skills::manifest::SkillManifest>)>> = Mutex::new(None);
const SKILL_CACHE_TTL: u64 = 30; // seconds

#[tauri::command]
async fn get_skills() -> Result<Vec<agent_core::skills::manifest::SkillManifest>, String> {
    // Check cache hit
    if let Some((cached_at, cached)) = SKILL_CACHE.lock().as_ref() {
        if cached_at.elapsed().as_secs() < SKILL_CACHE_TTL {
            return Ok(cached.clone());
        }
    }
    // Cache miss — scan from disk
    let mut manager = agent_core::skills::SkillManager::with_defaults();
    manager.scan().map_err(|e| e.to_string())?;
    let skills: Vec<agent_core::skills::manifest::SkillManifest> = manager.list().into_iter().cloned().collect();
    // Update cache
    *SKILL_CACHE.lock() = Some((Instant::now(), skills.clone()));
    Ok(skills)
}

#[tauri::command]
fn invalidate_skills_cache() -> Result<(), String> {
    // Clear the cache
    *SKILL_CACHE.lock() = None;
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

/// Inject skill content into a system prompt (content path).
fn inject_skill_content(
    brain: &Brain,
    skills: &[String],
    system_prompt: &str,
) -> String {
    let mut prompt = system_prompt.to_string();
    if let Some(ref sm) = brain.skill_manager {
        let mgr = sm.lock();
        for name in skills {
            if let Ok(Some(content)) = mgr.load_skill_context(name) {
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(&content);
            }
        }
    }
    prompt
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
    use agent_core::subagent::Subagent;

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
    // Inject skill content into the system prompt (content path).
    subagent_config.system_prompt =
        inject_skill_content(&brain, &def.skills, &subagent_config.system_prompt);

    let model_config = agent_core::agent_registry::build_model_config(&def, &brain.config);
    let permission_config =
        agent_core::agent_registry::build_permission_config(&def, &brain.config.permissions);

    // Build tool registry: inherit all if tools empty, else named subset.
    let registry = if def.tools.is_empty() {
        brain.build_tool_registry(agent_core::AgentMode::Build)
    } else {
        agent_core::ToolRegistry::from_names(&def.tools)
    };

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
        Some(def.id.clone()),
    );

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
async fn approve_skill_draft(name: String) -> Result<(), String> {
    let drafts_dir = skill_drafts_dir();
    let skills_dir = user_skills_dir();
    tokio::task::spawn_blocking(move || {
        agent_core::agent_registry::approve_draft(&drafts_dir, &skills_dir, &name)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("approve_skill_draft task failed: {e}"))?
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
    tauri::Builder::default()
        .setup(|app| {
            // Resolve home directory
            let agverse_dir = agent_core::paths::get_agverse_dir();
            let config_path = agverse_dir.join("config.toml");
            let config_path_str = config_path.to_string_lossy().to_string();

            // Ensure .agverse directory exists
            std::fs::create_dir_all(&agverse_dir)
                .unwrap_or_else(|e| eprintln!("warning: could not create ~/.agverse: {e}"));

            // Load config
            let config = if let Ok(cfg) = agent_core::config::Config::load(&config_path_str) {
                cfg
            } else if let Ok(cfg) = agent_core::config::Config::from_env() {
                let _ = cfg.save(&config_path_str);
                cfg
            } else {
                let mut default_config = agent_core::config::Config {
                    default_model: "default/default".to_string(),
                    providers: {
                        let mut p = std::collections::HashMap::new();
                        let mut m = std::collections::HashMap::new();
                        m.insert("default".to_string(), agent_core::config::ProviderModelEntry {
                            model_id: "gpt-4o-mini".to_string(),
                            temperature: None,
                            max_tokens: None,
                            system_prompt: None,
                            max_context_tokens: None,
                            reasoning_effort: None,
                            thinking_enabled: false,
                        });
                        p.insert("default".to_string(), agent_core::config::ProviderConfig {
                            name: "default".to_string(),
                            base_url: "https://api.openai.com/v1".to_string(),
                            api_key: "".to_string(),
                            max_context_tokens: 128000,
                            temperature: None,
                            max_tokens: None,
                            react_enabled: true,
                            system_prompt: None,
                            max_iterations: 100,
                            request_timeout_secs: 60,
                            models: m,
                        });
                        p
                    },
                    legacy_models: std::collections::HashMap::new(),
                    models: std::collections::HashMap::new(),
                    memory: None,
                    permissions: Default::default(),
                    mcp: Default::default(),
                    reflector_enabled: false,
                };
                default_config.rebuild_models();
                let _ = default_config.save(&config_path_str);
                default_config
            };

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

            let session_manager = Arc::new(
                agent_core::SessionManager::new(storage.clone())
            );

            // Build the RunManager
            let run_manager = RunManager::new(brain);

            app.manage(AppState {
                run_manager: Arc::new(AsyncMutex::new(run_manager)),
                config_path: config_path_str,
                project_manager,
                session_manager,
                storage: storage.clone(),
                agent_registry: agent_core::agent_registry::AgentRegistry::new(storage.clone()),
                workflow_cancels: Arc::new(AsyncMutex::new(std::collections::HashMap::new())),
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            send_message, approve_tool, abort_agent, replay_since,
            pause_run, resume_run, steer_run, cancel_steer, get_run_state,
            list_directory, search_files,
            get_config, save_config, switch_model, set_mode, get_mode,
            create_session, delete_session, rename_session,
            save_session_messages, resume_session,
            list_projects, create_project, delete_project, rename_project, open_in_explorer,
            list_git_branches, switch_git_branch, get_project_sessions,
            get_agverse_md, read_file, get_skills, invalidate_skills_cache,
            list_cronjobs, create_cronjob, update_cronjob, delete_cronjob, toggle_cronjob,
            list_available_tools,
            create_agent, list_agents, get_agent, update_agent, delete_agent,
            search_agent_memory, get_agent_history, run_agent_standalone,
            validate_workflow,
            create_workflow, list_workflows, get_workflow, save_workflow, delete_workflow,
            run_workflow, cancel_workflow_run, list_workflow_runs, get_workflow_run_results,
            generate_agent_skill_drafts, list_skill_drafts, approve_skill_draft, reject_skill_draft
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
