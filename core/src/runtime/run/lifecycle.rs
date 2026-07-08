//! Run lifecycle — main entry point, turn loop, command polling,
//! pause/resume, and approval resolution.

use std::time::Instant;

use crate::context::ContextEngine as Context;
use crate::runtime::command::{RunCommand, SteerEntry};
use crate::runtime::event::RunEvent;
use crate::runtime::state::RunState;
use crate::types::Message;

use super::{Run, RunError, TurnOutcome};

impl Run {
    // ── The main entry point ──────────────────────────────────────

    /// Run the execution loop. Consumes self.
    ///
    /// This is spawned as a tokio task by RunManager. It:
    /// 1. Waits for the `Start` command
    /// 2. Runs the turn loop
    /// 3. Handles cancel/pause/approval mid-loop
    /// 4. Cleans up all resources on exit
    pub async fn run(mut self, user_input: &str) {
        // Wait for Start command
        loop {
            match self.cmd_rx.recv().await {
                Some(RunCommand::Start) => break,
                Some(RunCommand::Cancel) | None => {
                    self.transition(RunState::Cancelled);
                    self.emit(RunEvent::RunCancelled {
                        reason: "cancelled before start".into(),
                    });
                    return;
                }
                _ => { /* ignore other commands while Created */ }
            }
        }

        self.emit(RunEvent::RunStarted);
        self.transition(RunState::Running);

        // Add user message to context (strip /goal prefix; pin as goal when present)
        let goal = user_input
            .strip_prefix("/goal ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let display_input = goal.clone().unwrap_or_else(|| user_input.to_string());
        self.context.add(Message::user_with_model(&display_input, &self.client.model.model_id));
        self.refresh_context_snapshot();

        // Store in memory if enabled
        if let Some(ref mem) = self.brain.memory {
            if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                // Compute the embedding OUTSIDE the memory lock so other
                // memory operations are not blocked for the 10-50ms the
                // embedding model takes. The lock is then only held for
                // the lightweight I/O + index update.
                let embedding = {
                    let m = mem.lock();
                    m.embedding_model()
                        .map(|model| model.embed_single(user_input).unwrap_or_default())
                };
                let m = mem.lock();
                let memory_session_id = self.session_id.as_deref().unwrap_or_else(|| m.session_id());
                let _ = m.store_conversation_for_session_precomputed(
                    memory_session_id,
                    "user",
                    user_input,
                    embedding.as_deref(),
                );
            }
        }

        // Feed to reflection daemon (non-blocking, Deep mode only)
        if let Some(ref daemon) = self.brain.reflection_daemon {
            daemon.try_send("user", user_input);
        }

        // Skill auto-trigger: check user message against skill triggers and @skill: tags
        if let Some(ref sm) = self.brain.skill_manager {
            let mut mgr = sm.lock();
            let matched_names: Vec<String> = mgr.check_triggers(user_input)
                .iter()
                .map(|s| s.name.clone())
                .collect();
            for name in matched_names {
                mgr.activate(&name);
            }

            // Also explicitly activate any skills tagged with @skill:name
            for word in user_input.split_whitespace() {
                if let Some(skill_name) = word.strip_prefix("@skill:") {
                    let clean_name = skill_name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
                    mgr.activate(clean_name);
                }
            }
        }

        // /goal: pin goal + decompose into todos
        if let Some(ref g) = goal {
            self.goal = Some(g.clone());
            self.emit(RunEvent::GoalSet { goal: g.clone() });
            match self.decompose_goal(g).await {
                Ok(items) => self.emit(RunEvent::TodoUpdated { items }),
                Err(RunError::Failed(msg)) => tracing::warn!("goal decomposition failed: {msg}"),
                Err(RunError::Cancelled) => tracing::warn!("goal decomposition cancelled"),
            }
        }

        // Run the loop
        let result = self.run_loop().await;

        // Refresh snapshot so callers (Agent wrapper) get final context.
        self.refresh_context_snapshot();

        match result {
            Ok(text) => {
                // Auto-session-summary: write a brief memory file
                self.write_session_memory(&text);

                // Emit cumulative cache metrics before final completion event.
                if self.cache_metrics.has_data() {
                    self.emit(RunEvent::CacheSummary {
                        total_turns: self.cache_metrics.total_turns,
                        total_hit_tokens: self.cache_metrics.total_hit_tokens,
                        total_miss_tokens: self.cache_metrics.total_miss_tokens,
                        turns_with_hits: self.cache_metrics.turns_with_hits,
                        cumulative_hit_rate: self.cache_metrics.cumulative_hit_rate,
                    });
                }

                self.transition(RunState::Completed);
                self.emit(RunEvent::RunCompleted { final_text: text });
            }
            Err(RunError::Cancelled) => {
                self.cancel_and_cleanup().await;
                self.transition(RunState::Cancelled);
                self.emit(RunEvent::RunCancelled {
                    reason: "cancelled by user".into(),
                });
            }
            Err(RunError::Failed(e)) => {
                self.transition(RunState::Failed);
                self.emit(RunEvent::RunFailed { error: e });
            }
        }

        // Final cleanup (idempotent — already done if cancelled)
        self.cleanup_on_exit();
    }

