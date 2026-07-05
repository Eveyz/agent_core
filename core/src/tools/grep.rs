use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using regex pattern. Returns matching lines with file paths and line numbers. Similar to 'grep -rn'."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "The directory or file path to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File extension filter, e.g. '*.rs' or '*.py'"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 50)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern = args["pattern"].as_str().context("missing 'pattern'")?;
        let path = args["path"].as_str().unwrap_or(".");
        let include = args["include"].as_str();
        let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;

        let regex = regex::Regex::new(pattern)
            .with_context(|| format!("invalid regex pattern: {pattern}"))?;

        let mut results: Vec<String> = Vec::new();
        search_path(
            std::path::Path::new(path),
            &regex,
            include,
            &mut results,
            max_results,
        )
        .await?;

        if results.is_empty() {
            return Ok("No matches found.".to_string());
        }

        Ok(results.join("\n"))
    }
}

async fn search_path(
    path: &std::path::Path,
    regex: &regex::Regex,
    include: Option<&str>,
    results: &mut Vec<String>,
    max: usize,
) -> Result<()> {
    if results.len() >= max {
        return Ok(());
    }

    if path.is_file() {
        if let Some(ext_filter) = include {
            let pattern = glob::Pattern::new(ext_filter)
                .with_context(|| format!("invalid include pattern: {ext_filter}"))?;
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                && !pattern.matches(file_name)
            {
                return Ok(());
            }
        }

        if let Ok(content) = tokio::fs::read_to_string(path).await {
            for (line_num, line) in content.lines().enumerate() {
                if results.len() >= max {
                    break;
                }
                if regex.is_match(line) {
                    results.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    } else if path.is_dir() {
        let mut entries = tokio::fs::read_dir(path)
            .await
            .with_context(|| format!("failed to read dir: {}", path.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            if results.len() >= max {
                break;
            }
            let file_type = entry.file_type().await?;

            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
                continue;
            }

            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() || file_type.is_file() {
                Box::pin(search_path(&entry.path(), regex, include, results, max)).await?;
            }
        }
    }

    Ok(())
}
