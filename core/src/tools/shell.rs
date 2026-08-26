use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

use super::{Tool, ToolUpdateFn};
use crate::runtime::ProcessSupervisor;
use crate::runtime::platform_shell::{self, shell_command};
use crate::types::EventSender;

/// Shell tool with optional process supervision.
///
/// When `supervisor` is `Some`, child processes are spawned via the
/// [`ProcessSupervisor`], which places them in their own process group.
/// This ensures that `kill_all()` can terminate the entire process tree
/// (including piped commands) on cancel — no orphan processes.
///
/// When `supervisor` is `None` (legacy path), falls back to direct
/// `tokio::process::Command` with `kill_on_drop(true)`.
///
/// On Windows the interpreter is PowerShell; on Unix it is `sh -c`.
pub struct ShellTool {
    supervisor: Option<Arc<Mutex<ProcessSupervisor>>>,
    /// Default working directory (from Run's working_dir).
    default_working_dir: Option<String>,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            supervisor: None,
            default_working_dir: None,
        }
    }

    /// Create a ShellTool backed by a ProcessSupervisor for process-group
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

    /// Create an unsupervised ShellTool with a default working directory.
    /// Used by subagents so their shell commands execute in the subagent's
    /// working directory without relying on the process-global CWD (which
    /// would race with concurrent subagents).
    pub fn with_default_working_dir(default_working_dir: Option<String>) -> Self {
        Self {
            supervisor: None,
            default_working_dir,
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        #[cfg(windows)]
        {
            "Execute a PowerShell command and return stdout/stderr. Use PowerShell syntax \
             (e.g. Get-ChildItem, Get-Content, Select-String). Timeout: 60 seconds."
        }
        #[cfg(not(windows))]
        {
            "Execute a shell command via sh -c and return stdout/stderr. Use with caution. \
             Timeout: 60 seconds."
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": format!(
                        "The {} command to execute",
                        platform_shell::shell_label()
                    )
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
            self.run_shell(&command, &working_dir, on_update),
        )
        .await
        .context("command timed out")?
    }
}

impl ShellTool {
    async fn run_shell(
        &self,
        command: &str,
        working_dir: &str,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        // ── Supervised path (runtime Run) ──────────────────────────
        if let Some(ref sup) = self.supervisor {
            return self
                .run_shell_supervised(sup, command, working_dir, on_update)
                .await;
        }

        // ── Legacy path (old Agent) ────────────────────────────────
        self.run_shell_legacy(command, working_dir, on_update).await
    }

