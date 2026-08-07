//! Stdio transport for MCP — spawns a process and communicates via line-delimited JSON.
//!
//! MCP over stdio sends each JSON-RPC message as a single line terminated by `\n`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Wraps a child process with line-delimited JSON-RPC communication.
pub struct StdioTransport {
    child: Mutex<Option<Child>>,
    reader: Mutex<Option<BufReader<tokio::process::ChildStdout>>>,
    writer: Mutex<Option<tokio::process::ChildStdin>>,
    next_id: Mutex<u64>,
}

impl StdioTransport {
    /// Spawn the process and set up stdio pipes.
    /// `env` is merged into the child process environment (stdio auth tokens, etc.).
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {} {:?}", command, args))?;

        let stdout = child.stdout.take().context("Failed to capture stdout")?;
        let stdin = child.stdin.take().context("Failed to capture stdin")?;

        let reader = BufReader::new(stdout);

        Ok(Self {
            child: Mutex::new(Some(child)),
            reader: Mutex::new(Some(reader)),
            writer: Mutex::new(Some(stdin)),
            next_id: Mutex::new(1),
        })
    }

    /// Generate the next request ID.
    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Send a JSON-RPC request and wait for the matching response.
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id().await;
        let req = JsonRpcRequest::new(id, method, params);
        let msg = serde_json::to_string(&req)? + "\n";

        // Write to stdin
        {
            let mut writer = self.writer.lock().await;
            if let Some(ref mut w) = *writer {
                w.write_all(msg.as_bytes()).await?;
                w.flush().await?;
            } else {
                anyhow::bail!("MCP transport writer is closed");
            }
        }

        // Read responses until we find one with matching id
        let mut attempts = 0;
        let max_attempts = 100; // safety limit
        while attempts < max_attempts {
            let line = {
                let mut reader = self.reader.lock().await;
                if let Some(ref mut r) = *reader {
                    let mut buf = String::new();
                    r.read_line(&mut buf).await?;
                    if buf.is_empty() {
                        anyhow::bail!("MCP server closed stdout unexpectedly");
                    }
                    buf
                } else {
                    anyhow::bail!("MCP transport reader is closed");
                }
            };

            let response: JsonRpcResponse = serde_json::from_str(line.trim())
                .with_context(|| format!("Failed to parse JSON-RPC response: {}", line.trim()))?;

            // Notifications (id=None) are skipped
            // Match by id
            if response.id == Some(id) {
                return Ok(response);
            }

            // If there's an error with our id, return it
            if response.id == Some(id) {
                return Ok(response);
            }

            attempts += 1;
        }

        anyhow::bail!(
            "No response received for request id {} after {} attempts",
            id,
            max_attempts
        )
    }

    /// Send a notification (no response expected).
    pub async fn notify(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let req = JsonRpcRequest::notification(method, params);
        let msg = serde_json::to_string(&req)? + "\n";

        let mut writer = self.writer.lock().await;
        if let Some(ref mut w) = *writer {
            w.write_all(msg.as_bytes()).await?;
            w.flush().await?;
        } else {
            anyhow::bail!("MCP transport writer is closed");
        }

        Ok(())
    }

    /// Check if the child process is still alive.
    pub async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        if let Some(ref mut c) = *child {
            match c.try_wait() {
                Ok(Some(_)) => false, // exited
                Ok(None) => true,     // still running
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Shut down the transport: kill the child process.
    pub async fn shutdown(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        if let Some(mut c) = child.take() {
            let _ = c.kill().await;
        }
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Child is killed on drop via kill_on_drop
    }
}
