//! Localhost preview subsystem for Agverse.

pub mod gateway;
pub mod manager;
pub mod path_policy;
pub mod process;
pub mod tool;
pub mod types;
pub mod watcher;

pub use manager::PreviewManager;
pub use types::*;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::AppState;

/// Resolve the preview root directory and workspace id from a run's working directory.
///
/// Chat sessions use `~/.agverse/chats/<session_id>/` as cwd with workspace id
/// `__adhoc_chat__`. Registered projects match by canonical path.
pub(crate) fn resolve_preview_workspace(
    working_dir: &str,
    session_id: Option<&str>,
    project_manager: &agent_core::ProjectManager,
) -> Result<(String, PathBuf), String> {
    let root = PathBuf::from(working_dir);
    let canonical = root
        .canonicalize()
        .map_err(|e| format!("invalid working directory: {working_dir}: {e}"))?;

    if let Ok(projects) = project_manager.list() {
        for project in projects {
            if project.id == "__adhoc_chat__" {
                continue;
            }
            let project_path = PathBuf::from(&project.path);
            if let Ok(project_canonical) = project_path.canonicalize() {
                if project_canonical == canonical {
                    return Ok((project.id, canonical));
                }
            } else if project.path == working_dir {
                return Ok((project.id, canonical));
            }
        }
    }

    let chats_dir = agent_core::paths::get_agverse_dir().join("chats");
    let chats_root = chats_dir.canonicalize().unwrap_or(chats_dir);
    if canonical.starts_with(&chats_root) {
        return Ok(("__adhoc_chat__".to_string(), canonical));
    }

    if let Some(sid) = session_id {
        let session_chat = chats_root.join(sid);
        let _ = std::fs::create_dir_all(&session_chat);
        if let Ok(session_canonical) = session_chat.canonicalize() {
            if session_canonical == canonical {
                return Ok(("__adhoc_chat__".to_string(), canonical));
            }
        }
    }

    // Default chat workspace — preview any valid cwd without a registered project.
    Ok(("__adhoc_chat__".to_string(), canonical))
}

fn resolve_workspace_path(
    state: &AppState,
    workspace_id: &str,
    session_id: Option<&str>,
) -> Result<PathBuf, String> {
    if workspace_id == "__adhoc_chat__" {
        if let Some(sid) = session_id {
            let chat_dir = agent_core::paths::get_agverse_dir().join("chats").join(sid);
            let _ = std::fs::create_dir_all(&chat_dir);
            return Ok(chat_dir);
        }
    }

    let pm = state.project_manager.lock();
    let projects = pm.list().map_err(|e| e.to_string())?;
    let project = projects
        .into_iter()
        .find(|p| p.id == workspace_id)
        .ok_or_else(|| "workspace not found".to_string())?;
    Ok(PathBuf::from(project.path))
}

#[tauri::command]
pub async fn preview_start(
    state: State<'_, AppState>,
    request: PreviewStartRequest,
) -> Result<PreviewDescriptor, String> {
    let root = resolve_workspace_path(
        &state,
        &request.workspace_id,
        request.session_id.as_deref(),
    )?;
    state
        .preview_manager
        .start(root, request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_stop(
    state: State<'_, AppState>,
    preview_id: Uuid,
) -> Result<(), String> {
    state
        .preview_manager
        .stop(preview_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_restart(
    state: State<'_, AppState>,
    preview_id: Uuid,
) -> Result<PreviewDescriptor, String> {
    state
        .preview_manager
        .restart(preview_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_get(
    state: State<'_, AppState>,
    preview_id: Uuid,
) -> Option<PreviewDescriptor> {
    state.preview_manager.get(preview_id)
}

#[tauri::command]
pub fn preview_list(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Vec<PreviewDescriptor> {
    state.preview_manager.list(&workspace_id)
}

#[tauri::command]
pub async fn preview_set_visibility(
    state: State<'_, AppState>,
    request: PreviewVisibilityRequest,
) -> Result<PreviewDescriptor, String> {
    state
        .preview_manager
        .set_visibility(request.preview_id, request.placement)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_open_popout(
    state: State<'_, AppState>,
    preview_id: Uuid,
) -> Result<(), String> {
    state
        .preview_manager
        .open_popout(preview_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_close_popout(
    state: State<'_, AppState>,
    preview_id: Uuid,
) -> Result<(), String> {
    state
        .preview_manager
        .close_popout(preview_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_logs(
    state: State<'_, AppState>,
    request: PreviewLogsRequest,
) -> Result<LogPage, String> {
    state
        .preview_manager
        .logs(request.preview_id, request.cursor)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_detect_framework(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<FrameworkDetection, String> {
    let root = resolve_workspace_path(&state, &workspace_id, None)?;
    state
        .preview_manager
        .detect_framework(&root)
        .await
        .map_err(|e| e.to_string())
}

pub fn create_manager() -> Arc<PreviewManager> {
    Arc::new(PreviewManager::new(PreviewQuotas::default()))
}
