//! Turn execution — model interaction, streaming collection, and turn dispatch.

use anyhow::Result;
use futures::StreamExt;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

use crate::runtime::tool_orchestrator::ToolOrchestrator;
use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
use crate::runtime::event::{Envelope, RunEvent, TodoItemPayload};
use crate::runtime::guard::EventGuard;
use crate::types::{CacheUsage, Message, MessageDelta, StreamEvent, ToolCall};

use super::{RecoveryOutcome, Run, RunError, TurnOutcome, CACHE_IDLE_WARN_SECS};

impl Run {
    pub(super) async fn run_turn(&mut self, turn_index: usize) -> Result<TurnOutcome, RunError> {
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
        let (text, tool_calls, message_id, cache_usage) = match self.model_turn().await {
            Ok(r) => r,
            Err(e) => {
                if self.cancel.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                self.emit(RunEvent::Error { message: e.clone() });
                return Ok(TurnOutcome::Stop(format!(
                    "Error communicating with the model: {e}"
                )));
            }
        };

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
            self.emit(RunEvent::MessageEnd {
                message_id: message_id.clone(),
                message: assistant_msg.clone(),
            });
            self.emit(RunEvent::TurnEnded { index: turn_index });
            self.hook_registry.lock().fire_turn_end(turn_index);

            // Store in memory
            if let Some(ref mem) = self.brain.memory {
                if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                    let m = mem.lock();
                    let _ = m.store_conversation("assistant", &text);
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
        self.emit(RunEvent::MessageEnd {
            message_id: message_id.clone(),
            message: assistant_msg.clone(),
        });

        // Stage: Execute
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
            let turn_id = self.current_turn_id.clone();
            let cid = call_id.clone();
            tool_guards.push(EventGuard::new(move || {
                let _ = tx.send(Envelope {
                    seq: seq.fetch_add(1, Ordering::Relaxed),
                    event_id: uuid::Uuid::new_v4().to_string(),
                    run_id: run_id.clone(),
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
        let turn_id = self.current_turn_id.clone();
        // Clone the approval resolver before constructing the orchestrator to
        // avoid borrowing conflicts with `&mut self.permission_policy` etc.
        // Using the per-Run resolver eliminates the actor deadlock that the
        // old global-map fallback path was vulnerable to.
        let approval_resolver = self.approval_resolver.clone();
        let tool_results = {
            let mut orchestrator = ToolOrchestrator {
                registry: &self.registry,
                permission_policy: &mut self.permission_policy,
                hook_registry: self.hook_registry.clone(),
                tool_execution_mode: self.tool_execution_mode,
                cancel_token: self.cancel.clone(),
                approval_resolver: Some(approval_resolver),
                session_id: self.session_id.clone(),
            };
            orchestrator
                .execute_tools(&tool_calls, &move |ev, parent_call_id: &str| {
                    if let Some(run_ev) = RunEvent::from_agent_event(&run_id, ev) {
                        let _ = event_tx.send(Envelope {
                            seq: seq.fetch_add(1, Ordering::Relaxed),
                            event_id: uuid::Uuid::new_v4().to_string(),
                            run_id: run_id.clone(),
                            turn_id: turn_id.clone(),
                            parent_call_id: Some(parent_call_id.to_string()),
                            ts: chrono::Utc::now(),
            event: run_ev,
                        });
                    }
                })
                .await
        };

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

        for _attempt in 0..MAX_RECOVERY_ATTEMPTS {
            if self.cancel.is_cancelled() {
                return Err("aborted".to_string());
            }

            let mut messages = self.build_messages();
            let tools = self.registry.tool_definitions();

            // Apply hygiene: truncate oversized tool results, replace long args
            crate::hygiene::sanitize(&mut messages);

            // BeforeModel hook: SkipModel short-circuit
            let snapshot = self.snapshot_messages_for_hook(&messages);
            if let Some(preset) = self.hook_registry.lock().fire_before_model(&snapshot) {
                self.recovery_ctx.record_success();
                self.hook_registry.lock().fire_after_model(&preset, 0);
                return Ok((preset, Vec::new(), uuid::Uuid::new_v4().to_string(), CacheUsage::default()));
            }

            let stream = self
                .client
                .chat_completion_stream(&messages, &tools)
                .await
                .map_err(|e| format!("LLM request failed: {e}"))?;

            let collected: Result<(String, Vec<ToolCall>, String, CacheUsage), String> = {
                let cancel = self.cancel.clone();
                let event_tx = self.event_tx.clone();
                let res = self.collect_stream(stream, &event_tx).await;
                match res {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        if cancel.is_cancelled() {
                            return Err("aborted".to_string());
                        }
                        Err(format!("Stream error: {e}"))
                    }
                }
            };

            match collected {
                Ok((text, tool_calls, message_id, cache_usage)) => {
                    self.recovery_ctx.record_success();
                    self.hook_registry.lock().fire_after_model(&text, tool_calls.len());

                    return Ok((text, tool_calls, message_id, cache_usage));
                }
                Err(msg) => {
                    self.recovery_ctx.record_error(&msg);
                    match self.try_recover(&msg).await {
                        RecoveryOutcome::Retry => continue,
                        RecoveryOutcome::GiveUp => return Err(msg),
                    }
                }
            }
        }

        Err("exhausted recovery attempts".to_string())
    }

    pub(super) async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_tx: &broadcast::Sender<Envelope>,
    ) -> Result<(String, Vec<ToolCall>, String, CacheUsage)> {
        let mut text_buffer = String::new();
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
            accumulator.into_tool_calls()
        } else {
            vec![]
        };

        Ok((text_buffer, tool_calls, message_id, cache_usage))
    }
}
