//! Agent tool: open a localhost preview panel for web content.

use std::sync::Arc;

use agent_core::tools::Tool;
use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::manager::PreviewManager;
use super::path_policy::normalize_entrypoint;
use super::resolve_preview_workspace;
use super::types::{PreviewMode, PreviewPlacement, PreviewStartRequest};

pub struct PreviewTool {
    preview_manager: Arc<PreviewManager>,
    project_manager: Arc<Mutex<agent_core::ProjectManager>>,
}

impl PreviewTool {
    pub fn new(
        preview_manager: Arc<PreviewManager>,
        project_manager: Arc<Mutex<agent_core::ProjectManager>>,
    ) -> Self {
        Self {
            preview_manager,
            project_manager,
        }
    }
}

#[async_trait]
impl Tool for PreviewTool {
    fn name(&self) -> &str {
        "preview"
    }

    fn description(&self) -> &str {
        "Open a live localhost preview of web content in an embedded browser panel. \
         Call this after creating or updating HTML, CSS, JavaScript, or other static \
         web files so the user can see the result immediately. Hot-reload is automatic \
         when files change. Works in default chat mode (files under the session folder) \
         and in registered project workspaces. For SPA frameworks (React, Svelte, Vite, etc.) \
         that need a dev server, write the files first and use `shell` to start the dev server — \
         static preview serves files directly without Node."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entrypoint": {
                    "type": "string",
                    "description": "HTML entry file. Prefer a path relative to the working directory (e.g. index.html). Absolute paths are accepted when they lie inside the workspace."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let working_dir = args
            .get("_working_dir")
            .and_then(|v| v.as_str())
            .context("preview requires a working directory (_working_dir)")?;
        let session_id = args.get("_session_id").and_then(|v| v.as_str());

        let entrypoint_raw = args
            .get("entrypoint")
            .and_then(|v| v.as_str())
            .unwrap_or("index.html");

        let (workspace_id, root) = {
            let pm = self.project_manager.lock();
            resolve_preview_workspace(working_dir, session_id, &pm)
                .map_err(|e| anyhow::anyhow!(e))?
        };

        normalize_entrypoint(&root, entrypoint_raw)
            .map_err(|e| anyhow::anyhow!("invalid entrypoint '{entrypoint_raw}': {e}"))?;

        // Replace an existing preview for this workspace/session instead of failing on quota.
        if let Some(existing_id) = self
            .preview_manager
            .find_active(&workspace_id, session_id)
        {
            self.preview_manager
                .stop(existing_id)
                .await
                .with_context(|| "failed to replace existing preview session")?;
        }

        let request = PreviewStartRequest {
            workspace_id,
            session_id: session_id.map(str::to_string),
            mode: PreviewMode::Static,
            entrypoint: Some(entrypoint_raw.to_string()),
            approved_command: None,
            placement: Some(PreviewPlacement::Split),
        };

        let descriptor = self
            .preview_manager
            .start(root, request)
            .await
            .with_context(|| format!("failed to start preview for '{entrypoint_raw}'"))?;

        Ok(serde_json::to_string_pretty(&descriptor)?)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    fn temp_storage() -> agent_core::memory::storage::Storage {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        // Keep TempDir alive by leaking — tests are short-lived.
        std::mem::forget(dir);
        agent_core::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn resolves_adhoc_chat_session_directory() {
        let storage = temp_storage();
        let pm = agent_core::ProjectManager::new(storage);
        let session_id = "test-session-123";
        let session_cwd = agent_core::paths::session_dir(session_id);
        std::fs::create_dir_all(&session_cwd).unwrap();
        let canonical = session_cwd.canonicalize().unwrap();

        let (workspace_id, root) =
            super::super::resolve_preview_workspace(
                &canonical.to_string_lossy(),
                Some(session_id),
                &pm,
            )
            .unwrap();

        assert_eq!(workspace_id, "__adhoc_chat__");
        assert_eq!(root, canonical);
    }

    #[test]
    fn falls_back_to_adhoc_chat_for_unregistered_cwd() {
        let storage = temp_storage();
        let pm = agent_core::ProjectManager::new(storage);
        let dir = TempDir::new().unwrap();
        let canonical = dir.path().canonicalize().unwrap();

        let (workspace_id, root) = super::super::resolve_preview_workspace(
            &canonical.to_string_lossy(),
            None,
            &pm,
        )
        .unwrap();

        assert_eq!(workspace_id, "__adhoc_chat__");
        assert_eq!(root, canonical);
    }
}
