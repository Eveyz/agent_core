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

        // Add user message to context (strip /goal prefix; pin as goal when present; clean /learn prefix)
        let trimmed = user_input.trim();
        let is_goal_clear = trimmed == "/goal clear"
            || trimmed == "/goal stop"
            || trimmed == "/goal cancel"
            || trimmed == "/goal off";
        let new_goal = user_input
            .strip_prefix("/goal ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !is_goal_clear);
        let is_learn = trimmed == "/learn" || trimmed.starts_with("/learn ");
        let display_input = if is_learn {
            "/learn".to_string()
        } else if is_goal_clear {
            "/goal clear".to_string()
        } else {
            new_goal
                .clone()
                .unwrap_or_else(|| user_input.to_string())
        };
        let user_images = std::mem::take(&mut self.pending_user_images);
        self.append_conversation(
            Message::user_with_model(&display_input, &self.client.model.model_id)
                .with_images(user_images),
        );
        self.refresh_context_snapshot();
        self.save_session_snapshot();

        // /learn: inject system instruction prompting the agent to extract and save lessons
        if is_learn {
            let learn_content = if user_input.trim() == "/learn" {
                "".to_string()
            } else {
                user_input.trim().strip_prefix("/learn ").unwrap_or("").trim().to_string()
            };

            let learn_prompt = if learn_content.is_empty() {
                "System instruction: The user wants you to learn from this session. Please analyze the conversation history, identify any critical lessons, coding standards, user preferences, or workflows established. \
                 \n\n\
                 You have two ways to save this learning based on its complexity:\n\
                 1. Core Memory: If it is a user trait, simple preference, or rule, call the `core_memory_append` tool (with block_id: 'human') to append it.\n\
                 2. Custom Skill: If it is a complex workflow, reusable procedure, or specialized agent task, create a Custom Skill. To create the skill:\n\
                    - Check if there is an available meta skill called `skill-creator` (by Anthropic). If it is available, use the `skill-creator` skill to build the skill.\n\n\
                    - If the `skill-creator` skill is not available, fallback to writing a `SKILL.md` file (starting with YAML frontmatter containing 'name' and 'description') under one of the customization directories:\n\
                      * Workspace root: `.agents/skills/<skill_name>/SKILL.md` (applies to all agents in this project)\n\
                      * Antigravity/Gemini Global: `/Users/zniverse/.gemini/config/skills/<skill_name>/SKILL.md`\n\
                      * Claude Code Global: `~/.claudecode/skills/<skill_name>/SKILL.md`\n\
                      * OpenCode / Codex Global customization folders.\n\n\
                 Choose the most appropriate approach, call the corresponding tools to save it, and respond explaining what you have learned and saved.".to_string()
            } else {
                format!(
                    "System instruction: The user wants you to save the following specific learning/rule/workflow:\n\
                     \"{}\"\n\n\
                     You have two ways to save this based on its complexity:\n\
                     1. Core Memory: If it is a user preference, habit, or simple rule, call the `core_memory_append` tool (with block_id: 'human') to append it.\n\
                     2. Custom Skill: If it is a complex workflow, reusable procedure, or specialized agent task, create a Custom Skill. To create the skill:\n\
                        - Check if there is an available meta skill called `skill-creator` (by Anthropic). If it is available, use the `skill-creator` skill to build the skill.\n\n\
                        - If the `skill-creator` skill is not available, fallback to writing a `SKILL.md` file (starting with YAML frontmatter containing 'name' and 'description') under one of the customization directories:\n\
                          * Workspace root: `.agents/skills/<skill_name>/SKILL.md` (applies to all agents in this project)\n\
                          * Antigravity/Gemini Global: `/Users/zniverse/.gemini/config/skills/<skill_name>/SKILL.md`\n\
                          * Claude Code Global: `~/.claudecode/skills/<skill_name>/SKILL.md`\n\
                          * OpenCode / Codex Global customization folders.\n\n\
                     Choose the most appropriate approach, call the corresponding tools to save it, and respond to confirm what you have saved.",
                    learn_content
                )
            };

            self.append_conversation(Message::system(&learn_prompt));
        }

        // Store in memory if enabled
        let mut reflected_session_id = self.session_id.clone();
        if let Some(ref mem) = self.brain.memory {
            if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                // Compute the embedding OUTSIDE the memory lock so other
                // memory operations are not blocked for the 10-50ms the
                // embedding model takes. The lock is then only held for
                // the lightweight I/O + index update.
                let model = { mem.lock().embedding_model().cloned() };
                let embedding = model
                    .map(|model| model.embed_single(user_input).unwrap_or_default());
                let m = mem.lock();
                let memory_session_id = self.session_id.as_deref().unwrap_or_else(|| m.session_id());
                reflected_session_id = Some(memory_session_id.to_string());
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
            if let Some(session_id) = reflected_session_id.as_deref() {
                daemon.notify_session(session_id);
            }
        }

        // Skill auto-trigger: check user message against skill triggers and @skill: tags
        let mut skill_misses: Vec<String> = Vec::new();
        if let Some(ref sm) = self.skill_manager {
            let mut mgr = sm.lock();
            let sid = self.session_id.as_deref();
            let matched_names: Vec<String> = mgr
                .check_triggers_for(sid, user_input)
                .iter()
                .map(|s| s.name.clone())
                .collect();
            for name in matched_names {
                mgr.activate_for(sid, &name);
            }

            let (_activated, missing) = mgr.activate_mentions_in(sid, user_input);
            skill_misses = missing;
        }
        for name in skill_misses {
            self.emit(RunEvent::Notice {
                code: "skill_not_found".to_string(),
                severity: "warning".to_string(),
                recoverable: true,
                message: format!(
                    "Skill '{name}' not found; use skill_list to see available skills."
                ),
            });
        }

        // /goal clear — drop session-level pin for this Run (persistence cleared by Tauri).
        if is_goal_clear {
            self.goal = None;
            self.goal_completed = false;
            self.goal_continue_nudges = 0;
            if let Some(ref sid) = self.session_id {
                self.brain.todo_lists.clear_session(sid);
            } else {
                self.brain.todo_lists.clear_session("");
            }
            self.emit(RunEvent::GoalCleared);
            self.emit_plans_updated();
        }

        // /goal <text>: pin a new goal (do NOT auto-decompose).
        // Auto-decomposition raced ahead of ask_user and produced generic plans
        // the agent then narrated instead of clarifying / executing.
        if let Some(ref g) = new_goal {
            self.goal = Some(g.clone());
            self.goal_completed = false;
            self.goal_continue_nudges = 0;
            if let Some(ref sid) = self.session_id {
                self.brain.todo_lists.clear_session(sid);
            } else {
                self.brain.todo_lists.clear_session("");
            }
            self.emit(RunEvent::GoalSet { goal: g.clone() });
            self.emit_plans_updated();
        } else if !is_goal_clear {
            // Inherited session-level goal: re-emit so UI stays in sync on follow-ups.
            if let Some(g) = self.goal.clone() {
                if !self.goal_completed {
                    self.emit(RunEvent::GoalSet { goal: g });
                }
            }
        }

        // New user prompt: park session-active plan if it belongs to another prompt.
        if !is_goal_clear
            && new_goal.is_none()
            && !is_learn
            && !crate::todo::is_plan_park_cmd(trimmed)
            && !crate::todo::is_plan_clear_cmd(trimmed)
            && !crate::todo::is_plan_resume_cmd(trimmed)
            && !crate::todo::is_bare_continue(trimmed)
            && !crate::todo::is_explicit_plan_resume(trimmed)
            && !trimmed.is_empty()
        {
            if self
                .brain
                .todo_lists
                .park_active_if_other_prompt(
                    self.session_id.as_deref(),
                    self.prompt_id.as_deref(),
                )
            {
                self.emit_plans_updated();
            }
        }

        // /plan park | /plan clear | /plan resume — explicit plan lifecycle.
        let is_plan_park = crate::todo::is_plan_park_cmd(trimmed);
        let is_plan_clear = crate::todo::is_plan_clear_cmd(trimmed);
        let is_plan_resume = crate::todo::is_plan_resume_cmd(trimmed);
        if is_plan_park {
            let _ = self
                .brain
                .todo_lists
                .park_active(self.session_id.as_deref());
            self.emit_plans_updated();
            self.execution.set_resume_hint(
                "Active plan parked. Answer the user; do not advance parked steps.".to_string(),
            );
        } else if is_plan_clear {
            if let Some(ref sid) = self.session_id {
                self.brain.todo_lists.clear_session(sid);
            } else {
                self.brain.todo_lists.clear_session("");
            }
            self.emit_plans_updated();
            self.execution.sync_from_todos(&crate::todo::TodoList::new());
        } else if is_plan_resume || crate::todo::is_explicit_plan_resume(trimmed) {
            use crate::todo::ResumeTarget;
            match crate::todo::parse_resume_target(trimmed) {
                ResumeTarget::PlanId(id) => {
                    match self
                        .brain
                        .todo_lists
                        .activate(self.session_id.as_deref(), &id)
                    {
                        Ok(()) => {
                            let list = self
                                .brain
                                .todo_lists
                                .active_list(self.session_id.as_deref());
                            self.execution.sync_from_todos(&list);
                            self.execution.set_resume_hint(
                                "Plan resumed. Continue the NEXT incomplete step with tools."
                                    .to_string(),
                            );
                            self.emit_plans_updated();
                        }
                        Err(e) => {
                            self.execution.set_resume_hint(format!(
                                "Could not resume plan: {e}. Call ask_user or name the plan clearly."
                            ));
                        }
                    }
                }
                ResumeTarget::Title(title) => {
                    match self
                        .brain
                        .todo_lists
                        .activate_by_title(self.session_id.as_deref(), &title)
                    {
                        Ok(_) => {
                            let list = self
                                .brain
                                .todo_lists
                                .active_list(self.session_id.as_deref());
                            self.execution.sync_from_todos(&list);
                            self.execution.set_resume_hint(format!(
                                "Plan \"{title}\" resumed. Continue the NEXT incomplete step with tools."
                            ));
                            self.emit_plans_updated();
                        }
                        Err(_) => {
                            // Title ambiguous/missing — fall back to single-parked / ask_user.
                            self.apply_continue_resolution();
                        }
                    }
                }
                ResumeTarget::Unspecified => {
                    self.apply_continue_resolution();
                }
            }
        } else if crate::todo::is_bare_continue(trimmed)
            && self
                .brain
                .todo_lists
                .has_resumable_work(self.session_id.as_deref())
        {
            // Ambiguous "continue" — ask whether to resume the plan.
            self.apply_bare_continue_ask();
        } else if crate::todo::looks_like_detour(trimmed) {
            // Explicit side-channel (btw / quick question): park active so Final-block doesn't hijack.
            let has_active_incomplete = self.brain.todo_lists.with_active(
                self.session_id.as_deref(),
                |list| list.has_incomplete(),
            );
            if has_active_incomplete {
                let _ = self
                    .brain
                    .todo_lists
                    .park_active(self.session_id.as_deref());
                self.emit_plans_updated();
            }
        }
        // Object-bearing continue ("继续算斐波那契") falls through as a normal turn.

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
                Ok(TurnOutcome::Final(text)) => {
                    // Soft autopilot: block premature Final while Execute/Verify
                    // still has incomplete steps (goal-pinned or not).
                    if self.should_block_premature_final() {
                        self.execution.note_final_blocked();
                        self.goal_continue_nudges =
                            self.goal_continue_nudges.saturating_add(1);
                        let step = self
                            .execution
                            .active_step_id
                            .clone()
                            .unwrap_or_else(|| "next pending".into());
                        let nudge = format!(
                            "[System] Execution phase is `{}` with incomplete steps. \
                             Do NOT stop with prose only. Continue step {step} with tools \
                             (todo_update as you go). Do not replan unless force=true.",
                            self.execution.phase
                        );
                        self.append_conversation(Message::system(&nudge));
                        self.last_turn_end_time = Some(Instant::now());
                        self.current_turn_id = None;
                        continue;
                    }
                    // Successful Final in Verify with all todos done → Done.
                    {
                        use crate::runtime::execution::ExecutionPhase;
                        let all_done = {
                            let todos = self.session_todos();
                            let list = todos.lock();
                            !list.items.is_empty()
                                && list.items.iter().all(|i| {
                                    i.status == crate::todo::TodoStatus::Completed
                                })
                        };
                        if all_done && self.execution.phase == ExecutionPhase::Verify {
                            self.execution.mark_verified_done();
                        }
                    }
                    return Ok(text);
                }
                Ok(TurnOutcome::Continue) => {}
                Err(RunError::Cancelled) => return Err(RunError::Cancelled),
                Err(RunError::Failed(e)) => return Err(RunError::Failed(e)),
            }
            // /goal: emit GoalCompleted once all todos are done
            if let Some(ref goal) = self.goal {
                if !self.goal_completed {
                    let all_done = {
                        let todos = self.session_todos();
                        let list = todos.lock();
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
                    self.enqueue_steer(steer_id, message);
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
                    let mut skill_misses: Vec<String> = Vec::new();
                    if let Some(ref sm) = self.skill_manager {
                        let mut mgr = sm.lock();
                        let sid = self.session_id.as_deref();
                        let (_activated, missing) = mgr.activate_mentions_in(sid, &message);
                        skill_misses = missing;
                    }
                    for name in skill_misses {
                        self.emit(RunEvent::Notice {
                            code: "skill_not_found".to_string(),
                            severity: "warning".to_string(),
                            recoverable: true,
                            message: format!(
                                "Skill '{name}' not found; use skill_list to see available skills."
                            ),
                        });
                    }
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

    /// Queue a steer message and notify the frontend.
    fn enqueue_steer(&mut self, steer_id: String, message: String) {
        let entry = SteerEntry {
            id: steer_id.clone(),
            message: RunCommand::steer_message(&message),
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

    /// Drain pending commands, then inject at most one steer into context.
    ///
    /// Called at turn boundaries so a steer that arrived mid-turn (still sitting
    /// in `cmd_rx`) is available for injection before the next LLM call —
    /// without waiting an extra full turn.
    ///
    /// Returns `true` if a steer was injected.
    pub(super) fn inject_next_steer(&mut self) -> Result<bool, RunError> {
        self.poll_commands()?;
        if let Some(entry) = self.steering_queue.pop_front() {
            self.emit(RunEvent::SteerInjected {
                steer_id: entry.id.clone(),
                message: entry.raw_text.clone(),
            });
            // Activate @skill: mentions in steer text the same as initial input.
            let mut skill_misses: Vec<String> = Vec::new();
            if let Some(ref sm) = self.skill_manager {
                let mut mgr = sm.lock();
                let sid = self.session_id.as_deref();
                let (_activated, missing) = mgr.activate_mentions_in(sid, &entry.raw_text);
                skill_misses = missing;
            }
            for name in skill_misses {
                self.emit(RunEvent::Notice {
                    code: "skill_not_found".to_string(),
                    severity: "warning".to_string(),
                    recoverable: true,
                    message: format!(
                        "Skill '{name}' not found; use skill_list to see available skills."
                    ),
                });
            }
            self.append_conversation(entry.message);
            return Ok(true);
        }
        Ok(false)
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
                    self.enqueue_steer(steer_id, message);
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
                    let mut skill_misses: Vec<String> = Vec::new();
                    if let Some(ref sm) = self.skill_manager {
                        let mut mgr = sm.lock();
                        let sid = self.session_id.as_deref();
                        let (_activated, missing) = mgr.activate_mentions_in(sid, &message);
                        skill_misses = missing;
                    }
                    for name in skill_misses {
                        self.emit(RunEvent::Notice {
                            code: "skill_not_found".to_string(),
                            severity: "warning".to_string(),
                            recoverable: true,
                            message: format!(
                                "Skill '{name}' not found; use skill_list to see available skills."
                            ),
                        });
                    }
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
        tracing::debug!(prompt_id, "approval prompt not found");
    }

    pub(super) fn resolve_input(&mut self, prompt_id: &str, answer: &str) {
        let answers: crate::runtime::input::ClarificationAnswers =
            match serde_json::from_str(answer) {
                Ok(a) => a,
                Err(e) => {
                    // Accept a bare `{ "q": ["a"] }` map as answers.
                    match serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(
                        answer,
                    ) {
                        Ok(map) => crate::runtime::input::ClarificationAnswers { answers: map },
                        Err(_) => {
                            tracing::warn!(prompt_id, error = %e, "invalid clarification answer JSON");
                            return;
                        }
                    }
                }
            };

        if self.input_resolver.resolve(prompt_id, answers.clone()) {
            self.emit(RunEvent::InputResolved {
                prompt_id: prompt_id.to_string(),
                answers,
            });
        } else {
            tracing::debug!(prompt_id, "clarification prompt not found");
        }
    }

    /// Whether a text-only Final should be rejected so the agent keeps working.
    /// Uses runtime ExecutionPhase (not only pinned goal).
    fn should_block_premature_final(&self) -> bool {
        // Ask/Plan are review/Q&A modes — never force-continue on leftover todos.
        if !matches!(self.mode, crate::mode::AgentMode::Build) {
            return false;
        }
        let list = self
            .brain
            .todo_lists
            .active_list(self.session_id.as_deref());
        if self.execution.should_block_final(&list) {
            return true;
        }
        // Legacy goal nudge: empty plan under an active goal still needs work.
        const MAX_NUDGES: u8 = 5;
        if self.goal.is_none() || self.goal_completed || self.goal_continue_nudges >= MAX_NUDGES {
            return false;
        }
        list.items.is_empty()
            || list
                .items
                .iter()
                .any(|i| i.status != crate::todo::TodoStatus::Completed)
    }

    fn apply_continue_resolution(&mut self) {
        use crate::mode::AgentMode;
        use crate::todo::ContinueResolution;

        // Ask/Plan must not be nudged toward todo tools.
        if matches!(self.mode, AgentMode::Ask | AgentMode::Plan) {
            let hint = match self.mode {
                AgentMode::Ask => {
                    "Continue answering with read-only tools if needed, then respond in text. \
                     File writes and checklist tools are unavailable in Ask mode."
                }
                AgentMode::Plan => {
                    "Continue research, then write or update `plan.md` via write_file for user review. \
                     Do not implement source changes in Plan mode."
                }
                AgentMode::Build => unreachable!(),
            };
            self.execution.set_resume_hint(hint.to_string());
            self.emit_plans_updated();
            return;
        }

        match self
            .brain
            .todo_lists
            .resolve_continue(self.session_id.as_deref())
        {
            ContinueResolution::NothingParked => {
                // Maybe already active — sync; else hint.
                let list = self
                    .brain
                    .todo_lists
                    .active_list(self.session_id.as_deref());
                if list.has_incomplete() {
                    self.execution.sync_from_todos(&list);
                    if let Some(id) = self.execution.active_step_id.clone() {
                        self.execution.set_resume_hint(format!(
                            "Resume active plan step {id}. Do not todo_write unless the plan is wrong."
                        ));
                    }
                } else {
                    self.execution.set_resume_hint(
                        "Nothing parked to resume. Ask what to work on, or todo_write a plan."
                            .to_string(),
                    );
                }
                self.emit_plans_updated();
            }
            ContinueResolution::Activated { title, .. } => {
                let list = self
                    .brain
                    .todo_lists
                    .active_list(self.session_id.as_deref());
                self.execution.sync_from_todos(&list);
                let step = self
                    .execution
                    .active_step_id
                    .clone()
                    .unwrap_or_else(|| "next".into());
                self.execution.set_resume_hint(format!(
                    "Resumed plan \"{title}\". Continue step {step} with tools; \
                     do NOT todo_write unless the plan is wrong."
                ));
                self.emit_plans_updated();
            }
            ContinueResolution::Choose(parked) => {
                let mut choices = String::from(
                    "Multiple parked plans. Call ask_user with ONE question listing these options \
                     (use plan ids as option ids). Default/latest is first:\n",
                );
                for (i, p) in parked.iter().enumerate() {
                    choices.push_str(&format!(
                        "  {}. id={} \"{}\" ({}/{})\n",
                        i + 1,
                        p.id,
                        p.title,
                        p.completed,
                        p.total
                    ));
                }
                choices.push_str(
                    "After the user picks, the runtime will activate via /plan resume <id> \
                     or you may tell them to click Resume in the UI. Do not invent todo ids.",
                );
                self.execution.set_resume_hint(choices);
                self.emit_plans_updated();
            }
        }
    }

    /// Bare "continue" with resumable work — force ask_user to avoid hijacking.
    fn apply_bare_continue_ask(&mut self) {
        use crate::mode::AgentMode;
        if matches!(self.mode, AgentMode::Ask | AgentMode::Plan) {
            self.apply_continue_resolution();
            return;
        }

        let latest = self
            .brain
            .todo_lists
            .latest_parked(self.session_id.as_deref());
        let active = self
            .brain
            .todo_lists
            .active_list(self.session_id.as_deref());
        let (label, progress) = if let Some(p) = latest {
            (p.title, format!("{}/{}", p.completed, p.total))
        } else if active.has_incomplete() {
            let (done, total) = active.progress_counts();
            let title = self
                .brain
                .todo_lists
                .snapshot(self.session_id.as_deref())
                .active_plan_title
                .unwrap_or_else(|| "current plan".into());
            (title, format!("{done}/{total}"))
        } else {
            self.execution.set_resume_hint(
                "Nothing to resume. Ask what to work on, or todo_write a plan.".to_string(),
            );
            return;
        };

        self.execution.set_resume_hint(format!(
            "The user said a bare \"continue\" which is ambiguous. Call ask_user with ONE question:\n\
             \"Continue executing the plan '{label}' ({progress}), or do something else?\"\n\
             Options:\n\
             - id=resume_plan label=\"Continue plan: {label} ({progress})\"\n\
             - id=not_plan label=\"Not the plan — follow my message as written\"\n\
             If they pick resume_plan, tell them to say /plan resume (or click Resume). \
             If not_plan, answer their message without advancing parked steps."
        ));
        self.emit_plans_updated();
    }
}
