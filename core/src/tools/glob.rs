use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::Path;

use super::Tool;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Search file names and paths in the workspace matching a glob pattern (e.g. '**/*.rs', '**/SKILL.md')."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to search for (e.g. '**/SKILL.md' or '**/*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "The base directory to start the glob search from (default: current directory)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern = args["pattern"].as_str().context("missing 'pattern'")?.to_string();
        let base_path = args["path"].as_str().unwrap_or(".").to_string();
        let working_dir = args.get("_working_dir").and_then(|v| v.as_str()).map(str::to_string);
        let max_results = args["max_results"].as_u64().unwrap_or(100) as usize;

        // glob::glob is synchronous and blocking — run it on a blocking thread.
        let results = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let base_path = crate::paths::resolve_tool_path(&base_path, None, working_dir.as_deref());
            let base = Path::new(&base_path);

            let full_pattern_str = if Path::new(&pattern).is_absolute() {
                pattern
            } else {
                base.join(&pattern).to_string_lossy().to_string()
            };

            let mut results: Vec<String> = Vec::new();

            let paths = match glob::glob(&full_pattern_str) {
                Ok(paths) => paths,
                Err(e) => anyhow::bail!("invalid glob pattern: {e}"),
            };

            for entry in paths {
                if results.len() >= max_results {
                    break;
                }
                match entry {
                    Ok(path) => {
                        let display_path = if let Ok(rel) = path.strip_prefix(&base) {
                            rel.display().to_string()
                        } else if let Ok(rel) = path.strip_prefix(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))) {
                            rel.display().to_string()
                        } else {
                            path.display().to_string()
                        };
                        results.push(display_path);
                    }
                    Err(_) => {
                        continue;
                    }
                }
            }

            Ok(results)
        })
        .await??;

        if results.is_empty() {
            return Ok("No matching files found.".to_string());
        }

        Ok(results.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_files() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join("glob_tool_tests");
        std::fs::create_dir_all(&dir).unwrap();
        
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();

        let file1 = dir.join("test_file_a.txt");
        let file2 = dir.join("test_file_b.log");
        let file3 = sub.join("nested_file.txt");

        std::fs::write(&file1, "content").unwrap();
        std::fs::write(&file2, "content").unwrap();
        std::fs::write(&file3, "content").unwrap();

        (dir, file1, file3)
    }

    #[tokio::test]
    async fn test_glob_finds_matching_files() {
        let (dir, _, _) = create_test_files();
        let out = GlobTool
            .execute(json!({
                "pattern": "**/*.txt",
                "path": dir.to_string_lossy()
            }))
            .await
            .unwrap();

        assert!(out.contains("test_file_a.txt"));
        assert!(out.contains("nested/nested_file.txt"));
        assert!(!out.contains("test_file_b.log"));
    }

    #[tokio::test]
    async fn test_glob_returns_no_matching_files() {
        let (dir, _, _) = create_test_files();
        let out = GlobTool
            .execute(json!({
                "pattern": "**/*.nonexistent",
                "path": dir.to_string_lossy()
            }))
            .await
            .unwrap();

        assert_eq!(out, "No matching files found.");
    }
}
