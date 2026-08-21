//! Shared turn-loop body for Interactive (`Run`) and Nested (`Subagent`).
//!
//! The adapters keep different identity (FSM, mailbox, transcript, recursion).
//! Compact, SSE collection, and stream retry live here so they are fixed once.

use anyhow::Result;
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator, ToolPreparingNotify};
use crate::client::{ClientCacheHint, OpenAIClient};
use crate::context::Context;
use crate::types::{
    CacheUsage, Message, MessageDelta, ReasoningState, StreamEvent, ToolCall, ToolDefinition,
    ToolExecutionMode,
};

const LATENCY_GAP_WARN_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    DualTranscript,
    TrimToFit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopPolicy {
    pub compact: CompactMode,
    pub recovery: bool,
    pub tool_mode: ToolExecutionMode,
    pub ask_user: bool,
    pub steering: bool,
}

impl LoopPolicy {
    pub fn interactive() -> Self {
        Self {
            compact: CompactMode::DualTranscript,
            recovery: true,
            tool_mode: ToolExecutionMode::Parallel,
            ask_user: true,
            steering: true,
        }
    }

    pub fn nested() -> Self {
        Self {
            compact: CompactMode::TrimToFit,
            recovery: false,
            tool_mode: ToolExecutionMode::Sequential,
            ask_user: false,
            steering: false,
        }
    }

    pub fn max_stream_attempts(&self) -> u32 {
        if self.recovery {
            6
        } else {
            3
        }
    }

    pub fn stream_idle_timeout(&self) -> Option<Duration> {
        self.recovery.then_some(Duration::from_secs(90))
    }

    pub fn request_backoff_base_ms(&self) -> u64 {
        if self.recovery {
            1_000
        } else {
            500
        }
    }

    pub fn stream_backoff_base_ms(&self) -> u64 {
        1_000
    }

    pub fn max_backoff_ms(&self) -> u64 {
        30_000
    }

    pub fn reject_empty_response(&self) -> bool {
        self.recovery
    }

    pub fn latency_trace(&self) -> bool {
        self.recovery
    }
}

/// Accumulated streaming text/thinking while a model call is in flight.
#[derive(Debug, Clone, Default)]
pub struct StreamPartial {
    pub text: String,
    pub thinking: String,
    pub message_id: Option<String>,
}

impl StreamPartial {
    pub fn merge_attempt(&mut self, attempt: &StreamPartial) {
        if attempt.text.len() > self.text.len() {
            self.text.clone_from(&attempt.text);
        }
        if attempt.thinking.len() > self.thinking.len() {
            self.thinking.clone_from(&attempt.thinking);
        }
        if attempt.message_id.is_some() {
            self.message_id.clone_from(&attempt.message_id);
        }
    }

    pub fn recoverable_text(&self) -> String {
        crate::hygiene::wrap_thinking(&self.thinking, &self.text)
    }
}

/// Result of one successful model stream collection.
#[derive(Debug, Clone)]
pub struct CollectedStream {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub message_id: String,
    pub cache_usage: CacheUsage,
    pub reasoning_blob: ReasoningState,
}

pub struct StreamCallbacks<'a> {
    pub on_delta: &'a (dyn Fn(&str, MessageDelta) + Send + Sync),
    pub on_tool_preparing: Option<&'a (dyn Fn(ToolPreparingNotify) + Send + Sync)>,
    pub on_partial: Option<&'a (dyn Fn(&StreamPartial) + Send + Sync)>,
}

pub struct ModelCall<'a> {
    pub client: &'a OpenAIClient,
    pub policy: LoopPolicy,
    pub messages: Vec<Message>,
    pub tools: &'a [ToolDefinition],
    pub cache_hint: Option<ClientCacheHint>,
    pub required_tool: Option<&'a str>,
    pub lifetime_cancel: Option<CancellationToken>,
    pub turn_cancel: Option<CancellationToken>,
    pub callbacks: StreamCallbacks<'a>,
    pub on_retry: Option<&'a (dyn Fn(u32, u64, &str) + Send + Sync)>,
    pub on_stream_open: Option<&'a (dyn Fn() + Send + Sync)>,
}

