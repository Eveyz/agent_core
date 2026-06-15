// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use agent_core::{Agent, ToolExecutionMode};
use tauri::{AppHandle, Emitter, Manager, State};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

struct AppState {
    agent: Arc<AsyncMutex<Agent>>,
    pending_approvals: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<agent_core::ApprovalChoice>>>>,
    config_path: String,
    project_manager: Arc<std::sync::Mutex<agent_core::ProjectManager>>,
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

#[tauri::command]
async fn send_message(state: State<'_, AppState>, app_handle: AppHandle, message: String) -> Result<String, String> {
    let mut agent = state.agent.lock().await;
    
    let app_handle_clone = app_handle.clone();
    let result = agent.run_with_events(&message, move |event| {
        if let Err(e) = app_handle_clone.emit("agent-event", event) {
            eprintln!("Failed to emit agent event: {}", e);
        }
    }).await;
    
    match result {
        Ok(res) => Ok(res),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn list_directory(path: Option<String>) -> Result<Vec<std::collections::HashMap<String, String>>, String> {
    let target_path = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };

    let mut entries = Vec::new();
    let dir_entries = std::fs::read_dir(&target_path).map_err(|e| e.to_string())?;

    for entry in dir_entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        let mut map = std::collections::HashMap::new();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        map.insert("name".to_string(), name);
        map.insert("type".to_string(), file_type.to_string());
        entries.push(map);
    }

    // Sort directories first, then files
    entries.sort_by(|a, b| {
        let a_is_dir = a.get("type").map(|t| t == "directory").unwrap_or(false);
        let b_is_dir = b.get("type").map(|t| t == "directory").unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.get("name").cmp(&b.get("name")),
        }
    });

    Ok(entries)
}

