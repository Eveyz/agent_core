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

        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dirs for: {path}"))?;
        }

        std::fs::write(path, content).with_context(|| format!("failed to write file: {path}"))?;

        Ok(format!(
            "Successfully wrote {} bytes to {path}",
            content.len()
        ))
    }
}
