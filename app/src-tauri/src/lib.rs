// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use agent_core::{AgentBuilder, Agent, ToolExecutionMode};
use tauri::{AppHandle, Emitter, Manager, State};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

struct AppState {
    agent: Arc<AsyncMutex<Agent>>,
}

#[tauri::command]
async fn send_message(state: State<'_, AppState>, app_handle: AppHandle, message: String) -> Result<String, String> {
    let mut agent = state.agent.lock().await;
    
    let app_handle_clone = app_handle.clone();
    let pending_approvals = agent.pending_approvals.clone();
    
    let result = agent.run_with_events(&message, move |event| {
        // Auto-approve tools for now
        if let agent_core::AgentEvent::ApprovalRequired { prompt_id, .. } = &event {
            if let Some(ref approvals) = pending_approvals {
                if let Ok(mut map) = approvals.lock() {
                    if let Some(tx) = map.remove(prompt_id) {
                        let _ = tx.send(agent_core::ApprovalChoice::AllowSession);
                    }
                }
            }
        }
        
        if let Err(e) = app_handle_clone.emit("agent-event", event) {
            eprintln!("Failed to emit agent event: {}", e);
        }
    }).await;
    
    match result {
        Ok(res) => Ok(res),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config = agent_core::config::Config {
                default_model: "default".to_string(),
                models: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("default".to_string(), agent_core::config::ModelConfig::default());
                    m
                },
                memory: None,
                permissions: Default::default(),
                mcp: Default::default(),
            };
            // Try multiple paths to find config.toml depending on where the app is launched from
            let builder = agent_core::AgentBuilder::from_config("config.toml")
                .or_else(|_| agent_core::AgentBuilder::from_config("../config.toml"))
                .or_else(|_| agent_core::AgentBuilder::from_config("../../config.toml"))
                .or_else(|_| agent_core::AgentBuilder::from_env())
                .unwrap_or_else(|_| agent_core::AgentBuilder::with_config(config));
            let agent = builder.with_tool_execution_mode(ToolExecutionMode::Parallel).build().expect("Failed to build agent");
            
            app.manage(AppState {
                agent: Arc::new(AsyncMutex::new(agent)),
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![send_message])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
