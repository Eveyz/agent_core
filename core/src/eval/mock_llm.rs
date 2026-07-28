//! Scripted OpenAI-compatible mock LLM (Chat Completions SSE).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Deserialize)]
pub struct MockScript {
    #[serde(default)]
    pub steps: Vec<MockStep>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MockStep {
    Text {
        text: String,
        #[serde(default)]
        cache_hit: u64,
        #[serde(default)]
        cache_miss: u64,
    },
    /// Delay the HTTP response so tests can deterministically interrupt an
    /// in-flight model request before any SSE event arrives.
    DelayedText {
        text: String,
        delay_ms: u64,
    },
    ToolCalls {
        tools: Vec<MockToolCall>,
        #[serde(default)]
        cache_hit: u64,
        #[serde(default)]
        cache_miss: u64,
    },
    /// HTTP error response (non-SSE).
    Error {
        #[serde(default = "default_status")]
        status: u16,
        #[serde(default)]
        body: String,
    },
    /// Empty successful stream (triggers empty-response handling).
    Empty,
}

fn default_status() -> u16 {
    500
}

#[derive(Debug, Clone, Deserialize)]
pub struct MockToolCall {
    pub name: String,
    /// JSON object string or raw JSON — serialized into function.arguments.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

impl MockScript {
    pub fn from_toml_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("read mock script {}", path.display()))?;
        Self::from_toml_str(&s)
    }
}

/// Handle for a running mock server.
pub struct MockServer {
    pub base_url: String,
    pub addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    call_index: Arc<AtomicUsize>,
}

impl MockServer {
    pub fn call_count(&self) -> usize {
        self.call_index.load(Ordering::SeqCst)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

/// Start a mock Chat Completions server bound to `127.0.0.1:0`.
///
/// Clients should use `base_url` as `{scheme}://{host}:{port}/v1`.
pub async fn start_mock_server(script: MockScript) -> Result<MockServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{addr}/v1");
    let call_index = Arc::new(AtomicUsize::new(0));
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let script = Arc::new(script);
    let idx = call_index.clone();

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept = listener.accept() => {
                    match accept {
                        Ok((mut socket, _)) => {
                            let script = script.clone();
                            let idx = idx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(&mut socket, &script, &idx).await {
                                    tracing::debug!(error = %e, "mock llm connection error");
                                }
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    Ok(MockServer {
        base_url,
        addr,
        shutdown_tx: Some(shutdown_tx),
        join: Some(join),
        call_index,
    })
}

async fn handle_connection(
    socket: &mut tokio::net::TcpStream,
    script: &MockScript,
    call_index: &AtomicUsize,
) -> Result<()> {
    let mut buf = vec![0u8; 65536];
    let n = socket.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    // Very small HTTP parser — enough for OpenAIClient.
    if !req.contains("POST") || !req.contains("/chat/completions") {
        let body = "not found";
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    let step_i = call_index.fetch_add(1, Ordering::SeqCst);
    let step = if let Some(s) = script.steps.get(step_i).cloned() {
        s
    } else if let Some(MockStep::Error { status, body }) = script.steps.last() {
        // Keep failing on retries instead of inventing a success text.
        MockStep::Error {
            status: *status,
            body: body.clone(),
        }
    } else {
        MockStep::Text {
            text: format!("(mock) no script step for call {step_i}"),
            cache_hit: 0,
            cache_miss: 0,
        }
    };

    match step {
        MockStep::Error { status, body } => {
            let resp = format!(
                "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(resp.as_bytes()).await?;
        }
        MockStep::DelayedText { text, delay_ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            let sse = render_sse(&MockStep::Text {
                text,
                cache_hit: 0,
                cache_miss: 0,
            });
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
                sse.len()
            );
            socket.write_all(resp.as_bytes()).await?;
        }
        other => {
            let sse = render_sse(&other);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
                sse.len()
            );
            socket.write_all(resp.as_bytes()).await?;
        }
    }
    Ok(())
}

fn render_sse(step: &MockStep) -> String {
    let mut out = String::new();
    match step {
        MockStep::Text {
            text,
            cache_hit,
            cache_miss,
        } => {
            let chunk = serde_json::json!({
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": text }
                }]
            });
            out.push_str(&format!("data: {chunk}\n\n"));
            let fin = serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "usage": {
                    "prompt_cache_hit_tokens": cache_hit,
                    "prompt_cache_miss_tokens": cache_miss
                }
            });
            out.push_str(&format!("data: {fin}\n\n"));
            out.push_str("data: [DONE]\n\n");
        }
        MockStep::ToolCalls {
            tools,
            cache_hit,
            cache_miss,
        } => {
            for (i, tool) in tools.iter().enumerate() {
                let args = match &tool.arguments {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let chunk = serde_json::json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": i,
                                "id": format!("call_mock_{i}"),
                                "type": "function",
                                "function": {
                                    "name": tool.name,
                                    "arguments": args
                                }
                            }]
                        }
                    }]
                });
                out.push_str(&format!("data: {chunk}\n\n"));
            }
            let fin = serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
                "usage": {
                    "prompt_cache_hit_tokens": cache_hit,
                    "prompt_cache_miss_tokens": cache_miss
                }
            });
            out.push_str(&format!("data: {fin}\n\n"));
            out.push_str("data: [DONE]\n\n");
        }
        MockStep::Empty => {
            out.push_str("data: [DONE]\n\n");
        }
        MockStep::DelayedText { .. } => {
            unreachable!("delayed text is rendered through the delayed response path")
        }
        MockStep::Error { .. } => unreachable!(),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;

    #[tokio::test]
    async fn mock_serves_text_sse() {
        let script = MockScript {
            steps: vec![MockStep::Text {
                text: "hello".into(),
                cache_hit: 1,
                cache_miss: 2,
            }],
        };
        let server = start_mock_server(script).await.unwrap();
        let client = Client::builder().no_proxy().build().unwrap();
        let url = format!("{}/chat/completions", server.base_url);
        let resp = client
            .post(&url)
            .bearer_auth("sk-test")
            .json(&serde_json::json!({
                "model": "mock",
                "messages": [{"role":"user","content":"hi"}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.text().await.unwrap();
        assert!(
            status.is_success(),
            "mock server returned {status}: {body}"
        );
        assert!(body.contains("hello"));
        assert!(body.contains("[DONE]"));
        server.shutdown().await;
    }
}