/// Compact stage of the shared turn body. Dual-transcript compaction stays
/// in Interactive; Nested only trims to the context window.
pub fn apply_compact(policy: LoopPolicy, context: &mut Context) {
    match policy.compact {
        CompactMode::TrimToFit => {
            let _ = context.trim_to_fit();
        }
        CompactMode::DualTranscript => {}
    }
}

/// Some reasoning-heavy models put the final answer in `reasoning_content`
/// with an empty `content` field. Promote thinking → text so callers see it.
pub fn promote_thinking_to_text(text: &mut String, thinking: &mut String) -> bool {
    if text.trim().is_empty() && !thinking.trim().is_empty() {
        *text = thinking.trim().to_string();
        thinking.clear();
        true
    } else {
        false
    }
}

/// Model stage: open an SSE stream, collect it, retry on mid-stream drop.
pub async fn run_model_phase(call: ModelCall<'_>) -> Result<CollectedStream> {
    let mut retry_checkpoint = StreamPartial::default();
    let max_attempts = call.policy.max_stream_attempts();
    let mut last_err: Option<String> = None;

    for attempt in 0..max_attempts {
        check_cancel(call.lifetime_cancel.as_ref(), call.turn_cancel.as_ref())?;

        let mut attempt_messages = call.messages.clone();
        crate::hygiene::inject_stream_retry_hint(
            &mut attempt_messages,
            &retry_checkpoint.thinking,
            &retry_checkpoint.text,
        );

        let mut attempt_partial = StreamPartial::default();
        let lifetime = call.lifetime_cancel.clone();
        let turn = call.turn_cancel.clone();
        let stream_res = tokio::select! {
            biased;
            _ = async {
                match &lifetime {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => anyhow::bail!("aborted"),
            _ = async {
                match &turn {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => anyhow::bail!("interrupted by user steer"),
            result = call.client.chat_completion_stream_with_hint_and_required_tool(
                &attempt_messages,
                call.tools,
                call.cache_hint,
                call.required_tool,
            ) => result,
        };

        let collected = match stream_res {
            Ok(stream) => {
                if let Some(on_open) = call.on_stream_open {
                    on_open();
                }
                match collect_model_stream(
                    stream,
                    &mut attempt_partial,
                    StreamCollectOptions {
                        lifetime_cancel: call.lifetime_cancel.clone(),
                        turn_cancel: call.turn_cancel.clone(),
                        idle_timeout: call.policy.stream_idle_timeout(),
                        latency_trace: call.policy.latency_trace(),
                    },
                    &call.callbacks,
                )
                .await
                {
                    Ok(r) => {
                        if call.policy.reject_empty_response()
                            && r.text.is_empty()
                            && r.tool_calls.is_empty()
                        {
                            Err(
                                "empty response from model — SSE stream had no useful events"
                                    .to_string(),
                            )
                        } else {
                            Ok(r)
                        }
                    }
                    Err(e) => {
                        if cancelled(call.lifetime_cancel.as_ref()) {
                            anyhow::bail!("aborted");
                        }
                        if cancelled(call.turn_cancel.as_ref()) {
                            anyhow::bail!("interrupted by user steer");
                        }
                        Err(format!("Stream error: {e}"))
                    }
                }
            }
            Err(e) => Err(e.to_string()),
        };

        match collected {
            Ok(r) => return Ok(r),
            Err(err_msg) => {
                retry_checkpoint.merge_attempt(&attempt_partial);
                if let Some(on_partial) = call.callbacks.on_partial {
                    on_partial(&retry_checkpoint);
                }
                last_err = Some(err_msg.clone());
                tracing::warn!(attempt, error = %err_msg, "stream attempt failed");
                if attempt + 1 >= max_attempts {
                    break;
                }
                let base = if err_msg.starts_with("Stream error:") {
                    call.policy.stream_backoff_base_ms()
                } else {
                    call.policy.request_backoff_base_ms()
                };
                let delay_ms = (base * 2u64.pow(attempt)).min(call.policy.max_backoff_ms());
                if let Some(on_retry) = call.on_retry {
                    on_retry(attempt, delay_ms, &err_msg);
                }
                tokio::select! {
                    biased;
                    _ = async {
                        match &call.lifetime_cancel {
                            Some(token) => token.cancelled().await,
                            None => std::future::pending().await,
                        }
                    } => anyhow::bail!("aborted"),
                    _ = async {
                        match &call.turn_cancel {
                            Some(token) => token.cancelled().await,
                            None => std::future::pending().await,
                        }
                    } => anyhow::bail!("interrupted by user steer"),
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                }
            }
        }
    }

    anyhow::bail!(
        "{}",
        last_err.unwrap_or_else(|| "exhausted stream retry attempts".to_string())
    )
}

pub struct StreamCollectOptions {
    pub lifetime_cancel: Option<CancellationToken>,
    pub turn_cancel: Option<CancellationToken>,
    pub idle_timeout: Option<Duration>,
    pub latency_trace: bool,
}

/// Collect one SSE stream into text / thinking / tool calls.
pub async fn collect_model_stream(
    stream: impl futures::Stream<Item = Result<StreamEvent>>,
    partial: &mut StreamPartial,
    opts: StreamCollectOptions,
    callbacks: &StreamCallbacks<'_>,
) -> Result<CollectedStream> {
    tracing::debug!("LATENCY: collect_stream start");
    let stream_t0 = Instant::now();
    let mut text_buffer = String::new();
    let mut thinking_buffer = String::new();
    let mut reasoning_blob = ReasoningState::default();
    let mut accumulator = ToolCallAccumulator::new();
    let mut has_tool_calls = false;
    let mut cache_usage = CacheUsage::default();
    let mut tokens = TokenAccumulator::new();
    let message_id = uuid::Uuid::new_v4().to_string();
    partial.message_id = Some(message_id.clone());

    let mut first_event_ms: Option<u64> = None;
    let mut first_thinking_ms: Option<u64> = None;
    let mut last_thinking_ms: Option<u64> = None;
    let mut first_text_ms: Option<u64> = None;
    let mut last_text_ms: Option<u64> = None;
    let mut first_tool_ms: Option<u64> = None;
    let mut first_tool_name: Option<String> = None;
    let mut first_preparing_ms: Option<u64> = None;
    let mut last_tool_delta_ms: Option<u64> = None;
    let mut tool_delta_count: u64 = 0;
    let mut thinking_delta_count: u64 = 0;
    let mut text_delta_count: u64 = 0;
    let mut last_event_at = stream_t0;
    let mut last_event_kind = "start";
    let mut max_gap_ms: u64 = 0;
    let mut max_gap_from = "start";
    let mut max_gap_to = "start";

    let stamp = |kind: &'static str,
                 now: Instant,
                 last_at: &mut Instant,
                 last_kind: &mut &'static str,
                 max_gap: &mut u64,
                 max_from: &mut &'static str,
                 max_to: &mut &'static str| {
        if !opts.latency_trace {
            return;
        }
        let gap = now.duration_since(*last_at).as_millis() as u64;
        if gap > *max_gap {
            *max_gap = gap;
            *max_from = *last_kind;
            *max_to = kind;
        }
        if gap >= LATENCY_GAP_WARN_MS {
            tracing::warn!(
                gap_ms = gap,
                from = %last_kind,
                to = %kind,
                since_start_ms = now.duration_since(stream_t0).as_millis() as u64,
                "LATENCY: stream gap"
            );
        }
        *last_at = now;
        *last_kind = kind;
    };

    let emit_flush = |tokens: &mut TokenAccumulator, force: bool| {
        let flushed = if force {
            tokens.force_flush()
        } else {
            tokens.flush()
        };
        if let Some((text, thinking)) = flushed {
            if !text.is_empty() {
                (callbacks.on_delta)(&message_id, MessageDelta::Text(text));
            }
            if !thinking.is_empty() {
                (callbacks.on_delta)(&message_id, MessageDelta::Thinking(thinking));
            }
        }
    };

    tokio::pin!(stream);
    loop {
        let flush_delay = tokens.pending_flush_delay();
        let next = tokio::select! {
            biased;
            _ = async {
                match &opts.lifetime_cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => anyhow::bail!("aborted"),
            _ = async {
                match &opts.turn_cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => anyhow::bail!("interrupted by user steer"),
            _ = tokio::time::sleep(flush_delay.unwrap_or(Duration::ZERO)),
                if flush_delay.is_some() => {
                emit_flush(&mut tokens, false);
                continue;
            }
            result = next_event(&mut stream, opts.idle_timeout) => result,
        };
        let event = match next? {
            Some(event) => event?,
            None => break,
        };
        let now = Instant::now();
        let since = now.duration_since(stream_t0).as_millis() as u64;
        if first_event_ms.is_none() {
            first_event_ms = Some(since);
            tracing::debug!(ttfe_ms = since, "LATENCY: first stream event");
        }

        match event {
            StreamEvent::TextDelta(delta) => {
                if !delta.is_empty() {
                    stamp(
                        "text",
                        now,
                        &mut last_event_at,
                        &mut last_event_kind,
                        &mut max_gap_ms,
                        &mut max_gap_from,
                        &mut max_gap_to,
                    );
                    text_delta_count += 1;
                    if first_text_ms.is_none() {
                        first_text_ms = Some(since);
                        tracing::debug!(
                            since_start_ms = since,
                            chars = delta.len(),
                            "LATENCY: first text delta"
                        );
                    }
                    last_text_ms = Some(since);
                }
                tokens.push_text(&delta);
                text_buffer.push_str(&delta);
                partial.text.push_str(&delta);
                if let Some(on_partial) = callbacks.on_partial {
                    on_partial(partial);
                }
                if tokens.should_flush() {
                    emit_flush(&mut tokens, false);
                }
            }
            StreamEvent::ThinkingDelta(delta) => {
                if !delta.is_empty() {
                    stamp(
                        "thinking",
                        now,
                        &mut last_event_at,
                        &mut last_event_kind,
                        &mut max_gap_ms,
                        &mut max_gap_from,
                        &mut max_gap_to,
                    );
                    thinking_delta_count += 1;
                    if first_thinking_ms.is_none() {
                        first_thinking_ms = Some(since);
                        tracing::debug!(
                            since_start_ms = since,
                            chars = delta.len(),
                            "LATENCY: first thinking delta"
                        );
                    }
                    last_thinking_ms = Some(since);
                }
                tokens.push_thinking(&delta);
                thinking_buffer.push_str(&delta);
                partial.thinking.push_str(&delta);
                if let Some(on_partial) = callbacks.on_partial {
                    on_partial(partial);
                }
                if tokens.should_flush() {
                    emit_flush(&mut tokens, false);
                }
            }
            StreamEvent::ReasoningBlob {
                encrypted_content,
                signature,
                summary,
            } => {
                stamp(
                    "reasoning_blob",
                    now,
                    &mut last_event_at,
                    &mut last_event_kind,
                    &mut max_gap_ms,
                    &mut max_gap_from,
                    &mut max_gap_to,
                );
                if let Some(blob) = encrypted_content {
                    if !blob.is_empty() {
                        reasoning_blob.encrypted_content = Some(blob);
                    }
                }
                if let Some(sig) = signature {
                    if !sig.is_empty() {
                        match &mut reasoning_blob.signature {
                            Some(existing) => existing.push_str(&sig),
                            None => reasoning_blob.signature = Some(sig),
                        }
                    }
                }
                if let Some(s) = summary {
                    if !s.is_empty() {
                        reasoning_blob.summary = Some(s);
                    }
                }
            }
            StreamEvent::ToolCallDelta { .. } => {
                stamp(
                    "tool_delta",
                    now,
                    &mut last_event_at,
                    &mut last_event_kind,
                    &mut max_gap_ms,
                    &mut max_gap_from,
                    &mut max_gap_to,
                );
                has_tool_calls = true;
                tool_delta_count += 1;
                last_tool_delta_ms = Some(since);
                if first_tool_ms.is_none() {
                    first_tool_ms = Some(since);
                    let gap_after_thinking = last_thinking_ms
                        .map(|t| since.saturating_sub(t))
                        .unwrap_or(since);
                    let gap_after_text = last_text_ms.map(|t| since.saturating_sub(t)).unwrap_or(0);
                    tracing::info!(
                        since_start_ms = since,
                        gap_after_last_thinking_ms = gap_after_thinking,
                        gap_after_last_text_ms = gap_after_text,
                        "LATENCY: first tool_call delta"
                    );
                }
                if let Some(notify) = accumulator.push(event) {
                    if first_tool_name.is_none() {
                        if let Some(ref name) = notify.name {
                            first_tool_name = Some(name.clone());
                        }
                    }
                    if first_preparing_ms.is_none() {
                        first_preparing_ms = Some(since);
                        tracing::debug!(
                            since_start_ms = since,
                            index = notify.index,
                            name = ?notify.name,
                            call_id = ?notify.call_id,
                            hint_path = ?notify.hint_path,
                            "LATENCY: first tool_preparing emit"
                        );
                    }
                    if let Some(on_prep) = callbacks.on_tool_preparing {
                        on_prep(notify);
                    }
                }
            }
            StreamEvent::Done => {
                stamp(
                    "done",
                    now,
                    &mut last_event_at,
                    &mut last_event_kind,
                    &mut max_gap_ms,
                    &mut max_gap_from,
                    &mut max_gap_to,
                );
                break;
            }
            StreamEvent::CacheUsage {
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
            } => {
                cache_usage = CacheUsage {
                    hit_tokens: prompt_cache_hit_tokens.unwrap_or(0),
                    miss_tokens: prompt_cache_miss_tokens.unwrap_or(0),
                };
            }
            StreamEvent::CompleteWithUsage {
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
            } => {
                stamp(
                    "complete_usage",
                    now,
                    &mut last_event_at,
                    &mut last_event_kind,
                    &mut max_gap_ms,
                    &mut max_gap_from,
                    &mut max_gap_to,
                );
                cache_usage = CacheUsage {
                    hit_tokens: prompt_cache_hit_tokens.unwrap_or(0),
                    miss_tokens: prompt_cache_miss_tokens.unwrap_or(0),
                };
                break;
            }
        }
    }

    emit_flush(&mut tokens, true);

    let tool_calls = if has_tool_calls {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            accumulator.into_tool_calls()
        })) {
            Ok(calls) => calls,
            Err(_) => {
                tracing::error!("TURN: tool_calls accumulator panicked");
                anyhow::bail!("tool call accumulator panicked — incomplete SSE stream");
            }
        }
    } else {
        vec![]
    };

    if opts.latency_trace {
        let total_ms = stream_t0.elapsed().as_millis() as u64;
        let thinking_to_tool_ms = match (last_thinking_ms, first_tool_ms) {
            (Some(t), Some(f)) => Some(f.saturating_sub(t)),
            _ => None,
        };
        let text_to_tool_ms = match (last_text_ms, first_tool_ms) {
            (Some(t), Some(f)) => Some(f.saturating_sub(t)),
            _ => None,
        };
        let tool_args_span_ms = match (first_tool_ms, last_tool_delta_ms) {
            (Some(f), Some(l)) => Some(l.saturating_sub(f)),
            _ => None,
        };
        tracing::info!(
            total_ms,
            first_event_ms,
            first_thinking_ms,
            last_thinking_ms,
            first_text_ms,
            last_text_ms,
            first_tool_ms,
            first_preparing_ms,
            last_tool_delta_ms,
            thinking_to_tool_ms,
            text_to_tool_ms,
            tool_args_span_ms,
            max_gap_ms,
            max_gap_from,
            max_gap_to,
            thinking_delta_count,
            text_delta_count,
            tool_delta_count,
            thinking_chars = thinking_buffer.len(),
            text_chars = text_buffer.len(),
            tool_count = tool_calls.len(),
            first_tool_name = ?first_tool_name,
            "LATENCY: collect_stream summary"
        );
    }

    Ok(CollectedStream {
        text: text_buffer,
        thinking: thinking_buffer,
        tool_calls,
        message_id,
        cache_usage,
        reasoning_blob,
    })
}

async fn next_event<S>(
    stream: &mut std::pin::Pin<&mut S>,
    idle_timeout: Option<Duration>,
) -> Result<Option<Result<StreamEvent>>>
where
    S: futures::Stream<Item = Result<StreamEvent>>,
{
    match idle_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
            Ok(item) => Ok(item),
            Err(_) => anyhow::bail!("model stream idle timeout after {}s", timeout.as_secs()),
        },
        None => Ok(stream.next().await),
    }
}

