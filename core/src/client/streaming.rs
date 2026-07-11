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

    // ── OpenAI Responses API events ────────────────────────────────
    if let Some(event_type) = v["type"].as_str() {
        match event_type {
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = v["delta"].as_str() {
                    return Ok(StreamEvent::ThinkingDelta(delta.to_string()));
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = v["delta"].as_str() {
                    return Ok(StreamEvent::TextDelta(delta.to_string()));
                }
            }
            "response.output_item.done" => {
                let item = &v["item"];
                if item["type"].as_str() == Some("reasoning") {
                    return Ok(StreamEvent::ReasoningBlob {
                        encrypted_content: item["encrypted_content"].as_str().map(|s| s.to_string()),
                        signature: None,
                        summary: item["summary"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|s| s["text"].as_str())
                            .map(|s| s.to_string()),
                    });
                }
                if item["type"].as_str() == Some("function_call") {
                    return Ok(StreamEvent::ToolCallDelta {
                        index: 0,
                        id: item["call_id"].as_str().map(|s| s.to_string()),
                        function_name: item["name"].as_str().map(|s| s.to_string()),
                        arguments_delta: item["arguments"].as_str().map(|s| s.to_string()),
                    });
                }
            }
            "response.completed" | "response.done" => {
                return Ok(StreamEvent::Done);
            }
            _ => {}
        }
    }

    // ── Anthropic Messages SSE ─────────────────────────────────────
    if let Some(delta_type) = v["delta"]["type"].as_str() {
        match delta_type {
            "thinking_delta" => {
                if let Some(t) = v["delta"]["thinking"].as_str() {
                    return Ok(StreamEvent::ThinkingDelta(t.to_string()));
                }
            }
            "signature_delta" => {
                if let Some(sig) = v["delta"]["signature"].as_str() {
                    return Ok(StreamEvent::ReasoningBlob {
                        encrypted_content: None,
                        signature: Some(sig.to_string()),
                        summary: None,
                    });
                }
            }
            "text_delta" => {
                if let Some(t) = v["delta"]["text"].as_str() {
                    return Ok(StreamEvent::TextDelta(t.to_string()));
                }
            }
            "input_json_delta" => {
                if let Some(partial) = v["delta"]["partial_json"].as_str() {
                    return Ok(StreamEvent::ToolCallDelta {
                        index: v["index"].as_u64().unwrap_or(0) as usize,
                        id: None,
                        function_name: None,
                        arguments_delta: Some(partial.to_string()),
                    });
                }
            }
            _ => {}
        }
    }
    if v["type"].as_str() == Some("content_block_start") {
        let block = &v["content_block"];
        if block["type"].as_str() == Some("tool_use") {
            return Ok(StreamEvent::ToolCallDelta {
                index: v["index"].as_u64().unwrap_or(0) as usize,
                id: block["id"].as_str().map(|s| s.to_string()),
                function_name: block["name"].as_str().map(|s| s.to_string()),
                arguments_delta: Some(String::new()),
            });
        }
        if block["type"].as_str() == Some("thinking") {
            // thinking block start — signature may arrive later via signature_delta
            return Ok(StreamEvent::TextDelta(String::new()));
        }
    }
    if v["type"].as_str() == Some("message_stop") {
        return Ok(StreamEvent::Done);
    }

    // ── Chat Completions (OpenAI-compat / DeepSeek) ────────────────
    let choices = match v["choices"].as_array() {
        Some(c) => c,
        None => {
            // Unknown shape — ignore quietly rather than fail the stream.
            return Ok(StreamEvent::TextDelta(String::new()));
        }
    };

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

    // Text/thinking BEFORE finish_reason — NVIDIA's DeepSeek gateway (and
    // some other OpenAI-compat providers) pack the last content delta and
    // `finish_reason: "stop"` into a single SSE chunk.  If we short-circuit
    // on finish_reason first the final answer is silently dropped, which is
    // why subagents (and main-agent turns) can end after a tool call with
    // no visible final response.
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

    // finish_reason AFTER tool_calls and content — when they share a chunk
    // the tool/text delta above takes priority.  Usage info is best-effort.
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
    /// Last path hint we already notified the UI about (dedupe).
    notified_hint_path: Option<String>,
    /// Whether we have emitted at least one preparing notification for this slot.
    notified_once: bool,
}

