//! Turn execution — model interaction, streaming collection, and turn dispatch.

use anyhow::Result;
use futures::StreamExt;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::runtime::tool_orchestrator::ToolOrchestrator;
use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
use crate::client::ClientCacheHint;
use crate::runtime::event::{Envelope, RunEvent, TodoItemPayload};
use crate::runtime::guard::EventGuard;
use crate::types::{CacheUsage, Message, MessageDelta, ReasoningState, StreamEvent, ToolCall};

use super::{RecoveryOutcome, Run, RunError, TurnOutcome, CACHE_IDLE_WARN_SECS};

/// Gap between consecutive SSE events that warrants a live warn (ms).
const LATENCY_GAP_WARN_MS: u64 = 2_000;

/// Result of one successful model stream collection.
pub(super) struct ModelTurnResult {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub message_id: String,
    pub cache_usage: CacheUsage,
    /// Opaque provider blobs collected during the stream (encrypted_content / signature).
    pub reasoning_blob: ReasoningState,
}

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
        let model_turn_started = Instant::now();
        let ModelTurnResult {
            text,
            thinking,
            tool_calls,
            message_id,
            cache_usage,
            reasoning_blob,
        } = match self.model_turn().await {
            Ok(r) => {
                tracing::info!(
                    text_len = r.text.len(),
                    thinking_len = r.thinking.len(),
                    tool_count = r.tool_calls.len(),
                    elapsed_ms = model_turn_started.elapsed().as_millis() as u64,
                    "TURN: model_turn ok"
                );
                r
            }
            Err(e) => {
                if self.cancel.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                tracing::warn!(
                    error = %e,
                    elapsed_ms = model_turn_started.elapsed().as_millis() as u64,
                    "TURN: model_turn failed"
                );
                let friendly_err = format_user_friendly_error(&e);
                return Err(RunError::Failed(friendly_err));
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
            // Final answer — persist thinking for resume; memory uses visible text only.
            let content = crate::hygiene::wrap_thinking(&thinking, &text);
            let mut assistant_msg = Message::assistant(&content);
            let mut reasoning = reasoning_blob;
            if !thinking.trim().is_empty() && reasoning.text.is_none() {
                reasoning.text = Some(thinking.trim().to_string());
            }
            if !reasoning.is_empty() {
                assistant_msg = assistant_msg.with_reasoning(reasoning);
            }
            self.append_conversation(assistant_msg.clone());
            self.save_session_snapshot();
            self.emit(RunEvent::MessageEnd {
                message_id: message_id.clone(),
                message: assistant_msg.clone(),
            });
            self.emit(RunEvent::TurnEnded { index: turn_index });
            self.hook_registry.lock().fire_turn_end(turn_index);

            // Store in memory
            let mut reflected_session_id = self.session_id.clone();
            if let Some(ref mem) = self.brain.memory {
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                    // Compute the embedding OUTSIDE the memory lock so other
                    // memory operations are not blocked for the 10-50ms the
                    // embedding model takes. The lock is then only held for
                    // the lightweight I/O + index update.
                    let model = { mem.lock().embedding_model().cloned() };
                    let embedding = model
                        .map(|model| model.embed_single(&text).unwrap_or_default());
                    let m = mem.lock();
                    let memory_session_id = self.session_id.as_deref().unwrap_or_else(|| m.session_id());
                    reflected_session_id = Some(memory_session_id.to_string());
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
                if let Some(session_id) = reflected_session_id.as_deref() {
                    daemon.notify_session(session_id);
                }
            }

            // Consolidate memory in background (non-blocking, best-effort).
            // Runs every 20 turns to amortize O(n²) cosine-similarity cost.
            // Skipped in Stateless mode (no memory to consolidate).
            // Clone the consolidator BEFORE acquiring the lock so the lock
            // is held only briefly; the heavy CPU work runs lock-free.
            if let Some(ref mem) = self.brain.memory {
                let completed_turn = mem.lock().record_completed_turn();
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless
                    && completed_turn % 20 == 0
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
                    && completed_turn % 40 == 0
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
            // Poll cmd_rx first so mid-turn steers are not deferred an extra turn.
            if self.inject_next_steer()? {
                return Ok(TurnOutcome::Continue);
            }

            return Ok(TurnOutcome::Final(text));
        }

        // Add assistant message with tool calls — include thinking for ReAct continuity.
        let post_model_t0 = Instant::now();
        let content = crate::hygiene::wrap_thinking(&thinking, &text);
        let mut assistant_msg = Message::assistant_with_tools(&content, tool_calls.clone());
        let mut reasoning = reasoning_blob;
        if !thinking.trim().is_empty() && reasoning.text.is_none() {
            reasoning.text = Some(thinking.trim().to_string());
        }
        if !reasoning.is_empty() {
            assistant_msg = assistant_msg.with_reasoning(reasoning);
        }
        self.append_conversation(assistant_msg.clone());
        let after_context_ms = post_model_t0.elapsed().as_millis() as u64;
        self.save_session_snapshot();
        let after_snapshot_ms = post_model_t0.elapsed().as_millis() as u64;
        self.emit(RunEvent::MessageEnd {
            message_id: message_id.clone(),
            message: assistant_msg.clone(),
        });
        tracing::info!(
            tool_count = tool_calls.len(),
            tools = ?tool_calls.iter().map(|c| &c.function.name).collect::<Vec<_>>(),
            context_ms = after_context_ms,
            snapshot_queue_ms = after_snapshot_ms.saturating_sub(after_context_ms),
            post_model_ms = post_model_t0.elapsed().as_millis() as u64,
            "LATENCY: pre-tool handoff (model done → execute)"
        );

        // Stage: Execute
        let execute_started = Instant::now();
        // info: turn-level wall clock; pair with execute_tools done.
        tracing::info!(
            tool_count = tool_calls.len(),
            tools = ?tool_calls.iter().map(|c| &c.function.name).collect::<Vec<_>>(),
            "LATENCY: execute_tools begin"
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
                run_id: Some(self.id.clone()),
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
        tracing::info!(
            tool_count = tool_calls.len(),
            elapsed_ms = execute_started.elapsed().as_millis() as u64,
            "LATENCY: execute_tools done"
        );
        if self.cancel.is_cancelled() {
            return Err(RunError::Cancelled);
        }

        // Stage: Observe
        let mut saw_abort = false;
        let mut todo_write_failed = false;
        let mut todo_write_ok = false;
        let mut todo_write_forced = false;

        for (call, result) in tool_calls.iter().zip(&tool_results) {
            let is_error = crate::runtime::execution::tool_result_is_error(result);
            let aborted = result.starts_with("Aborted")
                || result.contains("aborted (guard cleanup)");

            if aborted {
                saw_abort = true;
            }

            if call.function.name == "todo_write" {
                if is_error {
                    todo_write_failed = true;
                } else {
                    todo_write_ok = true;
                    todo_write_forced = call.function.arguments.contains("\"force\":true")
                        || call.function.arguments.contains("\"force\": true");
                }
            }

            if let Some(fact) = crate::runtime::execution::artifact_from_tool(
                &call.function.name,
                &call.function.arguments,
                result,
                is_error || aborted,
            ) {
                self.execution.record_artifact(fact);
            }

            self.emit(RunEvent::ToolEnded {
                subagent_id: None,
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                result: result.clone(),
                is_error: is_error || aborted,
            });

            self.append_conversation(Message::tool(
                call.id.clone(),
                result.clone(),
                Some(call.function.name.clone()),
            ));
        }
        self.save_session_snapshot();

        // Sync execution phase from todos after tool batch.
        {
            let todos = self.session_todos();
            let mut list = todos.lock();
            if todo_write_ok {
                self.execution.on_plan_written(&list, todo_write_forced);
                let _ = list.ensure_active_step();
            } else {
                self.execution.sync_from_todos(&list);
            }

            // All steps done → Verify (then Done on next successful Final).
            if !list.items.is_empty()
                && list
                    .items
                    .iter()
                    .all(|i| i.status == crate::todo::TodoStatus::Completed)
            {
                use crate::runtime::execution::ExecutionPhase;
                if self.execution.phase == ExecutionPhase::Execute {
                    self.execution.phase = ExecutionPhase::Verify;
                }
            }

            if saw_abort || todo_write_failed {
                let step = self
                    .execution
                    .active_step_id
                    .clone()
                    .or_else(|| list.ensure_active_step())
                    .unwrap_or_else(|| "?".into());
                let reason = if saw_abort {
                    "The previous tool batch did not finish"
                } else {
                    "todo_write failed"
                };
                self.execution.set_resume_hint(format!(
                    "{reason}. Plan unchanged (v{}). Continue step {step} with tools; \
                     do NOT replan unless force=true is required.",
                    self.execution.plan_version
                ));
            }
        }

        // All tools completed normally — disarm the guards.
        for g in tool_guards.iter_mut() {
            g.complete();
        }

        // If any todo tool was called, push the current todo snapshot to the frontend.
        let todo_changed = tool_calls
            .iter()
            .any(|c| matches!(c.function.name.as_str(), "todo_write" | "todo_update"));
        if todo_changed {
            let list = self.session_todos();
            let items: Vec<TodoItemPayload> = list
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
        // processed on subsequent turn boundaries. Poll cmd_rx first so
        // mid-turn steers land before the next model request.
        self.inject_next_steer()?;

        if turn_index == self.max_iterations - 1 {
            let summary = super::build_iteration_limit_summary(&self.context, self.max_iterations);
            return Err(RunError::Failed(summary));
        }

        Ok(TurnOutcome::Continue)
    }

    // ── Model interaction ────────────────────────────────────────

    pub(super) async fn model_turn(&mut self) -> Result<ModelTurnResult, String> {
        const MAX_RECOVERY_ATTEMPTS: u32 = 3;
        /// How many times we restart a dropped SSE stream before escalating to recovery.
        const MAX_STREAM_RETRIES: u32 = 5;
        const MAX_RETRY_DELAY_MS: u64 = 30_000;

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
                return Ok(ModelTurnResult {
                    text: preset,
                    thinking: String::new(),
                    tool_calls: Vec::new(),
                    message_id: uuid::Uuid::new_v4().to_string(),
                    cache_usage: CacheUsage::default(),
                    reasoning_blob: ReasoningState::default(),
                });
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
                    let stream_res = tokio::select! {
                        _ = self.cancel.cancelled() => return Err("aborted".to_string()),
                        result = self.client.chat_completion_stream_with_hint(
                            &attempt_messages,
                            &tools,
                            Some(cache_hint),
                        ) => result,
                    };

                    match stream_res {
                        Ok(s) => {
                            let cancel = self.cancel.clone();
                            let event_tx = self.event_tx.clone();
                            // Connection is up again. Thinking models can sit
                            // silent for a long time before the first token —
                            // clear the retry banner immediately, don't wait
                            // for model_streaming deltas.
                            // Use wrap+send (&self) rather than emit (&mut self)
                            // so we don't conflict with the stream borrow.
                            let _ = event_tx.send(self.wrap(RunEvent::ModelCallStarted));
                            let res = self
                                .collect_stream(s, &event_tx, &mut attempt_partial)
                                .await;
                            match res {
                                Ok(r) => {
                                    if r.text.is_empty() && r.tool_calls.is_empty() {
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
                            let delay_ms = (1000u64 * 2u64.pow(stream_attempt))
                                .min(MAX_RETRY_DELAY_MS);
                            self.emit(RunEvent::Notice {
                                code: "model_stream_retry".to_string(),
                                severity: "warning".to_string(),
                                recoverable: true,
                                message: format!(
                                    "Failed to connect to remote model (stream failed), retrying in {}s (attempt {}/{})",
                                    delay_ms / 1000,
                                    stream_attempt + 1,
                                    MAX_STREAM_RETRIES,
                                ),
                            });
                            tokio::select! {
                                _ = self.cancel.cancelled() => return Err("aborted".to_string()),
                                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                            }
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
            self.hook_registry.lock().fire_after_model(&r.text, r.tool_calls.len());
            return Ok(r);
        }

        Err("exhausted recovery attempts".to_string())
    }

    pub(super) async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_tx: &tokio::sync::mpsc::UnboundedSender<Envelope>,
        partial: &mut StreamPartial,
    ) -> Result<ModelTurnResult> {
        tracing::debug!("LATENCY: collect_stream start");
        let stream_t0 = Instant::now();
        let mut text_buffer = String::new();
        let mut thinking_buffer = String::new();
        let mut reasoning_blob = ReasoningState::default();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;
        let mut cache_usage = CacheUsage::default();
        // Token accumulator: batches text/thinking deltas to cut IPC traffic.
        let mut tokens = TokenAccumulator::new();
        // Stable id for this model response; carried on every streaming delta
        // so the frontend routes by identity instead of position.
        let message_id = uuid::Uuid::new_v4().to_string();

        // ── Latency milestones (ms since stream_t0) ─────────────────
        // Level policy (keep forever; tune RUST_LOG in prod):
        //   info  — one summary per stream + first tool_call (smoking gun)
        //   warn  — inter-event gaps ≥ LATENCY_GAP_WARN_MS
        //   debug — per-phase first-* breadcrumbs (TTFE / thinking / text / preparing)
        // Pinpoints: TTFT / first thinking / last thinking / first tool / stream done.
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

        // Classify + stamp a stream event for latency tracking.
        let stamp = |kind: &'static str,
                     now: Instant,
                     last_at: &mut Instant,
                     last_kind: &mut &'static str,
                     max_gap: &mut u64,
                     max_from: &mut &'static str,
                     max_to: &mut &'static str| {
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

        const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
        tokio::pin!(stream);
        loop {
            let event = tokio::select! {
                _ = self.cancel.cancelled() => anyhow::bail!("aborted"),
                result = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()) => {
                    match result {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(_) => anyhow::bail!("model stream idle timeout after {}s", STREAM_IDLE_TIMEOUT.as_secs()),
                    }
                }
            };
            let event = event?;
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
                            // Anthropic may stream signature in chunks; append.
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
                        let gap_after_text = last_text_ms
                            .map(|t| since.saturating_sub(t))
                            .unwrap_or(0);
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
                        let _ = event_tx.send(self.wrap(RunEvent::ToolPreparing {
                            index: notify.index,
                            call_id: notify.call_id,
                            name: notify.name,
                            hint_path: notify.hint_path,
                        }));
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
                StreamEvent::CompleteWithUsage { prompt_cache_hit_tokens, prompt_cache_miss_tokens } => {
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

        Ok(ModelTurnResult {
            text: text_buffer,
            thinking: thinking_buffer,
            tool_calls,
            message_id,
            cache_usage,
            reasoning_blob,
        })
    }

    /// Queue a best-effort mid-turn context snapshot write on the blocking
    /// pool. Serialization + disk I/O never stall the turn loop (e.g. before
    /// emitting `ApprovalRequired`). A generation counter drops superseded
    /// in-flight writes so an older snapshot cannot overwrite a newer one.
    pub(super) fn save_session_snapshot(&mut self) {
        let Some(path) = self.session_snapshot_path.clone() else {
            return;
        };
        let messages = crate::session::messages_for_snapshot(self.full_transcript());
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
