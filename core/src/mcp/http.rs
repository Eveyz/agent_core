//! Streamable HTTP transport for MCP (spec 2025-03-26).
//!
//! Unlike the legacy SSE transport, Streamable HTTP uses a single endpoint.
//! Each JSON-RPC request is sent via `POST` with
//! `Accept: application/json, text/event-stream`. The server may answer with a
//! plain JSON body *or* an SSE stream carrying the response; both are handled.
//! The `Mcp-Session-Id` header (if returned) is captured and reused.

use anyhow::{Context, Result};
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use tokio::sync::Mutex;

use super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Streamable HTTP transport for remote MCP servers.
pub struct StreamableHttpTransport {
    url: String,
    client: reqwest::Client,
    next_id: Mutex<u64>,
    session_id: Mutex<Option<String>>,
    connected: Mutex<bool>,
}

impl StreamableHttpTransport {
    /// Create a Streamable HTTP transport targeting the given endpoint URL.
    pub fn new(url: &str) -> Self {
        Self {
            url: url.trim().to_string(),
            client: reqwest::Client::new(),
            next_id: Mutex::new(1),
            session_id: Mutex::new(None),
            connected: Mutex::new(false),
        }
    }

    /// Mark connected. Streamable HTTP has no separate handshake endpoint —
    /// the `initialize` call itself opens (and may return) the session.
    pub async fn connect(&self) -> Result<()> {
        if self.url.is_empty() {
            anyhow::bail!("Streamable HTTP transport requires a URL");
        }
        *self.connected.lock().await = true;
        Ok(())
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Capture `Mcp-Session-Id` from a response so subsequent calls reuse it.
    async fn capture_session(&self, resp: &reqwest::Response) {
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            if let Ok(s) = sid.to_str() {
                *self.session_id.lock().await = Some(s.to_string());
            }
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<JsonRpcResponse> {
        let id = self.next_id().await;
        let req = JsonRpcRequest::new(id, method, params);

        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&req);

        if let Some(sid) = self.session_id.lock().await.clone() {
            builder = builder.header("Mcp-Session-Id", sid);
        }

        let resp = builder
            .send()
            .await
            .with_context(|| format!("Failed to POST to MCP endpoint: {}", self.url))?;

        self.capture_session(&resp).await;

        let status = resp.status();
        if !status.is_success() && status != reqwest::StatusCode::ACCEPTED {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "MCP HTTP returned {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            );
        }

        let ct = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if ct.contains("text/event-stream") {
            // Read the SSE stream until we locate the response matching our id.
            use futures::StreamExt;
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(ev_end) = buffer.find("\n\n") {
                    let ev = buffer[..ev_end].to_string();
                    buffer = buffer[ev_end + 2..].to_string();
                    if let Some(data) = parse_sse_data(&ev) {
                        if let Ok(jr) = serde_json::from_str::<JsonRpcResponse>(&data) {
                            if jr.id == Some(id) {
                                return Ok(jr);
                            }
                        } else if let Ok(batch) =
                            serde_json::from_str::<Vec<JsonRpcResponse>>(&data)
                        {
                            if let Some(jr) = batch.into_iter().find(|r| r.id == Some(id)) {
                                return Ok(jr);
                            }
                        }
                    }
                }
            }
            anyhow::bail!("MCP HTTP stream ended without response for id {}", id);
        } else {
            let jr: JsonRpcResponse = resp
                .json()
                .await
                .context("Failed to parse JSON-RPC response from MCP HTTP endpoint")?;
            Ok(jr)
        }
    }

    /// Send a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let req = JsonRpcRequest::notification(method, params);

        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&req);

        if let Some(sid) = self.session_id.lock().await.clone() {
            builder = builder.header("Mcp-Session-Id", sid);
        }

        let _ = builder.send().await?;
        Ok(())
    }

    /// Check if the transport is connected.
    pub async fn is_connected(&self) -> bool {
        *self.connected.lock().await
    }

    /// No process to kill — HTTP is stateless.
    pub async fn shutdown(&self) -> Result<()> {
        *self.connected.lock().await = false;
        Ok(())
    }
}

/// Extract the `data:` payload from an SSE event block (may span lines).
fn parse_sse_data(event_text: &str) -> Option<String> {
    let mut data = String::new();
    for line in event_text.lines() {
        if let Some(value) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim());
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_data_single() {
        let ev = "event: message\ndata: {\"id\":1,\"result\":{}}\n";
        assert_eq!(
            parse_sse_data(ev),
            Some("{\"id\":1,\"result\":{}}".to_string())
        );
    }

    #[test]
    fn test_parse_sse_data_multi_line() {
        let ev = "data: line1\ndata: line2\n";
        assert_eq!(parse_sse_data(ev), Some("line1\nline2".to_string()));
    }

    #[test]
    fn test_parse_sse_data_empty() {
        assert_eq!(parse_sse_data("event: ping\n"), None);
    }
}