/// Snapshot of a partial tool call worth surfacing to the UI mid-stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPreparingNotify {
    pub index: usize,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub hint_path: Option<String>,
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

    /// Push a tool-call delta. Returns a notify snapshot when name, call_id, or
    /// hint_path first becomes available / changes — so callers can emit
    /// `tool_preparing` without flooding on every args fragment.
    pub fn push(&mut self, event: StreamEvent) -> Option<ToolPreparingNotify> {
        let StreamEvent::ToolCallDelta {
            index,
            id,
            function_name,
            arguments_delta,
        } = event
        else {
            return None;
        };

        let entry = self.calls.entry(index).or_default();
        let mut changed = !entry.notified_once;

        if let Some(id) = id {
            if entry.id.as_ref() != Some(&id) {
                entry.id = Some(id);
                changed = true;
            }
        }
        if let Some(name) = function_name {
            if entry.function_name.as_ref() != Some(&name) {
                entry.function_name = Some(name);
                changed = true;
            }
        }
        if let Some(delta) = arguments_delta {
            entry.arguments.push_str(&delta);
        }

        let hint = extract_path_hint(&entry.arguments);
        if hint.is_some() && hint != entry.notified_hint_path {
            entry.notified_hint_path = hint.clone();
            changed = true;
        }

        if !changed {
            return None;
        }

        entry.notified_once = true;
        Some(ToolPreparingNotify {
            index,
            call_id: entry.id.clone(),
            name: entry.function_name.clone(),
            hint_path: entry.notified_hint_path.clone(),
        })
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

/// Best-effort extract of `"path"` / `"file_path"` from a partial JSON args
/// string. Does not require the full object to be valid JSON.
pub fn extract_path_hint(partial_args: &str) -> Option<String> {
    for key in ["\"path\"", "\"file_path\"", "\"file\""] {
        if let Some(v) = extract_json_string_after_key(partial_args, key) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn extract_json_string_after_key(s: &str, key: &str) -> Option<String> {
    let mut rest = s;
    while let Some(pos) = rest.find(key) {
        rest = &rest[pos + key.len()..];
        let trimmed = rest.trim_start();
        let after_colon = trimmed.strip_prefix(':')?.trim_start();
        let after_quote = after_colon.strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = after_quote.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(n) => out.push(n),
                    None => return None, // incomplete escape
                },
                '"' => return Some(out), // complete string only
                _ => out.push(c),
            }
        }
        // Incomplete string — wait for the closing quote before notifying.
        return None;
    }
    None
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
mod parse_sse_tests {
    use super::*;

    #[test]
    fn finish_reason_stop_with_content_returns_text_not_usage() {
        let data = r#"{"choices":[{"index":0,"delta":{"content":"Shenzhen is 28°C."},"finish_reason":"stop"}]}"#;
        let ev = parse_sse_data(data).unwrap();
        assert!(matches!(ev, StreamEvent::TextDelta(ref s) if s == "Shenzhen is 28°C."));
    }

    #[test]
    fn finish_reason_stop_with_reasoning_returns_thinking_not_usage() {
        let data = r#"{"choices":[{"index":0,"delta":{"reasoning_content":"Let me summarize."},"finish_reason":"stop"}]}"#;
        let ev = parse_sse_data(data).unwrap();
        assert!(matches!(ev, StreamEvent::ThinkingDelta(ref s) if s == "Let me summarize."));
    }

    #[test]
    fn finish_reason_alone_returns_complete_with_usage() {
        let data = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_cache_hit_tokens":10,"prompt_cache_miss_tokens":5}}"#;
        let ev = parse_sse_data(data).unwrap();
        assert!(matches!(
            ev,
            StreamEvent::CompleteWithUsage {
                prompt_cache_hit_tokens: Some(10),
                prompt_cache_miss_tokens: Some(5),
            }
        ));
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

#[cfg(test)]
mod tool_preparing_tests {
    use super::*;

    #[test]
    fn extract_path_from_partial_json() {
        assert_eq!(
            extract_path_hint(r#"{"path": "src/App.tsx", "content":"#),
            Some("src/App.tsx".into())
        );
        assert_eq!(
            extract_path_hint(r#"{"file_path":"/tmp/a.rs""#),
            Some("/tmp/a.rs".into())
        );
        assert_eq!(extract_path_hint(r#"{"content":"hello"}"#), None);
    }

    #[test]
    fn extract_path_incomplete_string() {
        // Incomplete quote — wait until closed
        assert_eq!(extract_path_hint(r#"{"path": "src/App"#), None);
        assert_eq!(
            extract_path_hint(r#"{"path": "src/App.tsx""#),
            Some("src/App.tsx".into())
        );
    }

    #[test]
    fn notify_on_first_name_then_path_not_every_delta() {
        let mut acc = ToolCallAccumulator::new();
        let n1 = acc.push(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            function_name: Some("write_file".into()),
            arguments_delta: Some(r#"{"pa"#.into()),
        });
        assert!(n1.is_some());
        assert_eq!(n1.as_ref().unwrap().name.as_deref(), Some("write_file"));
        assert!(n1.as_ref().unwrap().hint_path.is_none());

        // Incomplete path — no notify
        let n2 = acc.push(StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            function_name: None,
            arguments_delta: Some(r#"th": "src/main.rs"#.into()),
        });
        assert!(n2.is_none());

        // Closing quote completes path — notify once
        let n3 = acc.push(StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            function_name: None,
            arguments_delta: Some(r#"""#.into()),
        });
        assert!(n3.is_some());
        assert_eq!(n3.as_ref().unwrap().hint_path.as_deref(), Some("src/main.rs"));

        // Content deltas — no further notify
        let n4 = acc.push(StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            function_name: None,
            arguments_delta: Some(r#", "content": "aaaa""#.into()),
        });
        assert!(n4.is_none());
    }

    #[test]
    fn second_tool_index_notifies_separately() {
        let mut acc = ToolCallAccumulator::new();
        let _ = acc.push(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c0".into()),
            function_name: Some("write_file".into()),
            arguments_delta: Some(r#"{"path":"a.ts"}"#.into()),
        });
        let n = acc.push(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("c1".into()),
            function_name: Some("write_file".into()),
            arguments_delta: Some(r#"{"path":"b.ts"}"#.into()),
        });
        let n = n.expect("second index should notify");
        assert_eq!(n.index, 1);
        assert_eq!(n.hint_path.as_deref(), Some("b.ts"));
    }
}