fn cancelled(token: Option<&CancellationToken>) -> bool {
    token.is_some_and(CancellationToken::is_cancelled)
}

fn check_cancel(
    lifetime: Option<&CancellationToken>,
    turn: Option<&CancellationToken>,
) -> Result<()> {
    if cancelled(lifetime) {
        anyhow::bail!("aborted");
    }
    if cancelled(turn) {
        anyhow::bail!("interrupted by user steer");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_does_not_gain_interactive_ports() {
        let nested = LoopPolicy::nested();
        assert!(!nested.recovery);
        assert!(!nested.ask_user);
        assert!(!nested.steering);
        assert_eq!(nested.compact, CompactMode::TrimToFit);
        assert_eq!(nested.tool_mode, ToolExecutionMode::Sequential);
        assert_eq!(nested.max_stream_attempts(), 3);
        assert!(nested.stream_idle_timeout().is_none());
        assert!(!nested.reject_empty_response());
    }

    #[test]
    fn interactive_keeps_steer_and_ask_user() {
        let interactive = LoopPolicy::interactive();
        assert!(interactive.recovery);
        assert!(interactive.ask_user);
        assert!(interactive.steering);
        assert_eq!(interactive.compact, CompactMode::DualTranscript);
        assert_eq!(interactive.max_stream_attempts(), 6);
        assert!(interactive.stream_idle_timeout().is_some());
        assert!(interactive.reject_empty_response());
    }

    #[test]
    fn stream_partial_keeps_longest_attempt() {
        let mut acc = StreamPartial {
            text: "ab".into(),
            thinking: "t".into(),
            message_id: Some("1".into()),
        };
        acc.merge_attempt(&StreamPartial {
            text: "abcd".into(),
            thinking: String::new(),
            message_id: None,
        });
        assert_eq!(acc.text, "abcd");
        assert_eq!(acc.thinking, "t");
        assert_eq!(acc.message_id.as_deref(), Some("1"));
    }

    #[test]
    fn promote_thinking_fills_empty_text() {
        let mut text = String::new();
        let mut thinking = " the answer ".into();
        assert!(promote_thinking_to_text(&mut text, &mut thinking));
        assert_eq!(text, "the answer");
        assert!(thinking.is_empty());
    }

    #[test]
    fn promote_thinking_leaves_existing_text() {
        let mut text = "keep".into();
        let mut thinking = "hidden".into();
        assert!(!promote_thinking_to_text(&mut text, &mut thinking));
        assert_eq!(text, "keep");
        assert_eq!(thinking, "hidden");
    }

    #[tokio::test]
    async fn collect_model_stream_concatenates_text_and_emits_deltas() {
        let events = vec![
            Ok(StreamEvent::TextDelta("hel".into())),
            Ok(StreamEvent::TextDelta("lo".into())),
            Ok(StreamEvent::Done),
        ];
        let stream = futures::stream::iter(events);
        let mut partial = StreamPartial::default();
        let deltas = std::sync::Mutex::new(Vec::new());
        let on_delta = |_: &str, delta: MessageDelta| deltas.lock().unwrap().push(delta);
        let callbacks = StreamCallbacks {
            on_delta: &on_delta,
            on_tool_preparing: None,
            on_partial: None,
        };
        let collected = collect_model_stream(
            stream,
            &mut partial,
            StreamCollectOptions {
                lifetime_cancel: None,
                turn_cancel: None,
                idle_timeout: None,
                latency_trace: false,
            },
            &callbacks,
        )
        .await
        .unwrap();
        assert_eq!(collected.text, "hello");
        assert!(collected.tool_calls.is_empty());
        assert_eq!(partial.text, "hello");
        let deltas = deltas.lock().unwrap();
        let joined: String = deltas
            .iter()
            .filter_map(|d| match d {
                MessageDelta::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(joined, "hello");
    }

    #[tokio::test]
    async fn collect_model_stream_keeps_tool_calls() {
        let events = vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                function_name: Some("read_file".into()),
                arguments_delta: Some(r#"{"path":"a.rs"}"#.into()),
            }),
            Ok(StreamEvent::Done),
        ];
        let stream = futures::stream::iter(events);
        let mut partial = StreamPartial::default();
        let on_delta = |_: &str, _: MessageDelta| {};
        let preparing = std::sync::atomic::AtomicUsize::new(0);
        let on_prep = |_: ToolPreparingNotify| {
            preparing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        };
        let callbacks = StreamCallbacks {
            on_delta: &on_delta,
            on_tool_preparing: Some(&on_prep),
            on_partial: None,
        };
        let collected = collect_model_stream(
            stream,
            &mut partial,
            StreamCollectOptions {
                lifetime_cancel: None,
                turn_cancel: None,
                idle_timeout: None,
                latency_trace: false,
            },
            &callbacks,
        )
        .await
        .unwrap();
        assert_eq!(collected.tool_calls.len(), 1);
        assert_eq!(collected.tool_calls[0].id, "call_1");
        assert_eq!(collected.tool_calls[0].function.name, "read_file");
        assert_eq!(
            collected.tool_calls[0].function.arguments,
            r#"{"path":"a.rs"}"#
        );
        assert!(preparing.load(std::sync::atomic::Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn collect_model_stream_stops_on_turn_cancel() {
        let token = tokio_util::sync::CancellationToken::new();
        let stream = futures::stream::pending::<Result<StreamEvent>>();
        let mut partial = StreamPartial::default();
        let on_delta = |_: &str, _: MessageDelta| {};
        let callbacks = StreamCallbacks {
            on_delta: &on_delta,
            on_tool_preparing: None,
            on_partial: None,
        };
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            collect_model_stream(
                stream,
                &mut partial,
                StreamCollectOptions {
                    lifetime_cancel: None,
                    turn_cancel: Some(token.clone()),
                    idle_timeout: Some(Duration::from_secs(90)),
                    latency_trace: false,
                },
                &callbacks,
            ),
        )
        .await;
        let err = result
            .expect("cancel must not wait for the idle timeout")
            .expect_err("cancelled stream must error");
        assert!(
            err.to_string().contains("interrupted"),
            "unexpected error: {err}"
        );
    }
}
