use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;

pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Replace exact string in a file. Args: file_path (string), old_string (string), new_string (string)"
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
        let file_path = args["file_path"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'file_path'"))?;
        let old_string = args["old_string"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'old_string'"))?;
        let new_string = args["new_string"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'new_string'"))?;

        let content = std::fs::read_to_string(file_path)
            .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", file_path, e))?;

        let count = content.matches(old_string).count();
        if count == 0 {
            anyhow::bail!("old_string not found in '{}'", file_path);
        }
        if count > 1 {
            anyhow::bail!("old_string found {} times in '{}'; provide more context to make it unique", count, file_path);
        }

        let new_content = content.replacen(old_string, new_string, 1);
        std::fs::write(file_path, &new_content)?;
        Ok(format!("Edited '{}': replaced 1 occurrence", file_path))
    }
}
