use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files and directories matching a glob pattern. Returns matching paths sorted by modification time. Similar to 'find' with patterns."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match, e.g. '**/*.rs', 'src/**/*.ts', '*.toml'"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search from (default: current directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern = args["pattern"].as_str().context("missing 'pattern'")?;
        let path = args["path"].as_str().unwrap_or(".");

        let full_pattern = if path == "." {
            pattern.to_string()
        } else {
            format!("{}/{}", path.trim_end_matches('/'), pattern)
        };

        let paths: Vec<_> = glob::glob(&full_pattern)
            .with_context(|| format!("invalid glob pattern: {full_pattern}"))?
            .filter_map(|entry| entry.ok())
            .collect();

        if paths.is_empty() {
            return Ok("No files matched the pattern.".to_string());
        }

        let mut entries: Vec<(std::time::SystemTime, String)> = Vec::new();
        for p in &paths {
            let meta = std::fs::metadata(p).ok();
            let modified = meta
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((modified, p.display().to_string()));
        }

        entries.sort_by(|a, b| b.0.cmp(&a.0));

        let output: Vec<String> = entries.into_iter().map(|(_, path)| path).collect();
        Ok(output.join("\n"))
    }
}