    pub(super) async fn run_loop(&mut self) -> Result<String, RunError> {
        for turn_index in 0..self.max_iterations {
            // ── Hot-reload configs ─────────────────────────────────
            // Ensure the active run dynamically picks up config changes
            // (e.g. user changes permission level mid-conversation).
            self.permission_policy.update_from_config(&self.brain.config.permissions);

            // Re-render and update the tool catalog in context.
            // Cache the rendered string using the registry fingerprint to avoid
            // rebuilding every turn — only rebuild when tools or permissions change.
            let fp = self.registry.registry_fingerprint();
            let perm_mode = format!("{:?}", self.permission_policy.mode());
            let cache_key = format!("{perm_mode}|{fp}");
            let needs_rebuild = self.tool_catalog_cache.as_ref().map_or(true, |(k, _)| k != &cache_key);
            if needs_rebuild {
                let tool_defs = self.registry.tool_definitions();
                let danger_map = super::build_danger_map(&tool_defs, &self.permission_policy);
                let updated_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
                self.context.set_tool_catalog(&updated_catalog);
                self.tool_catalog_cache = Some((cache_key, updated_catalog));
            }

            // ── Poll commands (non-blocking) ───────────────────────
            self.poll_commands()?;

            if self.cancel.is_cancelled() {
                return Err(RunError::Cancelled);
            }

            if self.state == RunState::Paused {
                self.wait_for_resume().await?;
                if self.cancel.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
            }

            let turn_id = uuid::Uuid::new_v4().to_string();
            self.current_turn_id = Some(turn_id.clone());
            self.emit(RunEvent::TurnStarted { index: turn_index });
            self.refresh_context_snapshot();
            self.hook_registry.lock().fire_turn_start(turn_index);

            match self.run_turn(turn_index).await {
                Ok(TurnOutcome::Final(text)) => return Ok(text),
                Ok(TurnOutcome::Continue) => {}
                Ok(TurnOutcome::Stop(msg)) => return Ok(msg),
                Err(RunError::Cancelled) => return Err(RunError::Cancelled),
                Err(RunError::Failed(e)) => return Err(RunError::Failed(e)),
            }
            // /goal: emit GoalCompleted once all todos are done
            if let Some(ref goal) = self.goal {
                if !self.goal_completed {
                    let all_done = {
                        let list = self.brain.todo_list.lock();
                        !list.items.is_empty()
                            && list
                                .items
                                .iter()
                                .all(|i| i.status == crate::todo::TodoStatus::Completed)
                    };
                    if all_done {
                        let goal_text = goal.clone();
                        self.goal_completed = true;
                        self.emit(RunEvent::GoalCompleted { goal: goal_text });
                    }
                }
            }

            // Turn ended — clear the active turn id and record timestamp.
            self.last_turn_end_time = Some(Instant::now());
            self.current_turn_id = None;
        }

        let summary = super::build_iteration_limit_summary(&self.context, self.max_iterations);
        Err(RunError::Failed(summary))
    }

