use crate::types::StreamEvent;
use anyhow::Result;
use futures::stream::{self, Stream};
use reqwest::Response;
use serde_json::Value;
use tracing::debug;

pub struct SseParser;

impl SseParser {
    pub fn parse_stream(response: Response) -> impl Stream<Item = Result<StreamEvent>> {
        stream::unfold(
            (response, String::new(), false),
            |(mut resp, mut buffer, mut done)| async move {
                if done {
                    return None;
                }

                loop {
                    if let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                done = true;
                                return Some((Ok(StreamEvent::Done), (resp, buffer, done)));
                            }

                            if data.len() < 300 {
                                debug!(%data, "SSE data");
                            } else {
                                debug!(len = data.len(), prefix = %&data[..200], "SSE data (truncated)");
                            }

                            match parse_sse_data(data) {
                                Ok(event) => {
                                    return Some((Ok(event), (resp, buffer, done)));
                                }
                                Err(e) => {
                                    debug!(error = %e, %data, "SSE parse error");
                                    continue;
                                }
                            }
                        }
                        continue;
                    }

                    match resp.chunk().await {
                        Ok(Some(chunk)) => {
                            let text = String::from_utf8_lossy(&chunk);
                            if text.len() < 300 {
                                debug!(chunk = %text, "SSE raw chunk");
                            } else {
                                debug!(len = text.len(), prefix = %&text[..200], "SSE raw chunk (truncated)");
                            }
                            buffer.push_str(&text);
                        }
                    Ok(None) => {
                        if !buffer.trim().is_empty() {
                            let trimmed = buffer.trim();
                            if trimmed.starts_with('{') {
                                debug!(body = %trimmed, "SSE stream ended with non-SSE JSON (likely an API error)");
                                done = true;
                                return Some((
                                    Err(anyhow::anyhow!(
                                        "API returned non-SSE response: {}",
                                        if trimmed.len() > 300 { &trimmed[..300] } else { trimmed }
                                    )),
                                    (resp, buffer, done),
                                ));
                            }
                            debug!(remaining = %trimmed, "SSE stream ended with unparsed data");
                        }
                        done = true;
                        return Some((Ok(StreamEvent::Done), (resp, buffer, done)));
                    }
                        Err(e) => {
                            done = true;
                            return Some((
                                Err(anyhow::anyhow!("stream error: {e}")),
                                (resp, buffer, done),
                            ));
                        }
                    }
                }
            },
        )
    }
}

fn parse_sse_data(data: &str) -> Result<StreamEvent> {
    let v: Value = serde_json::from_str(data)?;

    let choices = v["choices"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no choices array"))?;

    if choices.is_empty() {
        return Ok(StreamEvent::Done);
    }

    let choice = &choices[0];
    let delta = &choice["delta"];

    // ── Tool calls MUST be processed before finish_reason.  ────────
    // NVIDIA's DeepSeek Flash gateway sometimes packs the last
    // tool-call fragment and `finish_reason: "tool_calls"` into a
    // single SSE chunk.  If we short-circuit on finish_reason first,
    // the trailing argument delta is dropped and the accumulator
    // builds a broken partial call → crash.
    if let Some(tool_calls) = delta["tool_calls"].as_array()
        && let Some(tc) = tool_calls.first()
    {
        let index = tc["index"].as_u64().unwrap_or(0) as usize;
        return Ok(StreamEvent::ToolCallDelta {
            index,
            id: tc["id"].as_str().map(|s| s.to_string()),
            function_name: tc["function"]["name"].as_str().map(|s| s.to_string()),
            arguments_delta: tc["function"]["arguments"].as_str().map(|s| s.to_string()),
        });
    }

    // finish_reason AFTER tool_calls — when they share a chunk the
    // tool delta above takes priority.  Usage info is best-effort:
    // if finish_reason is alone (empty delta) or follows a tool_call
    // in a separate chunk, we still capture it here.
    if let Some(finish_reason) = choice["finish_reason"].as_str()
        && (finish_reason == "stop" || finish_reason == "tool_calls")
    {
        let hit = v["usage"]["prompt_cache_hit_tokens"].as_u64();
        let miss = v["usage"]["prompt_cache_miss_tokens"].as_u64();
        return Ok(StreamEvent::CompleteWithUsage {
            prompt_cache_hit_tokens: hit,
            prompt_cache_miss_tokens: miss,
        });
    }

    if let Some(thinking) = delta["reasoning_content"].as_str()
        && !thinking.is_empty()
    {
        return Ok(StreamEvent::ThinkingDelta(thinking.to_string()));
    }

    if let Some(content) = delta["content"].as_str()
        && !content.is_empty()
    {
        return Ok(StreamEvent::TextDelta(content.to_string()));
    }

    Ok(StreamEvent::TextDelta(String::new()))
}

pub struct ToolCallAccumulator {
    calls: std::collections::HashMap<usize, PartialToolCall>,
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    function_name: Option<String>,
    arguments: String,
}

impl Default for ToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self {
            calls: std::collections::HashMap::new(),
        }
    }

    pub fn push(&mut self, event: StreamEvent) {
        if let StreamEvent::ToolCallDelta {
            index,
            id,
            function_name,
            arguments_delta,
        } = event
        {
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = id {
                entry.id = Some(id);
            }
            if let Some(name) = function_name {
                entry.function_name = Some(name);
            }
            if let Some(delta) = arguments_delta {
                entry.arguments.push_str(&delta);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn into_tool_calls(self) -> Vec<crate::types::ToolCall> {
        let mut sorted: Vec<_> = self.calls.into_iter().collect();
        sorted.sort_by_key(|(i, _)| *i);

        let mut seen_ids = std::collections::HashSet::new();

        sorted
            .into_iter()
            .filter_map(|(i, partial)| {
                let mut id = partial
                    .id
                    .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4()));

                while !seen_ids.insert(id.clone()) {
                    id = format!("{}_{}", id, i);
                }

                Some(crate::types::ToolCall {
                    id,
                    call_type: "function".to_string(),
                    function: crate::types::FunctionCall {
                        name: partial.function_name?,
                        arguments: partial.arguments,
                    },
                })
            })
            .collect()
    }
}

