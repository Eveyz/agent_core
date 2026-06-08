//! SSE transport for MCP — connects to remote MCP servers over HTTP/SSE.
//!
//! MCP SSE transport uses two HTTP connections:
//! 1. GET  /sse → SSE stream (server → client responses and notifications)
//! 2. POST /message → JSON-RPC requests (client → server)
//!
//! The endpoint URL is discovered from the first SSE event.

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::sync::Mutex;

use super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// SSE transport for remote MCP servers.
pub struct SseTransport {
    base_url: String,
    post_url: Mutex<Option<String>>,
    next_id: Mutex<u64>,
    /// Shared HTTP client for connection reuse.
    client: reqwest::Client,
}

impl SseTransport {
    /// Create an SSE transport targeting the given base URL.
    /// The base URL should be like `http://localhost:8000`.
    /// The `/sse` endpoint will be used for the SSE stream.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            post_url: Mutex::new(None),
            next_id: Mutex::new(1),
            client: reqwest::Client::new(),
        }
    }

    /// Initialize: connect to the SSE endpoint, discover the POST URL.
    pub async fn connect(&self) -> Result<()> {
        let sse_url = format!("{}/sse", self.base_url);

        let response = self
            .client
            .get(&sse_url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .with_context(|| format!("Failed to connect to SSE endpoint: {}", sse_url))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "SSE endpoint returned {}: {}",
                response.status(),
                sse_url
            );
        }

        // Read the SSE stream to find the endpoint event
        let mut stream = response.bytes_stream();
        use futures::StreamExt;

        let mut buffer = String::new();
        let endpoint_url: String = loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    // Parse SSE events from buffer
                    let mut found = None;
                    while let Some(event_end) = buffer.find("\n\n") {
                        let event_text = buffer[..event_end].to_string();
                        buffer = buffer[event_end + 2..].to_string();

                        if let Some(url) = parse_sse_endpoint(&event_text) {
                            found = Some(url);
                            break;
                        }
                    }
                    if let Some(url) = found {
                        break url;
                    }
                }
                Some(Err(e)) => {
                    anyhow::bail!("SSE stream error: {}", e);
                }
                None => {
                    anyhow::bail!("SSE stream ended without endpoint event");
                }
            }
            // Safety: don't loop forever
            if buffer.len() > 100_000 {
                anyhow::bail!("SSE endpoint event not found in first 100KB of stream");
            }
        };

        let post_url = endpoint_url;

        // Resolve relative URLs
        let full_post_url = if post_url.starts_with("http") {
            post_url
        } else if post_url.starts_with('/') {
            // Extract origin from base_url
            if let Some(pos) = self.base_url.find("://") {
                let after_proto = &self.base_url[pos + 3..];
                if let Some(slash) = after_proto.find('/') {
                    format!("{}{}", &self.base_url[..pos + 3 + slash], post_url)
                } else {
                    format!("{}{}", self.base_url, post_url)
                }
            } else {
                format!("{}{}", self.base_url, post_url)
            }
        } else {
            format!("{}/{}", self.base_url, post_url)
        };

        *self.post_url.lock().await = Some(full_post_url.clone());
        Ok(())
    }

    /// Get the POST endpoint URL (must call `connect()` first).
    async fn get_post_url(&self) -> Result<String> {
        self.post_url
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("SSE transport not connected. Call connect() first."))
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Send a JSON-RPC request via HTTP POST and wait for the response via SSE.
    ///
    /// NOTE: In a full implementation, the SSE stream would be kept open and
    /// responses matched by ID. For simplicity, we send the POST and expect a
    /// synchronous JSON-RPC response in the HTTP response body.
    ///
    /// Many MCP SSE servers support this hybrid mode where POST requests
    /// return the response directly instead of via the SSE stream.
    pub async fn request(&self, method: &str, params: Value) -> Result<JsonRpcResponse> {
        let id = self.next_id().await;
        let req = JsonRpcRequest::new(id, method, params);
        let post_url = self.get_post_url().await?;

        let http_response = self
            .client
            .post(&post_url)
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .with_context(|| format!("Failed to POST to {}", post_url))?;

        if !http_response.status().is_success() {
            let status = http_response.status();
            let body = http_response.text().await.unwrap_or_default();
            anyhow::bail!(
                "MCP POST returned {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            );
        }

        let response: JsonRpcResponse = http_response
            .json()
            .await
            .context("Failed to parse JSON-RPC response from POST")?;

        Ok(response)
    }

    /// Send a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let req = JsonRpcRequest::notification(method, params);
        let post_url = self.get_post_url().await?;

        let _ = self
            .client
            .post(&post_url)
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await?;

        Ok(())
    }

    /// Check if this transport is connected (has discovered the POST URL).
    pub async fn is_connected(&self) -> bool {
        self.post_url.lock().await.is_some()
    }

    /// No process to kill — SSE is stateless.
    pub async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Parse an SSE event to extract the `endpoint` field.
/// MCP SSE servers send an event:endpoint line followed by data:/message?....
fn parse_sse_endpoint(event_text: &str) -> Option<String> {
    let mut event_type = String::new();
    let mut data = String::new();

    for line in event_text.lines() {
        if let Some(value) = line.strip_prefix("event: ") {
            event_type = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("event:") {
            event_type = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data: ") {
            data = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data = value.trim().to_string();
        }
    }

    if event_type == "endpoint" && !data.is_empty() {
        Some(data)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_endpoint() {
        let event = "event: endpoint\ndata: /message?sessionId=abc123\n";
        let result = parse_sse_endpoint(event);
        assert_eq!(result, Some("/message?sessionId=abc123".to_string()));
    }

    #[test]
    fn test_parse_sse_endpoint_with_spaces() {
        let event = "event: endpoint\ndata: /mcp/message\n\n";
        let result = parse_sse_endpoint(event);
        assert_eq!(result, Some("/mcp/message".to_string()));
    }

    #[test]
    fn test_parse_sse_not_endpoint_event() {
        let event = "event: progress\ndata: 50\n";
        let result = parse_sse_endpoint(event);
        assert_eq!(result, None);
    }
}
