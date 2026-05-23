use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Command;

use super::Tool;

pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the working tree status (git status --short)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (default: current directory)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or(".");

        let output = Command::new("git")
            .args(["status", "--short", "--branch"])
            .current_dir(path)
            .output()
            .context("failed to run git status")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.is_empty() {
            return Ok("Working tree clean.".to_string());
        }

        Ok(stdout.to_string())
    }
}

pub struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show changes in the working tree or between commits (git diff)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (default: current directory)"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (git diff --staged). Default: false"
                },
                "file": {
                    "type": "string",
                    "description": "Show diff for a specific file"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or(".");
        let staged = args["staged"].as_bool().unwrap_or(false);
        let file = args["file"].as_str();

        let mut cmd = Command::new("git");
        cmd.arg("diff");

        if staged {
            cmd.arg("--staged");
        }

        if let Some(f) = file {
            cmd.arg(f);
        }

        let output = cmd
            .current_dir(path)
            .output()
            .context("failed to run git diff")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.is_empty() {
            return Ok("No changes.".to_string());
        }

        let result = stdout.to_string();
        if result.len() > 8000 {
            return Ok(format!("{}... (truncated)", &result[..8000]));
        }

        Ok(result)
    }
}

pub struct GitLogTool;

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show recent commit history (git log --oneline)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Repository path (default: current directory)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 20)"
                },
                "file": {
                    "type": "string",
                    "description": "Show log for a specific file"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or(".");
        let count = args["count"].as_u64().unwrap_or(20).to_string();
        let file = args["file"].as_str();

        let mut cmd = Command::new("git");
        cmd.args(["log", "--oneline", "-n", &count]);

        if let Some(f) = file {
            cmd.arg(f);
        }

        let output = cmd
            .current_dir(path)
            .output()
            .context("failed to run git log")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.is_empty() {
            return Ok("No commits found.".to_string());
        }

        Ok(stdout.to_string())
    }
}

pub struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage all changes and create a commit (git add -A && git commit -m <message>)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The commit message"
                },
                "path": {
                    "type": "string",
                    "description": "Repository path (default: current directory)"
                },
                "add": {
                    "type": "boolean",
                    "description": "Whether to stage all changes first (default: true)"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let message = args["message"].as_str().context("missing 'message'")?;
        let path = args["path"].as_str().unwrap_or(".");
        let do_add = args["add"].as_bool().unwrap_or(true);

        if do_add {
            let add_output = Command::new("git")
                .args(["add", "-A"])
                .current_dir(path)
                .output()
                .context("failed to run git add")?;

            if !add_output.status.success() {
                let stderr = String::from_utf8_lossy(&add_output.stderr);
                return Ok(format!("git add failed: {stderr}"));
            }
        }

        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(path)
            .output()
            .context("failed to run git commit")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Ok(format!("commit failed: {stderr}"))
        }
    }
}

pub struct GitShowTool;

#[async_trait]
impl Tool for GitShowTool {
    fn name(&self) -> &str {
        "git_show"
    }

    fn description(&self) -> &str {
        "Show details of a specific commit (git show)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "commit": {
                    "type": "string",
                    "description": "Commit hash or reference (default: HEAD)"
                },
                "path": {
                    "type": "string",
                    "description": "Repository path (default: current directory)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let commit = args["commit"].as_str().unwrap_or("HEAD");
        let path = args["path"].as_str().unwrap_or(".");

        let output = Command::new("git")
            .args(["show", "--stat", commit])
            .current_dir(path)
            .output()
            .context("failed to run git show")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.is_empty() {
            return Ok("Commit not found.".to_string());
        }

        let result = stdout.to_string();
        if result.len() > 8000 {
            return Ok(format!("{}... (truncated)", &result[..8000]));
        }

        Ok(result)
    }
}
