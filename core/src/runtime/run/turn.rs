//! Turn execution — model interaction, streaming collection, and turn dispatch.

use anyhow::Result;
use futures::StreamExt;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

use crate::runtime::tool_orchestrator::ToolOrchestrator;
use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
use crate::client::ClientCacheHint;
use crate::runtime::event::{Envelope, RunEvent, TodoItemPayload};
use crate::runtime::guard::EventGuard;
use crate::types::{CacheUsage, Message, MessageDelta, StreamEvent, ToolCall};

use super::{RecoveryOutcome, Run, RunError, TurnOutcome, CACHE_IDLE_WARN_SECS};

/// Partial stream output preserved across mid-stream retries within one model turn.
#[derive(Default, Clone)]
pub(super) struct StreamPartial {
    pub text: String,
    pub thinking: String,
}

impl StreamPartial {
    /// Keep the longest partial seen so far (model may regenerate from injected hint).
    fn merge_attempt(&mut self, attempt: &StreamPartial) {
        if attempt.text.len() > self.text.len() {
            self.text.clone_from(&attempt.text);
        }
        if attempt.thinking.len() > self.thinking.len() {
            self.thinking.clone_from(&attempt.thinking);
        }
    }
}

impl Run {
    pub(super) async fn run_turn(&mut self, turn_index: usize) -> Result<TurnOutcome, RunError> {
        tracing::info!(
            turn = turn_index,
            context_msgs = self.context.messages().len(),
            "TURN: entering"
        );

        // Stage: Idle detection — warn if the cache likely expired between turns.
        // DeepSeek's prefix cache has an undocumented ~5–10 minute idle timeout.
        // If the user paused for > CACHE_IDLE_WARN_SECS, the next API call will
        // likely be a cache miss. We emit a sentinel so the frontend can adjust
        // its display expectations.
        if let Some(last_end) = self.last_turn_end_time {
            let idle_secs = last_end.elapsed().as_secs();
            if idle_secs >= CACHE_IDLE_WARN_SECS {
                tracing::info!(
                    idle_secs,
                    "Cache likely expired from idle time — next model call may be cache-miss"
                );
                self.emit(RunEvent::CacheInfo {
                    hit_tokens: 0,
                    miss_tokens: 0,
                    hit_rate: -2.0, // -2 signals "cache likely expired from idle"
                });
            }
        }

        // Stage: Refresh
        self.refresh_context_segments();

        // Stage: Verify stable prefix hasn't drifted
        let current_fp = self.context.stable_prefix_fingerprint();
        if current_fp != self.last_prefix_fingerprint {
            self.emit(RunEvent::CacheInfo {
                hit_tokens: 0,
                miss_tokens: 0,
                hit_rate: -1.0, // -1 signals "prefix drifted"
            });
            self.last_prefix_fingerprint = current_fp;
        }

        // Stage: Compact (on-demand only — avoid per-turn cache invalidation)
        self.maybe_compact().await;

        // Stage: Model
        tracing::info!("TURN: calling model_turn");
        let (text, tool_calls, message_id, cache_usage) = match self.model_turn().await {
            Ok(r) => {
                tracing::info!(
                    text_len = r.0.len(),
                    tool_count = r.1.len(),
                    "TURN: model_turn ok"
                );
                r
            }
            Err(e) => {
                if self.cancel.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                tracing::warn!(error = %e, "TURN: model_turn failed");
                let friendly_err = format_user_friendly_error(&e);
                self.emit(RunEvent::Error { message: friendly_err.clone() });
                return Ok(TurnOutcome::Stop(friendly_err));
            }
        };

        // Record the turn timestamp for KV-cache TTL tracking.
        // This lets the next turn's cache_hint() detect idle gaps
        // that would expire provider-side KV caches.
        self.context.record_turn_timestamp();

        // Emit cache telemetry and update cumulative metrics
        if cache_usage.total() > 0 {
            self.cache_metrics.record(cache_usage.hit_tokens, cache_usage.miss_tokens);
            self.emit(RunEvent::CacheInfo {
                hit_tokens: cache_usage.hit_tokens,
                miss_tokens: cache_usage.miss_tokens,
                hit_rate: cache_usage.hit_rate(),
            });
        }

        // Stage: Dispatch
        if tool_calls.is_empty() {
            // Final answer
            let assistant_msg = Message::assistant(&text);
            self.context.add(assistant_msg.clone());
            self.save_session_snapshot();
            self.emit(RunEvent::MessageEnd {
                message_id: message_id.clone(),
                message: assistant_msg.clone(),
            });
            self.emit(RunEvent::TurnEnded { index: turn_index });
            self.hook_registry.lock().fire_turn_end(turn_index);

            // Store in memory
            if let Some(ref mem) = self.brain.memory {
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                    // Compute the embedding OUTSIDE the memory lock so other
                    // memory operations are not blocked for the 10-50ms the
                    // embedding model takes. The lock is then only held for
                    // the lightweight I/O + index update.
                    let embedding = {
                        let m = mem.lock();
                        m.embedding_model()
                            .map(|model| model.embed_single(&text).unwrap_or_default())
                    };
                    let m = mem.lock();
                    let memory_session_id = self.session_id.as_deref().unwrap_or_else(|| m.session_id());
                    let _ = m.store_conversation_for_session_precomputed(
                        memory_session_id,
                        "assistant",
                        &text,
                        embedding.as_deref(),
                    );
                }
            }

            // Feed to reflection daemon (non-blocking, Deep mode only)
            if let Some(ref daemon) = self.brain.reflection_daemon {
                daemon.try_send("assistant", &text);
            }

            // Consolidate memory in background (non-blocking, best-effort).
            // Runs every 20 turns to amortize O(n²) cosine-similarity cost.
            // Skipped in Stateless mode (no memory to consolidate).
            // Clone the consolidator BEFORE acquiring the lock so the lock
            // is held only briefly; the heavy CPU work runs lock-free.
            if let Some(ref mem) = self.brain.memory {
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless
                    && turn_index % 20 == 0
                {
                    let mem = mem.clone();
                    self.join_set.spawn(async move {
                        let consolidator = {
                            let guard = mem.lock();
                            guard.consolidator_clone()
                        }; // lock released here — consolidate() runs without it
                        // Run O(n²) dedup on tokio's blocking thread pool
                        // so it doesn't tie up an async worker for seconds.
                        let result = tokio::task::spawn_blocking(move || {
                            consolidator.consolidate()
                        })
                        .await
                        .unwrap_or_else(|e| {
                            Err(anyhow::anyhow!("consolidation panicked: {e}"))
                        });
                        if let Ok(report) = result {
                            if report.deduped_recall > 0 || report.deduped_archival > 0 {
                                tracing::info!(
                                    deduped_recall = report.deduped_recall,
                                    deduped_archival = report.deduped_archival,
                                    "memory consolidated"
                                );
                            }
                        }
                    });
                }

                // Lifecycle: prune cold recall + promote to archival every 40 turns.
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless
                    && turn_index > 0
                    && turn_index % 40 == 0
                {
                    let mem = mem.clone();
                    self.join_set.spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            let guard = mem.lock();
                            guard.run_lifecycle()
                        })
                        .await
                        .unwrap_or_else(|e| Err(anyhow::anyhow!("lifecycle panicked: {e}")));
                        if let Ok(report) = result {
                            if report.pruned > 0 || report.promoted > 0 {
                                tracing::info!(
                                    pruned = report.pruned,
                                    promoted = report.promoted,
                                    "memory lifecycle completed"
                                );
                            }
                        }
                    });
                }
            }

            // Process steering messages — inject one per turn boundary
            // to avoid overwhelming the LLM with multiple instructions at once.
            if let Some(entry) = self.steering_queue.pop_front() {
                self.emit(RunEvent::SteerInjected {
                    steer_id: entry.id.clone(),
                    message: entry.raw_text.clone(),
                });
                self.context.add(entry.message);
                // Continue the loop with the steered message
                return Ok(TurnOutcome::Continue);
            }

            return Ok(TurnOutcome::Final(text));
        }

        // Add assistant message with tool calls
        let assistant_msg = Message::assistant_with_tools(&text, tool_calls.clone());
        self.context.add(assistant_msg.clone());
        self.save_session_snapshot();
        self.emit(RunEvent::MessageEnd {
            message_id: message_id.clone(),
            message: assistant_msg.clone(),
        });

        // Stage: Execute
        tracing::info!(
            tool_count = tool_calls.len(),
            tools = ?tool_calls.iter().map(|c| &c.function.name).collect::<Vec<_>>(),
            "TURN: executing tools"
        );
        // Clone the event sender and run id out of self so the bridge
        // closure doesn't borrow self (which would conflict with the
        // &mut borrows the orchestrator needs).
        // RAII tool guards: if execute_tools panics (or the task is aborted
        // mid-execution), the ToolEnded loop below is skipped, leaving the
        // frontend with orphaned spinning tool blocks. Each guard emits a
        // ToolEnded{is_error:true} on drop unless completed.
        let tool_call_ids: Vec<String> = tool_calls.iter().map(|c| c.id.clone()).collect();
        let mut tool_guards: Vec<EventGuard<()>> = Vec::new();
        for call_id in &tool_call_ids {
            let tx = self.event_tx.clone();
            let seq = self.seq.clone();
            let run_id = self.id.clone();
            let session_id = self.session_id.clone();
            let turn_id = self.current_turn_id.clone();
            let cid = call_id.clone();
            tool_guards.push(EventGuard::new(move || {
                let _ = tx.send(Envelope {
                    seq: seq.fetch_add(1, Ordering::Relaxed),
                    event_id: uuid::Uuid::new_v4().to_string(),
                    run_id: run_id.clone(),
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    parent_call_id: None,
                    ts: chrono::Utc::now(),
            event: RunEvent::ToolEnded {
                        subagent_id: None,
                        call_id: cid.clone(),
                        name: String::new(),
                        result: "Tool execution aborted (guard cleanup)".to_string(),
                        is_error: true,
                    },
                });
            }));
        }

        let event_tx = self.event_tx.clone();
        let run_id = self.id.clone();
        let seq = self.seq.clone();
        let session_id = self.session_id.clone();
        let turn_id = self.current_turn_id.clone();
        // Clone the approval resolver before constructing the orchestrator to
        // avoid borrowing conflicts with `&mut self.permission_policy` etc.
        // Using the per-Run resolver eliminates the actor deadlock that the
        // old global-map fallback path was vulnerable to.
        let approval_resolver = self.approval_resolver.clone();
        let input_resolver = self.input_resolver.clone();
        let tool_results = {
            let mut orchestrator = ToolOrchestrator {
                registry: &self.registry,
                permission_policy: &mut self.permission_policy,
                hook_registry: self.hook_registry.clone(),
                tool_execution_mode: self.tool_execution_mode,
                cancel_token: self.cancel.clone(),
                approval_resolver: Some(approval_resolver),
                input_resolver: Some(input_resolver),
                session_id: self.session_id.clone(),
                working_dir: self.working_dir.clone(),
            };
            orchestrator
                .execute_tools(&tool_calls, &move |ev, parent_call_id: &str| {
                    if let Some(run_ev) = RunEvent::from_agent_event(&run_id, ev) {
                        let _ = event_tx.send(Envelope {
                            seq: seq.fetch_add(1, Ordering::Relaxed),
                            event_id: uuid::Uuid::new_v4().to_string(),
                            run_id: run_id.clone(),
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            parent_call_id: Some(parent_call_id.to_string()),
                            ts: chrono::Utc::now(),
            event: run_ev,
                        });
                    }
                })
                .await
        };
        if self.cancel.is_cancelled() {
            return Err(RunError::Cancelled);
        }

        // Stage: Observe
        for (call, result) in tool_calls.iter().zip(&tool_results) {
            let is_error = result.starts_with("Error")
                || result.starts_with("Permission denied")
                || result.starts_with("Hook vetoed");

            self.emit(RunEvent::ToolEnded {
                subagent_id: None,
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                result: result.clone(),
                is_error,
            });

            self.context
                .add(Message::tool(call.id.clone(), result.clone(), Some(call.function.name.clone())));
        }
        self.save_session_snapshot();

        // All tools completed normally — disarm the guards.
        for g in tool_guards.iter_mut() {
            g.complete();
        }

        // If any todo tool was called, push the current todo snapshot to the frontend.
        let todo_changed = tool_calls
            .iter()
            .any(|c| matches!(c.function.name.as_str(), "todo_write" | "todo_update"));
        if todo_changed {
            let items: Vec<TodoItemPayload> = self
                .brain
                .todo_list
                .lock()
                .items
                .iter()
                .map(|item| TodoItemPayload {
                    id: item.id.clone(),
                    description: item.description.clone(),
                    status: item.status.to_string(),
                })
                .collect();
            self.emit(RunEvent::TodoUpdated { items });
        }

        self.emit(RunEvent::TurnEnded { index: turn_index });
        self.hook_registry.lock().fire_turn_end(turn_index);

        // Process steering messages (injected before next LLM call).
        // Inject one per turn boundary — remaining messages will be
        // processed on subsequent turn boundaries.
        if let Some(entry) = self.steering_queue.pop_front() {
            self.emit(RunEvent::SteerInjected {
                steer_id: entry.id.clone(),
                message: entry.raw_text.clone(),
            });
            self.context.add(entry.message);
        }

        if turn_index == self.max_iterations - 1 {
            let summary = super::build_iteration_limit_summary(&self.context, self.max_iterations);
            self.emit(RunEvent::Error {
                message: summary.clone(),
            });
            return Ok(TurnOutcome::Stop(summary));
        }

        Ok(TurnOutcome::Continue)
    }

    // ── Model interaction ────────────────────────────────────────

    pub(super) async fn model_turn(&mut self) -> Result<(String, Vec<ToolCall>, String, CacheUsage), String> {
        const MAX_RECOVERY_ATTEMPTS: u32 = 3;
        /// How many times we restart a dropped SSE stream before escalating to recovery.
        const MAX_STREAM_RETRIES: u32 = 10;

        for _attempt in 0..MAX_RECOVERY_ATTEMPTS {
            tracing::info!(attempt = _attempt, "TURN: model_turn recovery attempt");
            if self.cancel.is_cancelled() {
                return Err("aborted".to_string());
            }

            let mut base_messages = self.build_messages();
            let tools = self.registry.tool_definitions();

            // Apply hygiene: truncate oversized tool results, replace long args,
            // cap embedded thinking tags in historical assistant messages.
            crate::hygiene::sanitize(&mut base_messages);

            // KV cache hint: derived from the 7-segment context layout. Carried
            // into the request so the client can emit cache telemetry and
            // (for Anthropic) attach a cache breakpoint on the stable prefix.
            let cache_hint = {
                let h = self.context.cache_hint();
                ClientCacheHint {
                    stable_prefix_tokens: h.stable_prefix_tokens,
                    cacheable_prefix_tokens: h.cacheable_prefix_tokens,
                    can_reuse_cache: h.can_reuse_cache,
                    strategy: h.strategy,
                    last_turn_elapsed_ms: h.last_turn_elapsed_ms,
                    expected_cold_miss: h.expected_cold_miss,
                }
            };

            // BeforeModel hook: SkipModel short-circuit
            let snapshot = self.snapshot_messages_for_hook(&base_messages);
            if let Some(preset) = self.hook_registry.lock().fire_before_model(&snapshot) {
                self.recovery_ctx.record_success();
                self.hook_registry.lock().fire_after_model(&preset, 0);
                return Ok((preset, Vec::new(), uuid::Uuid::new_v4().to_string(), CacheUsage::default()));
            }

            // ── Inner stream-retry loop ──────────────────────────────
            // The outer `send_with_retry` handles HTTP-level failures
            // (429, 5xx, connection refused). This inner loop handles
            // *mid-stream* drops — the HTTP connection succeeded, the
            // SSE stream started delivering tokens, then the connection
            // reset / the proxy timed out / the gateway dropped us.
            // On retry we inject truncated partial thinking/text so the
            // model can continue without redoing completed work.
            let mut stream_attempt = 0;
            let mut retry_checkpoint = StreamPartial::default();

            let stream_result = 'stream_loop: loop {
                if self.cancel.is_cancelled() {
                    return Err("aborted".to_string());
                }

                let mut attempt_messages = base_messages.clone();
                crate::hygiene::inject_stream_retry_hint(
                    &mut attempt_messages,
                    &retry_checkpoint.thinking,
                    &retry_checkpoint.text,
                );

                let mut attempt_partial = StreamPartial::default();

                let step_res = {
                    let stream_res = self
                        .client
                        .chat_completion_stream_with_hint(&attempt_messages, &tools, Some(cache_hint))
                        .await;

                    match stream_res {
                        Ok(s) => {
                            let cancel = self.cancel.clone();
                            let event_tx = self.event_tx.clone();
                            let res = self
                                .collect_stream(s, &event_tx, &mut attempt_partial)
                                .await;
                            match res {
                                Ok(r) => {
                                    if r.0.is_empty() && r.1.is_empty() {
                                        Err("empty response from model — SSE stream had no useful events".to_string())
                                    } else {
                                        Ok(r)
                                    }
                                }
                                Err(e) if cancel.is_cancelled() => return Err("aborted".to_string()),
                                Err(e) => Err(format!("Stream error: {e}")),
                            }
                        }
                        Err(e) => Err(e.to_string()),
                    }
                };

                match step_res {
                    Ok(r) => {
                        break 'stream_loop Ok(r);
                    }
                    Err(err_msg) => {
                        retry_checkpoint.merge_attempt(&attempt_partial);
                        tracing::warn!(attempt = stream_attempt, error = %err_msg, "stream attempt failed");
                        if stream_attempt < MAX_STREAM_RETRIES {
                            let delay_ms = 1000u64 * 2u64.pow(stream_attempt as u32);
                            self.emit(RunEvent::Error {
                                message: format!(
                                    "LLM stream request failed, retrying in {}s (attempt {}/{})",
                                    delay_ms / 1000,
                                    stream_attempt + 1,
                                    MAX_STREAM_RETRIES,
                                ),
                            });
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            stream_attempt += 1;
                            continue;
                        }

                        break 'stream_loop Err(err_msg);
                    }
                }
            };

            let r = match stream_result {
                Ok(res_val) => res_val,
                Err(msg) => {
                    self.recovery_ctx.record_error(&msg);
                    match self.try_recover(&msg).await {
                        RecoveryOutcome::Retry => {
                            tracing::info!("recovery engine retrying after stream errors");
                            continue;
                        }
                        RecoveryOutcome::GiveUp => return Err(msg),
                    }
                }
            };

            self.recovery_ctx.record_success();
            self.hook_registry.lock().fire_after_model(&r.0, r.1.len());
            return Ok(r);
        }

        Err("exhausted recovery attempts".to_string())
    }

    pub(super) async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_tx: &broadcast::Sender<Envelope>,
        partial: &mut StreamPartial,
    ) -> Result<(String, Vec<ToolCall>, String, CacheUsage)> {
        tracing::debug!("TURN: collect_stream start");
        let mut text_buffer = String::new();
        let mut thinking_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;
        let mut cache_usage = CacheUsage::default();
        // Token accumulator: batches text/thinking deltas to cut IPC traffic.
        let mut tokens = TokenAccumulator::new();
        // Stable id for this model response; carried on every streaming delta
        // so the frontend routes by identity instead of position.
        let message_id = uuid::Uuid::new_v4().to_string();

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            if self.cancel.is_cancelled() {
                anyhow::bail!("aborted");
            }
            let event = event?;
            match event {
                StreamEvent::TextDelta(delta) => {
                    tokens.push_text(&delta);
                    text_buffer.push_str(&delta);
                    partial.text.push_str(&delta);
                    if tokens.should_flush() {
                        if let Some((text, thinking)) = tokens.flush() {
                            if !text.is_empty() {
                                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                                    subagent_id: None,
                                    message_id: message_id.clone(),
                                    delta: MessageDelta::Text(text),
                                }));
                            }
                            if !thinking.is_empty() {
                                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                                    subagent_id: None,
                                    message_id: message_id.clone(),
                                    delta: MessageDelta::Thinking(thinking),
                                }));
                            }
                        }
                    }
                }
                StreamEvent::ThinkingDelta(delta) => {
                    tokens.push_thinking(&delta);
                    thinking_buffer.push_str(&delta);
                    partial.thinking.push_str(&delta);
                    if tokens.should_flush() {
                        if let Some((text, thinking)) = tokens.flush() {
                            if !text.is_empty() {
                                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                                    subagent_id: None,
                                    message_id: message_id.clone(),
                                    delta: MessageDelta::Text(text),
                                }));
                            }
                            if !thinking.is_empty() {
                                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                                    subagent_id: None,
                                    message_id: message_id.clone(),
                                    delta: MessageDelta::Thinking(thinking),
                                }));
                            }
                        }
                    }
                }
                StreamEvent::ToolCallDelta { .. } => {
                    has_tool_calls = true;
                    accumulator.push(event);
                }
                StreamEvent::Done => break,
                StreamEvent::CompleteWithUsage { prompt_cache_hit_tokens, prompt_cache_miss_tokens } => {
                    cache_usage = CacheUsage {
                        hit_tokens: prompt_cache_hit_tokens.unwrap_or(0),
                        miss_tokens: prompt_cache_miss_tokens.unwrap_or(0),
                    };
                    break;
                }
            }
        }

        // Final flush: emit any remaining buffered text/thinking.
        if let Some((text, thinking)) = tokens.force_flush() {
            if !text.is_empty() {
                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                    subagent_id: None,
                    message_id: message_id.clone(),
                    delta: MessageDelta::Text(text),
                }));
            }
            if !thinking.is_empty() {
                let _ = event_tx.send(self.wrap(RunEvent::ModelStreaming {
                    subagent_id: None,
                    message_id: message_id.clone(),
                    delta: MessageDelta::Thinking(thinking),
                }));
            }
        }

        let tool_calls = if has_tool_calls {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                accumulator.into_tool_calls()
            })) {
                Ok(calls) => calls,
                Err(_) => {
                    tracing::error!("TURN: tool_calls accumulator panicked");
                    return Err(anyhow::anyhow!("tool call accumulator panicked — incomplete SSE stream"));
                }
            }
        } else {
            vec![]
        };

        tracing::debug!(
            text_len = text_buffer.len(),
            tool_count = tool_calls.len(),
            "TURN: collect_stream done"
        );

        Ok((text_buffer, tool_calls, message_id, cache_usage))
    }

    /// Queue a best-effort mid-turn context snapshot write on the blocking
    /// pool. Serialization + disk I/O never stall the turn loop (e.g. before
    /// emitting `ApprovalRequired`). A generation counter drops superseded
    /// in-flight writes so an older snapshot cannot overwrite a newer one.
    pub(super) fn save_session_snapshot(&mut self) {
        let Some(path) = self.session_snapshot_path.clone() else {
            return;
        };
        let messages = self.context.messages();
        let generation = self
            .session_snapshot_gen
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let gen_arc = self.session_snapshot_gen.clone();

        self.join_set.spawn(async move {
            let write_result = tokio::task::spawn_blocking(move || {
                // Another save was queued after us — skip this write.
                if gen_arc.load(Ordering::Relaxed) != generation {
                    return Ok(());
                }
                let json = serde_json::to_string(&messages).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                // Re-check after serialize: a newer save may have started.
                if gen_arc.load(Ordering::Relaxed) != generation {
                    return Ok(());
                }
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let tmp_path = path.with_extension("tmp.snapshot");
                match std::fs::write(&tmp_path, &json).and_then(|_| std::fs::rename(&tmp_path, &path))
                {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp_path);
                        Err(e)
                    }
                }
            })
            .await;

            match write_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "failed to write session snapshot");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "session snapshot task panicked or cancelled");
                }
            }
        });
    }
}

fn format_user_friendly_error(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("503") || lower.contains("service unavailable") || lower.contains("502") || lower.contains("bad gateway") || lower.contains("504") || lower.contains("gateway timeout") {
        "The AI model service is temporarily unavailable or overloaded (returned a server 503/502 error). I tried to connect several times but it is still not responding. Please try again in a minute; this is usually a brief issue on the provider's side.".to_string()
    } else if lower.contains("429") || lower.contains("rate limit") {
        "The AI model service is currently rate-limiting requests (HTTP 429). I retried several times but it is still busy — please wait a moment and try again.".to_string()
    } else {
        err.to_string()
    }
}
