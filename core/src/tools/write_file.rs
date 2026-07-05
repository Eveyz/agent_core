use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create a new file or completely overwrite an existing file. ONLY use this when writing a file from scratch or replacing its entire contents. Do NOT use this for small edits to existing files — use `edit` instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .context("missing required parameter 'path'")?;

        let content = args["content"]
            .as_str()
            .context("missing required parameter 'content'")?;

        let session_id = args.get("_session_id").and_then(|v| v.as_str());
        let resolved_path = if let Some(sid) = session_id {
            crate::paths::redirect_if_artifact(path, sid)
        } else {
            std::path::PathBuf::from(path)
        };
        let resolved_path_str = resolved_path.to_string_lossy().to_string();

        if let Some(parent) = resolved_path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create parent dirs for: {resolved_path_str}"))?;
            }
        }

        tokio::fs::write(&resolved_path, content)
            .await
            .with_context(|| format!("failed to write file: {resolved_path_str}"))?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            resolved_path_str
        ))
    }
}
