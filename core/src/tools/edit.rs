use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;
use similar::TextDiff;

pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Modify an existing file by replacing an exact string. Read the file first to get the exact old_string, then provide old_string and new_string. Use this for ALL edits to existing files — never use write_file for small changes."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Absolute path to file"},
                "old_string": {"type": "string", "description": "Exact string to replace"},
                "new_string": {"type": "string", "description": "Replacement string"}
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'file_path'"))?;
        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'old_string'"))?;
        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'new_string'"))?;

        let session_id = args.get("_session_id").and_then(|v| v.as_str());
        let working_dir = args.get("_working_dir").and_then(|v| v.as_str());
        let resolved_path = crate::paths::resolve_tool_path(file_path, session_id, working_dir);
        let resolved_path_str = resolved_path.to_string_lossy().to_string();

        let old_content = tokio::fs::read_to_string(&resolved_path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", resolved_path_str, e))?;

        let count = old_content.matches(old_string).count();
        if count == 0 {
            anyhow::bail!("old_string not found in '{}'", resolved_path_str);
        }
        if count > 1 {
            anyhow::bail!(
                "old_string found {} times in '{}'; provide more context to make it unique",
                count,
                resolved_path_str
            );
        }

        let new_content = old_content.replacen(old_string, new_string, 1);

        // Atomic write: write to a temp file first, then rename into place.
        // This prevents file corruption if the write is interrupted mid-flight.
        let tmp_path = resolved_path.with_extension("tmp.edit");
        tokio::fs::write(&tmp_path, &new_content).await?;
        tokio::fs::rename(&tmp_path, &resolved_path).await?;

        // Compute the line range of the edited region (1-based) in the
        // *original* file, so the UI can show "Edited lines L12–L18".
        let byte_offset = old_content.find(old_string).unwrap_or(0);
        let line_start = old_content[..byte_offset].matches('\n').count() + 1;
        let line_end = line_start + old_string.matches('\n').count();

        // Generate unified diff for the change
        let diff = TextDiff::from_lines(&old_content, &new_content);
        let mut diff_bytes = Vec::new();
        diff.unified_diff()
            .header(file_path, file_path)
            .context_radius(3)
            .to_writer(&mut diff_bytes)?;
        let diff_str = String::from_utf8(diff_bytes)?;

        // Count additions / deletions (exclude header lines)
        let additions = diff_str
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        let deletions = diff_str
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count();

        Ok(format!(
            "Edited lines {}–{} ({} {}, {} {})\n{}",
            line_start,
            line_end,
            additions,
            if additions == 1 {
                "addition"
            } else {
                "additions"
            },
            deletions,
            if deletions == 1 {
                "deletion"
            } else {
                "deletions"
            },
            diff_str
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

    fn tmp_path(name: &str) -> PathBuf {
        // Per-test unique subdir so parallel calls don't clobber each other.
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("edit_tool_tests_{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[tokio::test]
    async fn test_edit_basic_replacement() {
        let path = tmp_path("basic.txt");
        tokio::fs::write(&path, "hello world\n").await.unwrap();
        let out = EditTool
            .execute(json!({
                "file_path": path.to_string_lossy(),
                "old_string": "world",
                "new_string": "rust"
            }))
            .await
            .unwrap();
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(after, "hello rust\n");
        assert!(out.contains("Edited lines"));
    }

    #[tokio::test]
    async fn test_edit_no_temp_file_left() {
        let path = tmp_path("temp_check.txt");
        tokio::fs::write(&path, "a\n").await.unwrap();
        let _ = EditTool
            .execute(json!({
                "file_path": path.to_string_lossy(),
                "old_string": "a",
                "new_string": "b"
            }))
            .await
            .unwrap();
        let tmp = path.with_extension("tmp.edit");
        assert!(!tmp.exists(), "temp edit file should not remain");
    }

    #[tokio::test]
    async fn test_edit_old_string_not_found_errors() {
        let path = tmp_path("missing.txt");
        tokio::fs::write(&path, "content\n").await.unwrap();
        let res = EditTool
            .execute(json!({
                "file_path": path.to_string_lossy(),
                "old_string": "nope",
                "new_string": "x"
            }))
            .await;
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("old_string not found"));
    }

    #[tokio::test]
    async fn test_edit_multiple_matches_errors() {
        let path = tmp_path("multi.txt");
        tokio::fs::write(&path, "dup\ndup\ndup\n").await.unwrap();
        let res = EditTool
            .execute(json!({
                "file_path": path.to_string_lossy(),
                "old_string": "dup",
                "new_string": "x"
            }))
            .await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("found 3 times") || msg.contains("found 3"),
            "msg = {msg}"
        );
    }

    #[tokio::test]
    async fn test_edit_missing_params_errors() {
        let res = EditTool
            .execute(json!({"file_path": "/tmp", "old_string": "a"}))
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_edit_multiline_string() {
        let path = tmp_path("multiline.txt");
        let original = "fn main() {\n    println!(\"hi\");\n}\n";
        tokio::fs::write(&path, original).await.unwrap();
        let _ = EditTool
            .execute(json!({
                "file_path": path.to_string_lossy(),
                "old_string": "    println!(\"hi\");",
                "new_string": "    println!(\"hello\");\n    println!(\"world\");"
            }))
            .await
            .unwrap();
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(after.contains("hello"));
        assert!(after.contains("world"));
    }
}