#[tauri::command]
fn approve_tool(state: State<'_, AppState>, prompt_id: String, choice: String) {
    if let Ok(mut map) = state.pending_approvals.lock() {
        if let Some(tx) = map.remove(&prompt_id) {
            let choice_enum = match choice.as_str() {
                "allow_session" => agent_core::ApprovalChoice::AllowSession,
                "allow_persistent" => agent_core::ApprovalChoice::AllowPersistent,
                _ => agent_core::ApprovalChoice::Deny,
            };
            let _ = tx.send(choice_enum);
        }
    }
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<agent_core::config::Config, String> {
    agent_core::config::Config::load(&state.config_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(state: State<'_, AppState>, config: agent_core::config::Config) -> Result<(), String> {
    config.save(&state.config_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn switch_model(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut agent = state.agent.lock().await;
    // Reload config so newly added models are available
    if let Ok(fresh_config) = agent_core::config::Config::load(&state.config_path) {
        agent.config = fresh_config;
    }
    agent.switch_model(&name).map_err(|e| e.to_string())
}

// ── Session Commands ─────────────────────────────────────────────────

#[tauri::command]
fn create_session(state: State<'_, AppState>, project_id: String) -> Result<agent_core::SessionMeta, String> {
    let messages: Vec<agent_core::types::Message> = vec![];
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    let project = pm.get(&project_id).map_err(|e| e.to_string())?
        .ok_or_else(|| "Project not found".to_string())?;
    let cwd = project.path.clone();
    let model = "default";
    let session_id = state.session_manager.save_with_project(None, &messages, &cwd, model, Some(&project_id))
        .map_err(|e| e.to_string())?;
    let _ = state.session_manager.rename(&session_id, "New Session");
    let meta = state.session_manager.get_meta(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found after creation".to_string())?;
    Ok(meta)
}

#[tauri::command]
fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    state.session_manager.delete(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_session(state: State<'_, AppState>, session_id: String, new_title: String) -> Result<bool, String> {
    state.session_manager.rename(&session_id, &new_title).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_session_messages(
    state: State<'_, AppState>,
    session_id: String,
    messages_json: String,
    cwd: String,
    model_used: String,
    process_time_ms: Option<u64>,
    thought_time_ms: Option<u64>,
    event_log_json: Option<String>,
) -> Result<(), String> {
    let frontend_msgs: Vec<FrontendMessage> = serde_json::from_str(&messages_json)
        .map_err(|e| format!("Invalid messages JSON: {}", e))?;
    let messages: Vec<agent_core::types::Message> = frontend_msgs
        .iter()
        .map(|m| m.to_agent_message())
        .collect();
    state.session_manager.save(Some(&session_id), &messages, &cwd, &model_used)
        .map_err(|e| e.to_string())?;

    // Save timing data
    if let (Some(pt), Some(tt)) = (process_time_ms, thought_time_ms) {
        state.session_manager.save_timing(&session_id, pt, tt)
            .map_err(|e| e.to_string())?;
    }

    // Save event log (replace all existing events for this session)
    if let Some(log_json) = event_log_json {
        state.session_manager.clear_event_log(&session_id)
            .map_err(|e| e.to_string())?;
        let events: Vec<serde_json::Value> = serde_json::from_str(&log_json)
            .map_err(|e| format!("Invalid event log JSON: {}", e))?;
        for event in &events {
            let turn_index = event["turn_index"].as_u64().unwrap_or(0) as usize;
            let event_type = event["event_type"].as_str().unwrap_or("unknown");
            let payload = event.get("payload").cloned().unwrap_or(serde_json::json!({}));
            let started_at = event["started_at"].as_str();
            let ended_at = event["ended_at"].as_str();
            state.session_manager.log_event(
                &session_id, turn_index, event_type, &payload, started_at, ended_at,
            ).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
fn resume_session(state: State<'_, AppState>, session_id: String) -> Result<FrontendSession, String> {
    let session = state.session_manager.resume(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;
    let messages: Vec<FrontendMessage> = session.messages.iter().map(|m| FrontendMessage {
        role: m.role.to_string(),
        content: m.content.clone().unwrap_or_default(),
    }).collect();
    Ok(FrontendSession {
        meta: session.meta,
        messages,
        event_log: session.event_log,
    })
}

// ── Project Commands ─────────────────────────────────────────────────

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Result<Vec<agent_core::Project>, String> {
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    pm.list().map_err(|e| e.to_string())
}

#[tauri::command]
fn create_project(state: State<'_, AppState>, path: String) -> Result<agent_core::Project, String> {
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    pm.create(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_project(state: State<'_, AppState>, project_id: String) -> Result<bool, String> {
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    pm.delete(&project_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_project(state: State<'_, AppState>, project_id: String, new_name: String) -> Result<bool, String> {
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    pm.rename(&project_id, &new_name).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_git_branches(path: String) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&path)
        .output()
        .map_err(|e| format!("Failed to run git branch: {}", e))?;
    if !output.status.success() {
        return Err(format!("git branch failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(branches)
}

#[tauri::command]
fn switch_git_branch(path: String, branch: String) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args(["checkout", &branch])
        .current_dir(&path)
        .output()
        .map_err(|e| format!("Failed to run git checkout: {}", e))?;
    if !output.status.success() {
        return Err(format!("git checkout failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

#[tauri::command]
fn get_project_sessions(state: State<'_, AppState>, project_id: String) -> Result<Vec<agent_core::session::SessionMeta>, String> {
    let pm = state.project_manager.lock().map_err(|e| e.to_string())?;
    pm.list_sessions(&project_id).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Resolve home directory (handles both Unix HOME and Windows USERPROFILE)
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".to_string());
            let agverse_dir = std::path::PathBuf::from(&home).join(".agverse");
            let config_path = agverse_dir.join("config.toml");
            let config_path_str = config_path.to_string_lossy().to_string();

            // Ensure .agverse directory exists
            std::fs::create_dir_all(&agverse_dir)
                .unwrap_or_else(|e| eprintln!("warning: could not create ~/.agverse: {e}"));

            // Try to load config from ~/.agverse/config.toml first, then fallbacks
            let config = if let Ok(cfg) = agent_core::config::Config::load(&config_path_str) {
                cfg
            } else if let Ok(cfg) = agent_core::config::Config::from_env() {
                // Save the env-based config so it persists for next launch
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
                };
                default_config.rebuild_models();
                // Save default config so next launch finds it
                let _ = default_config.save(&config_path_str);
                default_config
            };

            let builder = agent_core::AgentBuilder::with_config(config);
            let mut agent = builder.with_tool_execution_mode(ToolExecutionMode::Parallel).build().expect("Failed to build agent");

            // Register subagent tools so the LLM can spawn child agents
            {
                let model_config = agent.current_model_config().clone();
                let tool_names: Vec<String> = agent
                    .tool_registry()
                    .list_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let reg = agent.tool_registry_mut();
                agent_core::tools::subagent::register_subagent_tools(
                    reg,
                    model_config,
                    tool_names,
                    None, // session manager not wired up in Tauri app yet
                );
            }

            let pending_approvals = agent.pending_approvals_clone();

            // Initialize ProjectManager using the same SQLite path as memory
            let db_path = if let Some(mem_config) = agent.config().memory.as_ref() {
                mem_config.db_path.clone()
            } else {
                "~/.agverse/memory.db".to_string()
            };
            let storage = agent_core::memory::storage::Storage::new(&db_path)
                .expect("Failed to open storage database");
            let project_manager = Arc::new(std::sync::Mutex::new(
                agent_core::ProjectManager::new(storage.clone())
            ));
            let session_manager = Arc::new(
                agent_core::SessionManager::new(storage)
            );

            app.manage(AppState {
                agent: Arc::new(AsyncMutex::new(agent)),
                pending_approvals,
                config_path: config_path_str,
                project_manager,
                session_manager,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            send_message, approve_tool, list_directory,
            get_config, save_config, switch_model,
            create_session, delete_session, rename_session,
            save_session_messages, resume_session,
            list_projects, create_project, delete_project, rename_project, open_in_explorer,
            list_git_branches, switch_git_branch, get_project_sessions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
