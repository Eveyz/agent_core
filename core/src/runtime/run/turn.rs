//! Turn execution — model interaction, streaming collection, and turn dispatch.

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::client::ClientCacheHint;
use crate::runtime::agent_loop::{
    CollectedStream, LoopPolicy, ModelCall, StreamCallbacks, StreamPartial,
};
use crate::runtime::event::{CacheStatus, Envelope, RunEvent};
use crate::runtime::guard::EventGuard;
use crate::runtime::tool_orchestrator::ToolOrchestrator;
use crate::types::{Message, MessageDelta};

use super::{CACHE_IDLE_WARN_SECS, ModelTurnFailure, RecoveryOutcome, Run, RunError, TurnOutcome};

impl Run {
    pub(super) async fn run_turn(&mut self, turn_index: usize) -> Result<TurnOutcome, RunError> {
        tracing::info!(
            turn = turn_index,
            context_msgs = self.context.len(),
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
                    status: CacheStatus::IdleExpired,
                });
            }
        }

        // Stage: Refresh
        self.refresh_context_segments();
        // Segments changed — publish usage so the ring matches the context the
        // model is about to see (before maybe_compact). Conversation messages
        // themselves are unchanged, so the model-window snapshot stays valid.
        self.refresh_context_usage_snapshot();

        // Stage: Verify stable prefix hasn't drifted
        let current_fp = self.context.stable_prefix_fingerprint();
        if current_fp != self.last_prefix_fingerprint {
            self.emit(RunEvent::CacheInfo {
                hit_tokens: 0,
                miss_tokens: 0,
                status: CacheStatus::PrefixDrifted,
            });
            self.last_prefix_fingerprint = current_fp;
        }

        // Stage: Compact (on-demand only — avoid per-turn cache invalidation)
        self.maybe_compact().await;

        // Stage: Model
        tracing::info!("TURN: calling model_turn");
        let model_turn_started = Instant::now();
        let CollectedStream {
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
            Err(ModelTurnFailure::Cancelled) => return Err(RunError::Cancelled),
            Err(ModelTurnFailure::Interrupted(partial)) => {
                let message_id = partial
                    .message_id
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let partial_message = if partial.text.trim().is_empty()
                    && partial.thinking.trim().is_empty()
                {
                    None
                } else {
                    let content =
                        crate::hygiene::wrap_thinking(&partial.thinking, partial.text.trim_end());
                    Some(Message::assistant(&content))
                };
                if let Some(message) = partial_message.as_ref() {
                    let visible = message.content.as_deref().unwrap_or_default();
                    self.append_conversation(Message::assistant(&format!(
                        "{visible}\n\n[Interrupted by user steer]"
                    )));
                    self.save_session_snapshot();
                }
                self.emit(RunEvent::MessageInterrupted {
                    message_id,
                    reason: "user_steer".to_string(),
                    partial_message,
                });
                self.emit(RunEvent::TurnEnded { index: turn_index });
                self.hook_registry.lock().fire_turn_end(turn_index);
                self.inject_next_steer()?;
                return Ok(TurnOutcome::Continue);
            }
            Err(ModelTurnFailure::Failed(e)) => {
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
            self.cache_metrics
                .record(cache_usage.hit_tokens, cache_usage.miss_tokens);
            self.emit(RunEvent::CacheInfo {
                hit_tokens: cache_usage.hit_tokens,
                miss_tokens: cache_usage.miss_tokens,
                status: CacheStatus::Rate {
                    hit_rate: cache_usage.hit_rate(),
                },
            });
        }

        // Stage: Dispatch
        if tool_calls.is_empty() {
            if let Some(required_tool) = self.required_tool.clone() {
                tracing::warn!(
                    %required_tool,
                    "provider returned text without the runtime-required tool call"
                );
                let content = crate::hygiene::wrap_thinking(&thinking, &text);
                self.append_conversation(Message::assistant(&content));
                self.append_conversation(Message::system(&format!(
                    "[Runtime requirement] You cannot finish this Run yet. Call the \
                     `{required_tool}` tool now. This tool is required by the scoped \
                     feature that started the Run; do not answer on its behalf."
                )));
                self.save_session_snapshot();
                self.emit(RunEvent::MessageEnd {
                    message_id: message_id.clone(),
                    message: Message::assistant(&content),
                });
                self.emit(RunEvent::TurnEnded { index: turn_index });
                return Ok(TurnOutcome::Continue);
            }

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
                    let embedding =
                        model.map(|model| model.embed_single(&text).unwrap_or_default());
                    let m = mem.lock();
                    let memory_session_id =
                        self.session_id.as_deref().unwrap_or_else(|| m.session_id());
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
                        let result =
                            tokio::task::spawn_blocking(move || consolidator.consolidate())
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

            // Process all steering messages accepted before this boundary.
            // Poll cmd_rx first so legacy command-channel steers are not
            // deferred an extra turn.
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
            let mut orchestrator = ToolOrchestrator::new(
                &self.registry,
                &mut self.permission_policy,
                self.hook_registry.clone(),
                self.tool_execution_mode,
                self.steering.turn_token(),
                Some(self.cancel.clone()),
                Some(approval_resolver),
                Some(input_resolver),
                self.session_id.clone(),
                self.prompt_id.clone(),
                Some(self.id.clone()),
                self.working_dir.clone(),
            );
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
        if self.steering.turn_token().is_cancelled() {
            // Dropping a supervised shell/repl future is not enough to reap
            // the child process; kill the active turn's processes before the
            // next model request starts.
            self.supervisor.lock().kill_all();
        }

        // Stage: Observe
        let mut saw_abort = false;
        let mut todo_write_failed = false;
        let mut todo_write_ok = false;
        let mut todo_write_forced = false;

        let mut required_tool_succeeded = false;
        for (call, result) in tool_calls.iter().zip(&tool_results) {
            let is_error = crate::runtime::execution::tool_result_is_error(result);
            let aborted = result.starts_with("Aborted")
                || result.starts_with("Interrupted by user steer")
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

            if !is_error && !aborted {
                self.file_ledger
                    .observe_tool(&call.function.name, &call.function.arguments);
                if self.required_tool.as_deref() == Some(call.function.name.as_str()) {
                    required_tool_succeeded = true;
                }
            }

            self.emit(RunEvent::ToolEnded {
                subagent_id: None,
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                result: result.clone(),
                is_error: is_error || aborted,
            });

            // PLAN-0016: spill oversized incidental output, store truncated + path
            // so resume does not re-inflate multi-MB shell logs into the model window.
            // Live UI already received the full body via ToolEnded above.
            let spill_path = crate::paths::tool_spill_path(self.session_id.as_deref(), &call.id);
            let stored = crate::hygiene::prepare_tool_result_for_storage(
                Some(call.function.name.as_str()),
                result,
                &spill_path,
            );
            self.append_conversation(Message::tool(
                call.id.clone(),
                stored,
                Some(call.function.name.clone()),
            ));
        }
        if required_tool_succeeded {
            self.required_tool = None;
        }
        self.save_session_snapshot();

        // Sync execution phase from todos after tool batch.
        {
            let list = self
                .brain
                .todo_lists
                .active_list(self.session_id.as_deref());
            if todo_write_ok {
                self.execution.on_plan_written(&list, todo_write_forced);
            } else {
                self.execution.sync_from_todos(&list);
            }

            // All steps done → Verify (then Done on next successful Final).
            if list.all_completed() {
                use crate::runtime::execution::ExecutionPhase;
                if self.execution.phase == ExecutionPhase::Execute {
                    self.execution.phase = ExecutionPhase::Verify;
                }
                let _ = self
                    .brain
                    .todo_lists
                    .finish_active_if_done(self.session_id.as_deref());
            }

            if saw_abort || todo_write_failed {
                let step = self
                    .execution
                    .active_step_id
                    .clone()
                    .or_else(|| {
                        self.brain
                            .todo_lists
                            .with_active_mut(self.session_id.as_deref(), |l| l.ensure_active_step())
                            .flatten()
                    })
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

        // If any todo tool was called, push the current plans snapshot to the frontend.
        let todo_changed = tool_calls
            .iter()
            .any(|c| matches!(c.function.name.as_str(), "todo_write" | "todo_update"));
        if todo_changed {
            self.emit_plans_updated();
        }

        self.emit(RunEvent::TurnEnded { index: turn_index });
        self.hook_registry.lock().fire_turn_end(turn_index);

        // Process all steering messages accepted before the next model call.
        // Poll cmd_rx first so legacy command-channel steers land before the
        // next model request.
        self.inject_next_steer()?;

        if turn_index == self.max_iterations - 1 {
            let summary = super::build_iteration_limit_summary(&self.context, self.max_iterations);
            return Err(RunError::Failed(summary));
        }

        Ok(TurnOutcome::Continue)
    }

    // ── Model interaction ────────────────────────────────────────

    pub(super) async fn model_turn(&mut self) -> Result<CollectedStream, ModelTurnFailure> {
        const MAX_RECOVERY_ATTEMPTS: u32 = 3;

        let turn_cancel = self.steering.turn_token();
        for _attempt in 0..MAX_RECOVERY_ATTEMPTS {
            tracing::info!(attempt = _attempt, "TURN: model_turn recovery attempt");
            if self.cancel.is_cancelled() {
                return Err(ModelTurnFailure::Cancelled);
            }
            if turn_cancel.is_cancelled() {
                return Err(ModelTurnFailure::Interrupted(StreamPartial::default()));
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

            // BeforeModel hook: SkipModel short-circuit. The default registry
            // is empty, so avoid constructing a JSON preview of the complete
            // history unless a hook can actually consume it.
            let preset = {
                let hooks = self.hook_registry.lock();
                if hooks.is_empty() {
                    None
                } else {
                    let snapshot = self.snapshot_messages_for_hook(&base_messages);
                    hooks.fire_before_model(&snapshot)
                }
            };
            if let Some(preset) = preset {
                self.recovery_ctx.record_success();
                self.hook_registry.lock().fire_after_model(&preset, 0);
                return Ok(CollectedStream {
                    text: preset,
                    thinking: String::new(),
                    tool_calls: Vec::new(),
                    message_id: uuid::Uuid::new_v4().to_string(),
                    cache_usage: crate::types::CacheUsage::default(),
                    reasoning_blob: crate::types::ReasoningState::default(),
                });
            }

            let event_tx = self.event_tx.clone();
            let seq = self.seq.clone();
            let run_id = self.id.clone();
            let session_id = self.session_id.clone();
            let turn_id = self.current_turn_id.clone();
            let emit: Arc<dyn Fn(RunEvent) + Send + Sync> = Arc::new(move |event| {
                let _ = event_tx.send(Envelope {
                    seq: seq.fetch_add(1, Ordering::Relaxed),
                    event_id: uuid::Uuid::new_v4().to_string(),
                    run_id: run_id.clone(),
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    parent_call_id: None,
                    ts: chrono::Utc::now(),
                    event,
                });
            });
            let emit_delta = emit.clone();
            let emit_prep = emit.clone();
            let emit_retry = emit.clone();
            let emit_open = emit.clone();
            let checkpoint = parking_lot::Mutex::new(StreamPartial::default());
            let on_delta = move |mid: &str, delta: MessageDelta| {
                emit_delta(RunEvent::ModelStreaming {
                    subagent_id: None,
                    message_id: mid.to_string(),
                    delta,
                });
            };
            let on_prep = move |notify: crate::client::streaming::ToolPreparingNotify| {
                emit_prep(RunEvent::ToolPreparing {
                    index: notify.index,
                    call_id: notify.call_id,
                    name: notify.name,
                    hint_path: notify.hint_path,
                });
            };
            let on_partial = |partial: &StreamPartial| {
                *checkpoint.lock() = partial.clone();
            };
            let on_retry = move |attempt: u32, delay_ms: u64, _err: &str| {
                emit_retry(RunEvent::Notice {
                    code: "model_stream_retry".to_string(),
                    severity: "warning".to_string(),
                    recoverable: true,
                    message: format!(
                        "Failed to connect to remote model (stream failed), retrying in {}s (attempt {}/{})",
                        delay_ms / 1000,
                        attempt + 1,
                        LoopPolicy::interactive()
                            .max_stream_attempts()
                            .saturating_sub(1),
                    ),
                });
            };
            let on_open = move || {
                emit_open(RunEvent::ModelCallStarted);
            };

            let policy = LoopPolicy::interactive();
            let result = crate::runtime::agent_loop::run_model_phase(ModelCall {
                client: &self.client,
                policy,
                messages: base_messages,
                tools: &tools,
                cache_hint: Some(cache_hint),
                required_tool: self.required_tool.as_deref(),
                lifetime_cancel: Some(self.cancel.clone()),
                turn_cancel: Some(turn_cancel.clone()),
                callbacks: StreamCallbacks {
                    on_delta: &on_delta,
                    on_tool_preparing: Some(&on_prep),
                    on_partial: Some(&on_partial),
                },
                on_retry: Some(&on_retry),
                on_stream_open: Some(&on_open),
            })
            .await;

            match result {
                Ok(r) => {
                    self.recovery_ctx.record_success();
                    self.hook_registry
                        .lock()
                        .fire_after_model(&r.text, r.tool_calls.len());
                    return Ok(r);
                }
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        return Err(ModelTurnFailure::Cancelled);
                    }
                    if turn_cancel.is_cancelled() {
                        return Err(ModelTurnFailure::Interrupted(checkpoint.lock().clone()));
                    }
                    let msg = e.to_string();
                    self.recovery_ctx.record_error(&msg);
                    match self.try_recover(&msg).await {
                        RecoveryOutcome::Retry => {
                            tracing::info!("recovery engine retrying after stream errors");
                            continue;
                        }
                        RecoveryOutcome::GiveUp => {
                            return Err(ModelTurnFailure::Failed(msg));
                        }
                    }
                }
            }
        }

        Err(ModelTurnFailure::Failed(
            "exhausted recovery attempts".to_string(),
        ))
    }

    /// Queue a best-effort mid-turn context snapshot write on the blocking
    /// pool. Serialization + disk I/O never stall the turn loop (e.g. before
    /// emitting `ApprovalRequired`). A generation counter drops superseded
    /// in-flight writes so an older snapshot cannot overwrite a newer one.
    ///
    /// Always refreshes the shared in-memory snapshots first so live
    /// `get_context_usage` / model-window readers stay current, and queues a
    /// durable `session_model_windows` checkpoint alongside the crash-safe
    /// transcript file when a session is attached.
    pub(super) fn save_session_snapshot(&mut self) {
        self.refresh_context_snapshot();

        let generation = self.session_snapshot_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gen_arc = self.session_snapshot_gen.clone();
        let snapshot_lock = self.session_snapshot_lock.clone();

        if let Some(path) = self.session_snapshot_path.clone() {
            let messages = crate::session::messages_for_snapshot(self.full_transcript());
            let gen_for_file = gen_arc.clone();
            let lock_for_file = snapshot_lock.clone();
            self.join_set.spawn(async move {
                let write_result = tokio::task::spawn_blocking(move || {
                    write_snapshot_if_current(
                        &path,
                        &messages,
                        generation,
                        &gen_for_file,
                        &lock_for_file,
                    )
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

        self.queue_model_window_checkpoint(generation);
    }

    /// Persist the live model window under the same generation/lock as the
    /// crash snapshot so idle usage and the next Run see an up-to-date window.
    fn queue_model_window_checkpoint(&mut self, generation: u64) {
        let (Some(session_manager), Some(session_id)) =
            (self.session_manager.clone(), self.session_id.clone())
        else {
            return;
        };
        let model_id = self.client.model.model_id.clone();
        let full_transcript = self.full_transcript.clone();
        let model_window = self.context.raw_messages().to_vec();
        let gen_arc = self.session_snapshot_gen.clone();
        let snapshot_lock = self.session_snapshot_lock.clone();

        self.join_set.spawn(async move {
            let write_result = tokio::task::spawn_blocking(move || {
                let _guard = snapshot_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if gen_arc.load(Ordering::Relaxed) != generation {
                    return Ok(());
                }
                session_manager.save_model_window_checkpoint(
                    &session_id,
                    &model_id,
                    &full_transcript,
                    &model_window,
                )
            })
            .await;

            match write_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "failed to persist model-window checkpoint");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "model-window checkpoint task panicked or cancelled");
                }
            }
        });
    }
}

fn write_snapshot_if_current(
    path: &std::path::Path,
    messages: &[Message],
    generation: u64,
    current_generation: &std::sync::atomic::AtomicU64,
    snapshot_lock: &std::sync::Mutex<()>,
) -> std::io::Result<()> {
    let _guard = snapshot_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current_generation.load(Ordering::Relaxed) != generation {
        return Ok(());
    }
    let json = serde_json::to_string(messages)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if current_generation.load(Ordering::Relaxed) != generation {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp_path = path.with_extension("tmp.snapshot");
    match std::fs::write(&tmp_path, &json).and_then(|_| std::fs::rename(&tmp_path, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn invalidated_writer_waiting_on_commit_lock_cannot_restore_stale_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let lock = Arc::new(std::sync::Mutex::new(()));
        let held = lock.lock().unwrap();
        let queued = Arc::new(Barrier::new(2));

        let writer = {
            let generation = generation.clone();
            let lock = lock.clone();
            let queued = queued.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                queued.wait();
                write_snapshot_if_current(&path, &[Message::user("stale")], 1, &generation, &lock)
                    .unwrap();
            })
        };

        queued.wait();
        generation.fetch_add(1, Ordering::Relaxed);
        drop(held);
        writer.join().unwrap();

        assert!(
            !path.exists(),
            "a writer invalidated while waiting for the live-checkpoint lock must skip rename"
        );
    }
}

fn format_user_friendly_error(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("503")
        || lower.contains("service unavailable")
        || lower.contains("502")
        || lower.contains("bad gateway")
        || lower.contains("504")
        || lower.contains("gateway timeout")
    {
        "The AI model service is temporarily unavailable or overloaded (returned a server 503/502 error). I tried to connect several times but it is still not responding. Please try again in a minute; this is usually a brief issue on the provider's side.".to_string()
    } else if lower.contains("429") || lower.contains("rate limit") {
        "The AI model service is currently rate-limiting requests (HTTP 429). I retried several times but it is still busy — please wait a moment and try again.".to_string()
    } else {
        err.to_string()
    }
}
