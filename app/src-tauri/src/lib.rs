// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use agent_core::{Agent, ToolExecutionMode};
use tauri::{AppHandle, Emitter, Manager, State};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

struct AppState {
    agent: Arc<AsyncMutex<Agent>>,
    pending_approvals: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<agent_core::ApprovalChoice>>>>,
    config_path: String,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Try multiple paths to find config.toml depending on where the app is launched from
            let config_paths = ["config.toml", "../config.toml", "../../config.toml"];
            let mut loaded_config: Option<(String, agent_core::config::Config)> = None;
            for path in &config_paths {
                if let Ok(config) = agent_core::config::Config::load(path) {
                    loaded_config = Some((path.to_string(), config));
                    break;
                }
            }

            let (config_path, config) = loaded_config
                .or_else(|| {
                    agent_core::config::Config::from_env()
                        .ok()
                        .map(|c| ("env".to_string(), c))
                })
                .unwrap_or_else(|| {
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
                    ("default".to_string(), default_config)
                });

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

            app.manage(AppState {
                agent: Arc::new(AsyncMutex::new(agent)),
                pending_approvals,
                config_path,
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![send_message, approve_tool, list_directory, get_config, save_config, switch_model])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