    /// Non-blocking poll of the command channel.
    pub(super) fn poll_commands(&mut self) -> Result<(), RunError> {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                RunCommand::Cancel => {
                    self.cancel.cancel();
                    return Err(RunError::Cancelled);
                }
                RunCommand::Pause => {
                    if self.state == RunState::Running {
                        self.transition(RunState::Paused);
                        self.emit(RunEvent::RunPaused);
                    }
                }
                RunCommand::Steer { steer_id, message } => {
                    let entry = SteerEntry {
                        id: steer_id.clone(),
                        message: Message::user(&message),
                        raw_text: message.clone(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };
                    self.steering_queue.push_back(entry);
                    let depth = self.steering_queue.len();
                    self.emit(RunEvent::SteerQueued {
                        steer_id,
                        message,
                        queue_depth: depth,
                    });
                }
                RunCommand::CancelSteer { steer_id } => {
                    let before = self.steering_queue.len();
                    self.steering_queue.retain(|e| e.id != steer_id);
                    if self.steering_queue.len() < before {
                        self.emit(RunEvent::SteerCancelled {
                            steer_id,
                            reason: "Cancelled by user".to_string(),
                        });
                    }
                    // If not found, it was already injected — silent no-op.
                }
                RunCommand::Approve { prompt_id, choice } => {
                    self.resolve_approval(&prompt_id, choice);
                }
                RunCommand::Answer { prompt_id, answer } => {
                    self.resolve_input(&prompt_id, &answer);
                }
                RunCommand::FollowUp { message } => {
                    self.follow_up_queue.push_back(Message::user(&message));
                }
                RunCommand::ClearQueues => {
                    self.steering_queue.clear();
                    self.follow_up_queue.clear();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Block until Resume or Cancel is received.
    pub(super) async fn wait_for_resume(&mut self) -> Result<(), RunError> {
        loop {
            match self.cmd_rx.recv().await {
                Some(RunCommand::Resume) => {
                    self.transition(RunState::Running);
                    self.emit(RunEvent::RunResumed);
                    return Ok(());
                }
                Some(RunCommand::Cancel) | None => {
                    self.cancel.cancel();
                    return Err(RunError::Cancelled);
                }
                Some(RunCommand::Steer { steer_id, message }) => {
                    let entry = SteerEntry {
                        id: steer_id.clone(),
                        message: Message::user(&message),
                        raw_text: message.clone(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };
                    self.steering_queue.push_back(entry);
                    let depth = self.steering_queue.len();
                    self.emit(RunEvent::SteerQueued {
                        steer_id,
                        message,
                        queue_depth: depth,
                    });
                }
                Some(RunCommand::CancelSteer { steer_id }) => {
                    let before = self.steering_queue.len();
                    self.steering_queue.retain(|e| e.id != steer_id);
                    if self.steering_queue.len() < before {
                        self.emit(RunEvent::SteerCancelled {
                            steer_id,
                            reason: "Cancelled by user".to_string(),
                        });
                    }
                }
                Some(RunCommand::Approve { prompt_id, choice }) => {
                    self.resolve_approval(&prompt_id, choice);
                }
                Some(RunCommand::Answer { prompt_id, answer }) => {
                    self.resolve_input(&prompt_id, &answer);
                }
                Some(RunCommand::FollowUp { message }) => {
                    self.follow_up_queue.push_back(Message::user(&message));
                }
                Some(RunCommand::ClearQueues) => {
                    self.steering_queue.clear();
                    self.follow_up_queue.clear();
                }
                _ => {}
            }
        }
    }

    pub(super) fn resolve_approval(&mut self, prompt_id: &str, choice: crate::permission::ApprovalChoice) {
        // Try the per-Run resolver first (used when ToolOrchestrator has
        // approval_resolver set, which is the new default path).
        if self.approval_resolver.resolve(prompt_id, choice.clone()) {
            tracing::debug!(prompt_id, "approval resolved via per-Run resolver");
            self.emit(RunEvent::ApprovalResolved {
                prompt_id: prompt_id.to_string(),
                choice,
            });
            return;
        }
        // Fallback: global map — used by the deprecated Agent path, subagents,
        // and any code that still sets approval_resolver: None.
        #[allow(deprecated)]
        {
            let pending_arc = crate::permission::global_pending_approvals();
            let mut pending = pending_arc.lock();
            if let Some(tx) = pending.remove(prompt_id) {
                tracing::debug!(prompt_id, "approval resolved via global map");
                let _ = tx.send(choice.clone());
                self.emit(RunEvent::ApprovalResolved {
                    prompt_id: prompt_id.to_string(),
                    choice,
                });
                return;
            }
        }
        tracing::debug!(prompt_id, "approval prompt not found");
    }

    pub(super) fn resolve_input(&mut self, _prompt_id: &str, _answer: &str) {
        // TODO: implement input request mechanism (future phase)
    }
}
