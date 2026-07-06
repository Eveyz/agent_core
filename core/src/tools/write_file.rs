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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("write_file_tool_tests_{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_write_file_basic() {
        let path = tmp_dir().join("basic.txt");
        let out = WriteFileTool
            .execute(json!({
                "path": path.to_string_lossy(),
                "content": "hello\nworld\n"
            }))
            .await
            .unwrap();
        assert!(out.contains("Successfully wrote 12 bytes"));
        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(on_disk, "hello\nworld\n");
    }

    #[tokio::test]
    async fn test_write_file_creates_parent_dirs() {
        let path = tmp_dir().join("nested").join("deep").join("file.txt");
        let _ = WriteFileTool
            .execute(json!({
                "path": path.to_string_lossy(),
                "content": "x"
            }))
            .await
            .unwrap();
        assert!(path.exists());
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "x");
    }

    #[tokio::test]
    async fn test_write_file_overwrites() {
        let path = tmp_dir().join("overwrite.txt");
        tokio::fs::write(&path, "old").await.unwrap();
        let _ = WriteFileTool
            .execute(json!({
                "path": path.to_string_lossy(),
                "content": "new"
            }))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "new");
    }

    #[tokio::test]
    async fn test_write_file_missing_params_errors() {
        let res1 = WriteFileTool
            .execute(json!({"content": "x"}))
            .await;
        assert!(res1.is_err());
        let res2 = WriteFileTool
            .execute(json!({"path": "/tmp/whatever"}))
            .await;
        assert!(res2.is_err());
    }

    #[tokio::test]
    async fn test_write_file_empty_content() {
        let path = tmp_dir().join("empty.txt");
        let _ = WriteFileTool
            .execute(json!({
                "path": path.to_string_lossy(),
                "content": ""
            }))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "");
        let meta = tokio::fs::metadata(&path).await.unwrap();
        assert_eq!(meta.len(), 0);
    }
}
