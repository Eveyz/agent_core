use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
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
        let command = args["command"]
            .as_str()
            .context("missing 'command'")?
            .to_string();
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
            return self
                .run_bash_supervised(sup, command, working_dir, on_update)
                .await;
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
            let mut supervisor = sup.lock();
            supervisor.spawn_bash(command, working_dir)?
        };

        let stdout_handle = {
            let mut supervisor = sup.lock();
            let child = supervisor
                .get_child(&child_id)
                .ok_or_else(|| anyhow::anyhow!("child disappeared after spawn"))?;
            child.take_stdout()
        };

        let stderr_handle = {
            let mut supervisor = sup.lock();
            let child = supervisor
                .get_child(&child_id)
                .ok_or_else(|| anyhow::anyhow!("child disappeared after spawn"))?;
            child.take_stderr()
        };

        let stdout_fut = async {
            let mut result = String::new();
            if let Some(mut stdout) = stdout_handle {
                if let Some(on_update) = on_update.as_ref() {
                    let reader = tokio::io::BufReader::new(stdout);
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = reader.lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                on_update(&line);
                                result.push_str(&line);
                                result.push('\n');
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "stdout read error in supervised bash");
                                break;
                            }
                        }
                    }
                } else {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    if let Err(e) = stdout.read_to_end(&mut buf).await {
                        tracing::warn!(error = %e, "stdout read error in supervised bash");
                    }
                    result = String::from_utf8_lossy(&buf).to_string();
                }
            }
            result
        };

        let stderr_fut = async {
            let mut result = String::new();
            if let Some(mut stderr) = stderr_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                if let Err(e) = stderr.read_to_end(&mut buf).await {
                    tracing::warn!(error = %e, "stderr read error in supervised bash");
                }
                result = String::from_utf8_lossy(&buf).to_string();
            }
            result
        };

        let sup_clone = sup.clone();
        let child_id_clone = child_id.clone();
        let wait_fut = async move {
            loop {
                let maybe_code = {
                    let mut supervisor = sup_clone.lock();
                    let child = supervisor.get_child(&child_id_clone);
                    if let Some(c) = child {
                        c.try_exit_code()
                    } else {
                        Some(-1)
                    }
                };
                if let Some(code) = maybe_code {
                    return code;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };

        let (stdout_str, stderr_str, exit_code) = tokio::join!(stdout_fut, stderr_fut, wait_fut);

        // Remove the child from the supervisor (it's done)
        {
            let mut supervisor = sup.lock();
            if let Err(e) = supervisor.kill(&child_id) {
                tracing::warn!(error = %e, "failed to kill child process during cleanup");
            }
        }

        let mut result = stdout_str;
        if !stderr_str.is_empty() {
            result.push_str("\n--- stderr ---\n");
            result.push_str(&stderr_str);
        }
        if exit_code != 0 {
            result.push_str(&format!("\n[exit code: {exit_code}]"));
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

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_fut = async {
            let mut result = String::new();
            if let Some(mut stdout) = stdout_handle {
                if let Some(on_update) = on_update.as_ref() {
                    let reader = tokio::io::BufReader::new(stdout);
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = reader.lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                on_update(&line);
                                result.push_str(&line);
                                result.push('\n');
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "stdout read error in legacy bash");
                                break;
                            }
                        }
                    }
                } else {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    if let Err(e) = stdout.read_to_end(&mut buf).await {
                        tracing::warn!(error = %e, "stdout read error in legacy bash");
                    }
                    result = String::from_utf8_lossy(&buf).to_string();
                }
            }
            result
        };

        let stderr_fut = async {
            let mut result = String::new();
            if let Some(mut stderr) = stderr_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                if let Err(e) = stderr.read_to_end(&mut buf).await {
                    tracing::warn!(error = %e, "stderr read error in legacy bash");
                }
                result = String::from_utf8_lossy(&buf).to_string();
            }
            result
        };

        let wait_fut = async {
            child
                .wait()
                .await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1)
        };

        let (stdout_str, stderr_str, exit_code) = tokio::join!(stdout_fut, stderr_fut, wait_fut);

        let mut result = stdout_str;
        if !stderr_str.is_empty() {
            result.push_str("\n--- stderr ---\n");
            result.push_str(&stderr_str);
        }
        if exit_code != 0 {
            result.push_str(&format!("\n[exit code: {exit_code}]"));
        }

        Ok(result)
    }
}