// ── Token accumulator (IPC debouncer) ───────────────────────────────

/// Time-and-size based accumulator that batches streaming text/thinking
/// deltas before emitting them, reducing IPC traffic by ~90%.
///
/// A flush is triggered when:
/// 1. `max_interval` has elapsed since the last flush (default 50ms ~ 20fps), or
/// 2. `max_chars` of pending text has accumulated (default 256), or
/// 3. `force_flush()` is called (e.g. on `StreamEvent::Done`).
///
/// Pending text and thinking are tracked separately so they don't get mixed.
pub struct TokenAccumulator {
    text: String,
    thinking: String,
    last_flush: std::time::Instant,
    max_interval: std::time::Duration,
    max_chars: usize,
}

impl TokenAccumulator {
    pub fn new() -> Self {
        Self::with_params(std::time::Duration::from_millis(50), 256)
    }

    pub fn with_params(max_interval: std::time::Duration, max_chars: usize) -> Self {
        Self {
            text: String::new(),
            thinking: String::new(),
            last_flush: std::time::Instant::now(),
            max_interval,
            max_chars,
        }
    }

    pub fn push_text(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    pub fn push_thinking(&mut self, delta: &str) {
        self.thinking.push_str(delta);
    }

    pub fn should_flush(&self) -> bool {
        let total = self.text.len() + self.thinking.len();
        total >= self.max_chars || self.last_flush.elapsed() >= self.max_interval
    }

    /// Drain pending text/thinking into `(text, thinking)`, resetting the timer.
    /// Returns `None` if nothing is pending.
    pub fn flush(&mut self) -> Option<(String, String)> {
        if self.text.is_empty() && self.thinking.is_empty() {
            return None;
        }
        let text = std::mem::take(&mut self.text);
        let thinking = std::mem::take(&mut self.thinking);
        self.last_flush = std::time::Instant::now();
        Some((text, thinking))
    }

    /// Force a flush regardless of thresholds (e.g. on stream end).
    pub fn force_flush(&mut self) -> Option<(String, String)> {
        self.last_flush = std::time::Instant::now() - self.max_interval;
        self.flush()
    }
}

impl Default for TokenAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod accumulator_tests {
    use super::*;

    #[test]
    fn flush_when_empty_returns_none() {
        let mut acc = TokenAccumulator::new();
        assert!(acc.flush().is_none());
    }

    #[test]
    fn flush_drains_pending_text() {
        let mut acc = TokenAccumulator::new();
        acc.push_text("hello ");
        acc.push_text("world");
        let (text, thinking) = acc.force_flush().unwrap();
        assert_eq!(text, "hello world");
        assert!(thinking.is_empty());
        assert!(acc.flush().is_none());
    }

    #[test]
    fn text_and_thinking_kept_separate() {
        let mut acc = TokenAccumulator::new();
        acc.push_text("answer");
        acc.push_thinking("reasoning");
        let (text, thinking) = acc.force_flush().unwrap();
        assert_eq!(text, "answer");
        assert_eq!(thinking, "reasoning");
    }

    #[test]
    fn should_flush_by_size() {
        let mut acc = TokenAccumulator::with_params(std::time::Duration::from_secs(60), 10);
        acc.push_text("1234567890");
        assert!(acc.should_flush());
    }

    #[test]
    fn should_not_flush_below_thresholds() {
        let mut acc = TokenAccumulator::with_params(std::time::Duration::from_secs(60), 1000);
        acc.push_text("short");
        assert!(!acc.should_flush());
    }
}
