use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolUpdateFn};
use crate::types::EventSender;

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr. Use with caution. Timeout: 60 seconds."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command (default: current directory)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        on_update: Option<ToolUpdateFn>,
        _event_sender: Option<EventSender>,
    ) -> Result<String> {
        let command = args["command"].as_str().context("missing 'command'")?.to_string();
        let working_dir = args["working_dir"].as_str().unwrap_or(".").to_string();
        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60);

        // Wrap the entire execution in a timeout to prevent infinite loops
        // (e.g. streaming output from a never-ending process)
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            run_command_inner(&command, &working_dir, on_update),
        )
        .await
        .context("command timed out")?
    }
}

async fn run_command_inner(
    command: &str,
    working_dir: &str,
    on_update: Option<ToolUpdateFn>,
) -> Result<String> {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn command")?;

    let mut result = String::new();

    // Stream stdout lines if on_update callback is provided
    if let (Some(on_update), Some(stdout)) = (on_update.as_ref(), child.stdout.take()) {
        let reader = tokio::io::BufReader::new(stdout);
        use tokio::io::AsyncBufReadExt;
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            on_update(&line);
            result.push_str(&line);
            result.push('\n');
        }
    }

    // Wait for the process to finish
    let output = child
        .wait_with_output()
        .await
        .context("failed to wait for command")?;

    // If we didn't stream (no on_update), collect stdout normally
    if on_update.is_none() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        result.push_str(&stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = output.status.code().unwrap_or(-1);

    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push_str("\n--- stderr ---\n");
        }
        result.push_str(&stderr);
    }

    if result.is_empty() {
        result = format!("(exit code: {status}, no output)");
    } else {
        result.push_str(&format!("\n(exit code: {status})"));
    }

    if result.len() > 8000 {
        result.truncate(8000);
        result.push_str("\n... (output truncated)");
    }

    Ok(result)
}