    /// Supervised execution: spawn via ProcessSupervisor (process group),
    /// stream stdout, kill on cancel.
    async fn run_shell_supervised(
        &self,
        sup: &Arc<Mutex<ProcessSupervisor>>,
        command: &str,
        working_dir: &str,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        let child_id = {
            let mut supervisor = sup.lock();
            supervisor.spawn_shell(command, working_dir)?
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
                                // AsyncBufReadExt::lines strips the trailing newline;
                                // re-add it so streaming UI matches the final result.
                                let mut chunk = line;
                                chunk.push('\n');
                                on_update(&chunk);
                                result.push_str(&chunk);
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "stdout read error in supervised shell");
                                break;
                            }
                        }
                    }
                } else {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    if let Err(e) = stdout.read_to_end(&mut buf).await {
                        tracing::warn!(error = %e, "stdout read error in supervised shell");
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
                    tracing::warn!(error = %e, "stderr read error in supervised shell");
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
    async fn run_shell_legacy(
        &self,
        command: &str,
        working_dir: &str,
        on_update: Option<ToolUpdateFn>,
    ) -> Result<String> {
        let mut child = shell_command(command)
            .current_dir(working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!("failed to spawn {} command", platform_shell::shell_label())
            })?;

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
                                // AsyncBufReadExt::lines strips the trailing newline;
                                // re-add it so streaming UI matches the final result.
                                let mut chunk = line;
                                chunk.push('\n');
                                on_update(&chunk);
                                result.push_str(&chunk);
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(error = %e, "stdout read error in legacy shell");
                                break;
                            }
                        }
                    }
                } else {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    if let Err(e) = stdout.read_to_end(&mut buf).await {
                        tracing::warn!(error = %e, "stdout read error in legacy shell");
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
                    tracing::warn!(error = %e, "stderr read error in legacy shell");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> ShellTool {
        ShellTool::new()
    }

    #[tokio::test]
    async fn test_shell_echo_hello() {
        let out = tool()
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(out.contains("hello"));
        assert!(!out.contains("exit code"));
    }

    #[tokio::test]
    async fn test_shell_nonzero_exit_code_is_propagated() {
        let out = tool().execute(json!({"command": "exit 42"})).await.unwrap();
        assert!(
            out.contains("42"),
            "exit code 42 should be in output: {out}"
        );
    }

    #[tokio::test]
    async fn test_shell_captures_stderr() {
        #[cfg(windows)]
        let command = "[Console]::Error.WriteLine('err')";
        #[cfg(not(windows))]
        let command = "echo err >&2; true";

        let out = tool().execute(json!({"command": command})).await.unwrap();
        assert!(out.contains("err"), "stderr should be captured: {out}");
    }

    #[tokio::test]
    async fn test_shell_timeout_returns_error() {
        #[cfg(windows)]
        let command = "Start-Sleep -Seconds 10";
        #[cfg(not(windows))]
        let command = "sleep 10";

        let res = tool()
            .execute(json!({
                "command": command,
                "timeout_secs": 1
            }))
            .await;
        assert!(res.is_err(), "sleep with 1s timeout should time out");
    }

    #[tokio::test]
    async fn test_shell_working_dir_override() {
        #[cfg(windows)]
        {
            let tmp = std::env::temp_dir();
            let tmp_str = tmp.to_string_lossy().to_string();
            let out = tool()
                .execute(json!({
                    "command": "(Get-Location).Path",
                    "working_dir": tmp_str
                }))
                .await
                .unwrap();
            // PowerShell may normalize path casing / separators
            let normalized_out = out.replace('/', "\\").to_lowercase();
            let normalized_tmp = tmp_str.replace('/', "\\").to_lowercase();
            assert!(
                normalized_out.contains(&normalized_tmp)
                    || normalized_out.contains(&normalized_tmp.trim_end_matches('\\').to_string()),
                "pwd should report temp dir: out={out}, tmp={tmp_str}"
            );
        }
        #[cfg(not(windows))]
        {
            if !std::path::Path::new("/tmp").exists() {
                return;
            }
            let out = tool()
                .execute(json!({
                    "command": "pwd",
                    "working_dir": "/tmp"
                }))
                .await
                .unwrap();
            assert!(out.contains("/tmp"), "pwd should report /tmp: {out}");
        }
    }

    #[tokio::test]
    async fn test_shell_default_working_dir_from_tool() {
        #[cfg(windows)]
        {
            let tmp = std::env::temp_dir();
            let tmp_str = tmp.to_string_lossy().to_string();
            let t = ShellTool::with_default_working_dir(Some(tmp_str.clone()));
            let out = t
                .execute(json!({"command": "(Get-Location).Path"}))
                .await
                .unwrap();
            let normalized_out = out.replace('/', "\\").to_lowercase();
            let normalized_tmp = tmp_str.replace('/', "\\").to_lowercase();
            assert!(
                normalized_out.contains(&normalized_tmp)
                    || normalized_out.contains(&normalized_tmp.trim_end_matches('\\').to_string()),
                "default working_dir should apply: out={out}, tmp={tmp_str}"
            );
        }
        #[cfg(not(windows))]
        {
            if !std::path::Path::new("/tmp").exists() {
                return;
            }
            let t = ShellTool::with_default_working_dir(Some("/tmp".to_string()));
            let out = t.execute(json!({"command": "pwd"})).await.unwrap();
            assert!(
                out.contains("/tmp"),
                "default working_dir should apply: {out}"
            );
        }
    }

    #[tokio::test]
    async fn test_shell_missing_command_errors() {
        let res = tool().execute(json!({})).await;
        assert!(res.is_err(), "missing 'command' should error");
    }

    #[tokio::test]
    async fn test_shell_multiline_output_preserved() {
        #[cfg(windows)]
        let command = "Write-Output \"a`nb`nc\"";
        #[cfg(not(windows))]
        let command = "printf 'a\\nb\\nc\\n'";

        let out = tool().execute(json!({"command": command})).await.unwrap();
        assert!(out.contains("a") && out.contains("b") && out.contains("c"));
    }

    #[tokio::test]
    async fn test_shell_stream_updates_include_newlines() {
        use std::sync::{Arc, Mutex};
        #[cfg(windows)]
        let command = "Write-Output \"a`nb`nc\"";
        #[cfg(not(windows))]
        let command = "printf 'a\\nb\\nc\\n'";

        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let chunks_cb = chunks.clone();
        let on_update: super::super::ToolUpdateFn = Arc::new(move |partial: &str| {
            chunks_cb.lock().unwrap().push(partial.to_string());
        });

        let out = tool()
            .execute_with_stream(json!({ "command": command }), Some(on_update), None)
            .await
            .unwrap();

        let streamed = chunks.lock().unwrap().join("");
        // Streaming concatenation should preserve line breaks the same way as final output.
        assert!(
            streamed.contains('\n'),
            "streamed chunks must include newlines, got: {streamed:?}"
        );
        assert!(
            !streamed.replace('\n', "").contains("ab") || streamed.contains("a\nb"),
            "lines must not be glued together: {streamed:?}"
        );
        assert!(out.contains("a\n") || out.contains("a\r\n") || out.lines().any(|l| l == "a"));
    }
}
