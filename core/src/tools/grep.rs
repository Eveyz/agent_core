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
        let working_dir = args.get("_working_dir").and_then(|v| v.as_str());
        let include = args["include"].as_str();
        let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;

        let regex = regex::Regex::new(pattern)
            .with_context(|| format!("invalid regex pattern: {pattern}"))?;

        let mut results: Vec<String> = Vec::new();
        let search_root = crate::paths::resolve_tool_path(path, None, None, working_dir);
        search_path(
            &search_root,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn setup_fixture() -> PathBuf {
        // Use a per-test unique subdir with a process-local counter to avoid
        // races between parallel tests sharing /tmp.
        static COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "grep_tool_tests_{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("src");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(sub.join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(sub.join("b.rs"), "const GAMMA: u32 = 1;\n").unwrap();
        std::fs::write(dir.join("notes.md"), "# Alpha notes\nbeta mentions\n").unwrap();

        // hidden dir + file should be skipped
        let hid = dir.join(".hidden");
        std::fs::create_dir_all(&hid).unwrap();
        std::fs::write(hid.join("secret.rs"), "ALPHA\n").unwrap();

        dir
    }

    #[tokio::test]
    async fn test_grep_matches_pattern_in_files() {
        let dir = setup_fixture();
        let out = GrepTool
            .execute(json!({
                "pattern": "alpha",
                "path": dir.to_string_lossy(),
                "include": "*.rs"
            }))
            .await
            .unwrap();
        assert!(out.contains("a.rs"), "out = {out}");
        assert!(out.contains("fn alpha"));
        assert!(!out.contains("notes.md"));
    }

    #[tokio::test]
    async fn test_grep_no_matches_returns_message() {
        let dir = setup_fixture();
        let out = GrepTool
            .execute(json!({
                "pattern": "definitely_not_present_xyz123",
                "path": dir.to_string_lossy()
            }))
            .await
            .unwrap();
        assert_eq!(out, "No matches found.");
    }

    #[tokio::test]
    async fn test_grep_skips_hidden_directories() {
        let dir = setup_fixture();
        let out = GrepTool
            .execute(json!({
                "pattern": "ALPHA",
                "path": dir.to_string_lossy()
            }))
            .await
            .unwrap();
        assert!(
            !out.contains("secret.rs"),
            "files in hidden dirs should be skipped: {out}"
        );
    }

    #[tokio::test]
    async fn test_grep_invalid_pattern_errors() {
        let dir = setup_fixture();
        let res = GrepTool
            .execute(json!({
                "pattern": "(",  // invalid regex
                "path": dir.to_string_lossy()
            }))
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_grep_invalid_include_pattern_errors_not_silently_matches_all() {
        let dir = setup_fixture();
        let res = GrepTool
            .execute(json!({
                "pattern": "alpha",
                "path": dir.to_string_lossy(),
                "include": "[bad"
            }))
            .await;
        assert!(
            res.is_err(),
            "invalid include pattern should error, not silently match all"
        );
    }

    #[tokio::test]
    async fn test_grep_max_results_limits() {
        let dir = setup_fixture();
        let out = GrepTool
            .execute(json!({
                "pattern": "fn|const",
                "path": dir.to_string_lossy(),
                "include": "*.rs",
                "max_results": 1
            }))
            .await
            .unwrap();
        assert_eq!(out.lines().count(), 1);
    }
}
