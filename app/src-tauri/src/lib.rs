// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use agent_core::{
    Brain, RunCommand, RunEvent, RunManager, RunState,
    permission::ApprovalChoice,
};
use tauri::{AppHandle, Emitter, Manager, State};
use std::sync::Arc;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

struct AppState {
    /// The RunManager owns the Brain and tracks all active Runs.
    run_manager: Arc<AsyncMutex<RunManager>>,
    config_path: String,
    project_manager: Arc<Mutex<agent_core::ProjectManager>>,
    session_manager: Arc<agent_core::SessionManager>,
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
#[tauri::command]
async fn steer_run(state: State<'_, AppState>, run_id: String, message: String) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    manager
        .command(&run_id, RunCommand::Steer { message })
        .await
        .map_err(|e| e.to_string())
}

/// Approve a tool execution that's waiting for approval.
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

    // Try global pending approvals first (used by subagents)
    {
        let pending_arc = agent_core::permission::global_pending_approvals();
        let mut pending = pending_arc.lock();
        if let Some(tx) = pending.remove(&prompt_id) {
            let _ = tx.send(choice_enum.clone());
            return Ok(());
        }
    }

    // If run_id is provided, route to that specific run.
    if let Some(id) = run_id {
        manager
            .command(&id, RunCommand::Approve {
                prompt_id,
                choice: choice_enum,
            })
            .await
            .map_err(|e| e.to_string())
    } else {
        // Broadcast to all active runs since we don't know which one owns the prompt
        let runs = manager.list_runs().await;
        for id in runs {
            let _ = manager.command(&id, RunCommand::Approve {
                prompt_id: prompt_id.clone(),
                choice: choice_enum.clone(),
            }).await;
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
                        
                        if is_dir && (file_name.starts_with('.') && file_name != ".agent_core" || ignore_dirs.contains(&file_name.as_str())) {
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
fn get_config(state: State<'_, AppState>) -> Result<agent_core::config::Config, String> {
    let manager = state.run_manager.blocking_lock();
    Ok(manager.brain().config.clone())
}

#[tauri::command]
fn save_config(state: State<'_, AppState>, config: agent_core::config::Config) -> Result<(), String> {
    config.save(&state.config_path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn switch_model(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let manager = state.run_manager.lock().await;
    // Check if there are active runs
    let runs = manager.list_runs().await;
    if !runs.is_empty() {
        return Err("Cannot switch model while runs are active".to_string());
    }
    // Model switching requires &mut Brain. The RunManager holds Arc<Brain>.
    // For now, we use the switch_model method on RunManager which handles
    // interior mutability internally.
    manager.switch_model(&name).map_err(|e| e.to_string())
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
        let cwd = project.path.clone();
        let model = "default";
        let session_id = sm
            .save_with_project(None, &messages, &cwd, model, Some(&project_id))
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
        sm.save(Some(&session_id), &messages, &cwd, &model_used)
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

// ── App entry point ──────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Resolve home directory
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".to_string());
            let agverse_dir = std::path::PathBuf::from(&home).join(".agverse");
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
                "~/.agverse/memory.db".to_string()
            };
            let storage = agent_core::memory::storage::Storage::new(&db_path)
                .expect("Failed to open storage database");
            let project_manager = Arc::new(Mutex::new(
                agent_core::ProjectManager::new(storage.clone())
            ));
            let session_manager = Arc::new(
                agent_core::SessionManager::new(storage)
            );

            // Build the RunManager
            let run_manager = RunManager::new(brain);

            app.manage(AppState {
                run_manager: Arc::new(AsyncMutex::new(run_manager)),
                config_path: config_path_str,
                project_manager,
                session_manager,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            send_message, approve_tool, abort_agent, replay_since,
            pause_run, resume_run, steer_run, get_run_state,
            list_directory, search_files,
            get_config, save_config, switch_model,
            create_session, delete_session, rename_session,
            save_session_messages, resume_session,
            list_projects, create_project, delete_project, rename_project, open_in_explorer,
            list_git_branches, switch_git_branch, get_project_sessions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
