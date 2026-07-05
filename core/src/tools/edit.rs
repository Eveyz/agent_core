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
        let resolved_path = if let Some(sid) = session_id {
            crate::paths::redirect_if_artifact(file_path, sid)
        } else {
            std::path::PathBuf::from(file_path)
        };
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
