use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncBufReadExt;

use super::{Tool, ToolUpdateFn};
use crate::runtime::ProcessSupervisor;
use crate::types::EventSender;

/// Bash tool with optional process supervision.
///
/// When `supervisor` is `Some`, child processes are spawned via the
/// [`ProcessSupervisor`], which places them in their own process group.
/// This ensures that `kill_all()` can terminate the entire process tree
/// (including piped commands) on cancel — no orphan processes.
///
/// When `supervisor` is `None` (legacy path), falls back to direct
/// `tokio::process::Command` with `kill_on_drop(true)`.
pub struct BashTool {
    supervisor: Option<Arc<Mutex<ProcessSupervisor>>>,
    /// Default working directory (from Run's working_dir).
    default_working_dir: Option<String>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BashTool {
    pub fn new() -> Self {
        Self {
            supervisor: None,
            default_working_dir: None,
        }
    }

    /// Create a BashTool backed by a ProcessSupervisor for process-group
    /// kill semantics. Used by the runtime Run path.
    pub fn with_supervisor(
        supervisor: Arc<Mutex<ProcessSupervisor>>,
        default_working_dir: Option<String>,
    ) -> Self {
        Self {
            supervisor: Some(supervisor),
            default_working_dir,
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash shell command and return stdout/stderr. Use with caution. Timeout: 60 seconds."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
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
        let working_dir = args["working_dir"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| self.default_working_dir.clone())
            .unwrap_or_else(|| ".".to_string());
        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60);

        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.run_bash(&command, &working_dir, on_update),
        )
        .await
        .context("command timed out")?
    }
}

impl BashTool {
    async fn run_bash(
        &self,
        command: &str,
        working_dir: &str,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        // ── Supervised path (runtime Run) ──────────────────────────
        if let Some(ref sup) = self.supervisor {
            return self.run_bash_supervised(sup, command, working_dir, on_update).await;
        }

        // ── Legacy path (old Agent) ────────────────────────────────
        self.run_bash_legacy(command, working_dir, on_update).await
    }

    /// Supervised execution: spawn via ProcessSupervisor (process group),
    /// stream stdout, kill on cancel.
    async fn run_bash_supervised(
        &self,
        sup: &Arc<Mutex<ProcessSupervisor>>,
        command: &str,
        working_dir: &str,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        let child_id = {
            let mut supervisor = sup.lock().unwrap();
            supervisor.spawn_bash(command, working_dir)?
        };

        // Take stdout for streaming. We need to lock the supervisor to access
        // the child, but we must release the lock before awaiting.
        let stdout = {
            let mut supervisor = sup.lock().unwrap();
            let child = supervisor
                .get_child(&child_id)
                .ok_or_else(|| anyhow::anyhow!("child disappeared after spawn"))?;
            child.take_stdout()
        };

        let mut result = String::new();

        // Stream stdout lines if on_update callback is provided
        if let (Some(on_update), Some(stdout)) = (on_update.as_ref(), stdout) {
            let reader = tokio::io::BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                on_update(&line);
                result.push_str(&line);
                result.push('\n');
            }
        }

        // Wait for the process to finish — take stdout/stderr handles out
        // of the supervisor lock scope so we don't hold the MutexGuard
        // across .await (which would make the future !Send).
        let (stdout_remaining, stderr_handle) = {
            let mut supervisor = sup.lock().unwrap();
            let child = supervisor
                .get_child(&child_id)
                .ok_or_else(|| anyhow::anyhow!("child disappeared during wait"))?;
            (child.take_stdout(), child.take_stderr())
        };

        // If we didn't stream, collect stdout now (outside the lock)
        if on_update.is_none() {
            if let Some(stdout) = stdout_remaining {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                let mut reader = stdout;
                let _ = reader.read_to_end(&mut buf).await;
                result.push_str(&String::from_utf8_lossy(&buf));
            }
        }

        // Wait for the process to exit — poll with try_wait to avoid
        // holding the MutexGuard across .await (which would make the
        // future !Send). try_wait is non-async, so it's safe inside the lock.
        let exit_code = loop {
            let maybe_code = {
                let mut supervisor = sup.lock().unwrap();
                let child = supervisor
                    .get_child(&child_id)
                    .ok_or_else(|| anyhow::anyhow!("child disappeared before wait"))?;
                child.try_exit_code()
            };
            if let Some(code) = maybe_code {
                break code;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        let stderr_output = stderr_handle;

        // Collect stderr
        let stderr = if let Some(stderr) = stderr_output {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let mut reader = stderr;
            let _ = reader.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        } else {
            String::new()
        };

        let status_code = exit_code;

        // Remove the child from the supervisor (it's done)
        {
            let mut supervisor = sup.lock().unwrap();
            let _ = supervisor.kill(&child_id);
        }

        // Build the result string
        if !stderr.is_empty() {
            result.push_str("\n--- stderr ---\n");
            result.push_str(&stderr);
        }
        if status_code != 0 {
            result.push_str(&format!("\n[exit code: {status_code}]"));
        }

        Ok(result)
    }

    /// Legacy execution: direct tokio::process::Command with kill_on_drop.
    async fn run_bash_legacy(
        &self,
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

        // If we didn't stream, collect stdout normally
        if on_update.is_none() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            result.push_str(&stdout);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let status = output.status.code().unwrap_or(-1);

        if !stderr.is_empty() {
            result.push_str("\n--- stderr ---\n");
            result.push_str(&stderr);
        }
        if status != 0 {
            result.push_str(&format!("\n[exit code: {status}]"));
        }

        Ok(result)
    }
}
