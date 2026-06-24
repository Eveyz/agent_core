use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;
use similar::TextDiff;

/// Stream-editor tool: applies a regex substitution (`s/pattern/replacement/flags`)
/// to a file, in place by default. Useful for bulk find-and-replace, pattern-based
/// edits across many lines, or transformations that `edit` (exact-string match)
/// can't express. Backed by the real `sed` binary for correctness.
pub struct SedTool;

#[async_trait::async_trait]
impl Tool for SedTool {
    fn name(&self) -> &str {
        "sed"
    }

    fn description(&self) -> &str {
        "Apply a sed substitution (s/pattern/replacement/flags) to a file in place. \
Use for regex-based find-and-replace, bulk edits across many lines, or pattern \
transformations that the 'edit' tool (exact-string match) cannot handle. \
The expression is passed directly to sed, so use standard sed/POSIX regex syntax. \
Examples: s/foo/bar/g, s/\\\\t/    /g, s/^#// to uncomment. \
By default writes back to the file; set preview=true to only show the diff."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Absolute path to the file to edit"},
                "expression": {"type": "string", "description": "A sed substitution expression, e.g. 's/foo/bar/g'"},
                "preview": {"type": "boolean", "description": "If true, show the diff without writing to the file (default: false)"}
            },
            "required": ["file_path", "expression"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'file_path'"))?;
        let expression = args["expression"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'expression'"))?;
        let preview = args["preview"].as_bool().unwrap_or(false);

        // Basic validation: the expression must start with s/ to be a substitution.
        if !expression.starts_with("s/") {
            anyhow::bail!(
                "sed tool only supports substitution expressions (must start with 's/'); got: {}",
                expression
            );
        }

        let old_content = std::fs::read_to_string(file_path)
            .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", file_path, e))?;

        // Run sed on the content via stdin so we don't rely on platform-specific
        // in-place flags (-i behaves differently on GNU vs BSD sed). We feed the
        // content to `sed -E '<expr>'` and capture stdout.
        let output = tokio::process::Command::new("sed")
            .arg("-E")
            .arg(expression)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        use tokio::io::AsyncWriteExt;
        let mut child = output;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(old_content.as_bytes()).await?;
            // drop stdin to signal EOF
        }
        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            anyhow::bail!("sed failed: {}", stderr.trim());
        }

        let new_content = String::from_utf8_lossy(&result.stdout).to_string();

        if new_content == old_content {
            return Ok(format!(
                "No changes: expression '{}' matched nothing in '{}'",
                expression, file_path
            ));
        }

        if !preview {
            std::fs::write(file_path, &new_content)?;
        }

        // Generate a unified diff so the UI can show what changed.
        let diff = TextDiff::from_lines(&old_content, &new_content);
        let mut diff_bytes = Vec::new();
        diff.unified_diff()
            .header(file_path, file_path)
            .context_radius(3)
            .to_writer(&mut diff_bytes)?;
        let diff_str = String::from_utf8(diff_bytes)?;

        let additions = diff_str
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        let deletions = diff_str
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count();

        let action = if preview {
            "Preview (not written)"
        } else {
            "Applied"
        };
        Ok(format!(
            "{} sed '{}' to '{}': +{} additions, -{} deletions\n{}",
            action, expression, file_path, additions, deletions, diff_str
        ))
    }
}
