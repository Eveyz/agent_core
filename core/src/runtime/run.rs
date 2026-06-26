//! Run — a single, independent execution space for one user request.
//!
//! Each Run owns:
//! - Its own [`ContextEngine`] (isolated from other Runs)
//! - Its own [`ProcessSupervisor`] (child processes killed on cancel/drop)
//! - Its own [`JoinSet`] (background tasks aborted on cancel/drop)
//! - Its own [`CancellationToken`] (propagates to model stream + tool exec)
//! - Its own permission policy copy and hook registry
//!
//! A Run is created by [`crate::runtime::RunManager`], driven by commands
//! from the frontend, and emits events via a broadcast channel.
//!
//! ## Cleanup guarantee
//!
//! Three layers ensure no leaks:
//! 1. Normal completion → explicit transition to terminal state
//! 2. Cancel → `cancel_and_cleanup` kills processes + aborts tasks
//! 3. Drop (RAII) → supervisor + join_set + cancel all fire automatically

use anyhow::{Result, bail};
use futures::StreamExt;
use parking_lot::{Mutex, MutexGuard};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::agent::ContextProcessor;
use crate::agent::executor::ToolOrchestrator;
use crate::client::OpenAIClient;
use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
use crate::config::ModelConfig;
use crate::context::ContextEngine as Context;
use crate::error_recovery::{RecoveryAction, RecoveryContext, RecoveryEngine};
use crate::hooks::HookRegistry;
use crate::permission::PermissionPolicy;
use crate::runtime::approval::ApprovalResolver;
use crate::runtime::brain::Brain;
use crate::runtime::command::RunCommand;
use crate::runtime::event::{Envelope, RunEvent, RunId};
use crate::runtime::guard::EventGuard;
use crate::runtime::state::RunState;
use crate::runtime::supervisor::ProcessSupervisor;
use crate::tools::ToolRegistry;
use crate::types::{Message, MessageDelta, Role, StreamEvent, ToolCall, ToolExecutionMode};

/// Result of running a single turn.
enum TurnOutcome {
    Final(String),
    Continue,
    Stop(String),
}

/// Outcome of a recovery attempt within model_turn.
enum RecoveryOutcome {
    Retry,
    GiveUp,
}

// ApprovalResolver is now in runtime/approval.rs — Run holds a clone.

/// A Run is the independent execution space for a single user request.
///
/// It is constructed by [`crate::runtime::RunManager`] and runs on its own
/// tokio task. Communication with the outside world is via commands (in)
/// and events (out).
pub struct Run {
    pub id: RunId,
    pub session_id: Option<String>,

    /// The shared, reusable brain.
    brain: Arc<Brain>,

    // ── Per-Run state (not shared) ────────────────────────────────
    state: RunState,
    context: Context,
    client: OpenAIClient,
    registry: ToolRegistry,
    permission_policy: PermissionPolicy,
    hook_registry: HookRegistry,
    recovery: RecoveryEngine,
    recovery_ctx: RecoveryContext,
    context_processors: Vec<ContextProcessor>,
    tool_execution_mode: ToolExecutionMode,

    // ── Communication channels ────────────────────────────────────
    cmd_rx: mpsc::Receiver<RunCommand>,
    event_tx: broadcast::Sender<Envelope>,
    /// Monotonic per-Run sequence counter (shared with RunManager).
    seq: Arc<AtomicU64>,
    /// The active turn's id (R7). Set when a turn starts, cleared on Run end.
    /// Events emitted during a turn carry this so the frontend can route by id
    /// instead of guessing the "active turn".
    current_turn_id: Option<String>,

    // ── Cancellation & process management ─────────────────────────
    cancel: CancellationToken,
    supervisor: Arc<Mutex<ProcessSupervisor>>,
    join_set: JoinSet<()>,

    // ── Queues for mid-run injection ──────────────────────────────
    steering_queue: VecDeque<Message>,

    // ── Pending approvals (per-Run, not global) ───────────────────
    approval_resolver: ApprovalResolver,

    // ── Configuration ─────────────────────────────────────────────
    max_iterations: usize,
    /// Working directory for this Run. When set, tools that don't specify
    /// an explicit working_dir will execute here instead of the process CWD.
    /// Used for worktree isolation — each concurrent Run can work in its own
    /// git worktree without file conflicts.
    working_dir: Option<String>,
}

impl Run {
    /// Construct a new Run (called by RunManager, not by users directly).
    pub(crate) fn new(
        id: RunId,
        session_id: Option<String>,
        brain: Arc<Brain>,
        model_config: ModelConfig,
        cmd_rx: mpsc::Receiver<RunCommand>,
        event_tx: broadcast::Sender<Envelope>,
        seq: Arc<AtomicU64>,
        working_dir: Option<String>,
        history: Vec<crate::types::Message>,
    ) -> Result<Self> {
        let client = brain.build_client()?;
        let permission_policy = brain.build_permission_policy();
        let recovery = brain.build_recovery();

        let identity = brain.identity_text();
        let max_context_tokens = model_config.max_context_tokens;
        let max_iterations = model_config.max_iterations;

        // Build the supervisor BEFORE the registry so we can inject it
        // into the BashTool. The supervisor is owned by the Run and
        // shared with the tool via Arc<Mutex>.
        let supervisor = Arc::new(Mutex::new(ProcessSupervisor::new()));
        let working_dir_for_tool = working_dir.clone();

        let mut registry = brain.build_tool_registry();
        // Replace the default BashTool with a supervised version
        registry.register(Box::new(crate::tools::bash::BashTool::with_supervisor(
            supervisor.clone(),
            working_dir_for_tool,
        )));

        let mut context = Context::new(&identity, max_context_tokens);

        // Segment 2: PRINCIPLES
        let perm_mode = format!("{:?}", permission_policy.mode());
        let principles = brain.principles_text(&perm_mode);
        context.set_principles(&principles);

        // Segment 3: ENVIRONMENT — use working_dir if set, else process CWD
        let cwd = working_dir.clone().or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
        });
        let env_str = Context::build_environment_string(cwd.as_deref(), None, None);
        context.set_environment(&env_str);

        // Segment 4: TOOL CATALOG
        let tool_defs = registry.tool_definitions();
        let danger_map = build_danger_map(&tool_defs, &permission_policy);
        let tool_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
        context.set_tool_catalog(&tool_catalog);

        // Segment 5: ACTIVE MEMORY
        // agverse.md is loaded per-turn in refresh_context_segments.
        // In Stateless mode, inject the stateless prompt.
        {
            let mode = brain.memory_mode();
            if mode == crate::config::MemoryMode::Stateless {
                context.set_active_memory(crate::prompt::memory_mode_prompt(&mode));
            }
        }

        let recovery_ctx = RecoveryContext::new(&model_config.model_id, max_context_tokens);
        let hooks = brain.build_hooks();

        // Populate history
        for msg in history {
            context.add(msg);
        }

        Ok(Self {
            id,
            session_id,
            brain,
            state: RunState::Created,
            context,
            client,
            registry,
            permission_policy,
            hook_registry: hooks,
            recovery,
            recovery_ctx,
            context_processors: Vec::new(),
            tool_execution_mode: ToolExecutionMode::Parallel,
            cmd_rx,
            event_tx,
            seq,
            current_turn_id: None,
            cancel: CancellationToken::new(),
            supervisor,
            join_set: JoinSet::new(),
            steering_queue: VecDeque::new(),
            approval_resolver: ApprovalResolver::new(),
            working_dir,
            max_iterations,
        })
    }

    // ── Public accessors ──────────────────────────────────────────

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn context_messages(&self) -> Vec<Message> {
        self.context.messages()
    }

    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }

    /// The working directory for this Run (if set via worktree isolation).
    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    /// Access the per-Run approval resolver (used by RunManager for direct resolution).
    pub fn approval_resolver(&self) -> &ApprovalResolver {
        &self.approval_resolver
    }

    // ── State machine helpers ─────────────────────────────────────

    fn transition(&mut self, to: RunState) {
        let from = self.state;
        self.state = to;
        let _ = self
            .event_tx
            .send(self.wrap(RunEvent::StateChanged { from, to }));
    }

    fn emit(&mut self, event: RunEvent) {
        // Broadcast to subscribers (stamped with seq + event_id).
        // Persistence is handled by a subscriber task in RunManager, so that
        // streaming events (which bypass emit()) are also logged.
        let _ = self.event_tx.send(self.wrap(event));
    }

    /// Stamp a [`RunEvent`] with a fresh `seq` + `event_id`, producing an
    /// [`Envelope`]. Safe to call from `&self` contexts (e.g. streaming)
    /// because `seq` is an `Arc<AtomicU64>`.
    fn wrap(&self, event: RunEvent) -> Envelope {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        Envelope {
            seq,
            event_id: uuid::Uuid::new_v4().to_string(),
            run_id: self.id.clone(),
            turn_id: self.current_turn_id.clone(),
            parent_call_id: None,
            event,
        }
    }

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

        // Add user message to context
        self.context.add(Message::user(user_input));

        // Store in memory if enabled
        if let Some(ref mem) = self.brain.memory {
            if self.brain.memory_mode() != crate::config::MemoryMode::Stateless {
                let m = mem.lock();
                let _ = m.store_conversation("user", user_input);
            }
        }

        // Feed to reflection daemon (non-blocking, Deep mode only)
        if let Some(ref daemon) = self.brain.reflection_daemon {
            daemon.try_send("user", user_input);
        }

        // Skill auto-trigger: check user message against skill triggers
        if let Some(ref sm) = self.brain.skill_manager {
            let matched: Vec<(String, String)> = {
                let mgr = sm.lock();
                let matched = mgr.check_triggers(user_input);
                let mut result = Vec::new();
                for skill in &matched {
                    if let Ok(content) = mgr.load_content(skill) {
                        result.push((
                            skill.name.clone(),
                            format!(
                                "== Skill: {} (v{}) ==\n{}\n== End Skill: {} ==\n",
                                skill.name, skill.version, content, skill.name
                            ),
                        ));
                    }
                }
                result
            };

            for (_, text) in &matched {
                self.context.set_loaded_skills(text);
            }

            let mut mgr = sm.lock();
            for (name, _) in &matched {
                mgr.activate(name);
            }
        }

        // Run the loop
        let result = self.run_loop().await;

        match result {
            Ok(text) => {
                // Auto-session-summary: write a brief memory file
                self.write_session_memory(&text);

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

    async fn run_loop(&mut self) -> Result<String, RunError> {
        for turn_index in 0..self.max_iterations {
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
            self.hook_registry.fire_turn_start(turn_index);

            match self.run_turn(turn_index).await {
                Ok(TurnOutcome::Final(text)) => return Ok(text),
                Ok(TurnOutcome::Continue) => {}
                Ok(TurnOutcome::Stop(msg)) => return Ok(msg),
                Err(RunError::Cancelled) => return Err(RunError::Cancelled),
                Err(RunError::Failed(e)) => return Err(RunError::Failed(e)),
            }
            // Turn ended — clear the active turn id.
            self.current_turn_id = None;
        }

        let summary = build_iteration_limit_summary(&self.context, self.max_iterations);
        Err(RunError::Failed(summary))
    }

    /// Non-blocking poll of the command channel.
    fn poll_commands(&mut self) -> Result<(), RunError> {
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
                RunCommand::Steer { message } => {
                    self.steering_queue.push_back(Message::user(&message));
                }
                RunCommand::Approve { prompt_id, choice } => {
                    self.resolve_approval(&prompt_id, choice);
                }
                RunCommand::Answer { prompt_id, answer } => {
                    self.resolve_input(&prompt_id, &answer);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Block until Resume or Cancel is received.
    async fn wait_for_resume(&mut self) -> Result<(), RunError> {
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
                Some(RunCommand::Steer { message }) => {
                    self.steering_queue.push_back(Message::user(&message));
                }
                Some(RunCommand::Approve { prompt_id, choice }) => {
                    self.resolve_approval(&prompt_id, choice);
                }
                Some(RunCommand::Answer { prompt_id, answer }) => {
                    self.resolve_input(&prompt_id, &answer);
                }
                _ => {}
            }
        }
    }

    fn resolve_approval(&mut self, prompt_id: &str, choice: crate::permission::ApprovalChoice) {
        // Try the per-Run resolver first (used when ToolOrchestrator has
        // approval_resolver set, which is the new default path).
        if self.approval_resolver.resolve(prompt_id, choice.clone()) {
            eprintln!("[resolve_approval] resolved via per-Run resolver pid={}", prompt_id);
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
                eprintln!("[resolve_approval] resolved via global map pid={}", prompt_id);
                let _ = tx.send(choice.clone());
                self.emit(RunEvent::ApprovalResolved {
                    prompt_id: prompt_id.to_string(),
                    choice,
                });
                return;
            }
        }
        eprintln!("[resolve_approval] NOT found pid={}", prompt_id);
    }

    fn resolve_input(&mut self, _prompt_id: &str, _answer: &str) {
        // TODO: implement input request mechanism (future phase)
    }

    // ── Turn execution ────────────────────────────────────────────

    async fn run_turn(&mut self, turn_index: usize) -> Result<TurnOutcome, RunError> {
        // Stage: Refresh
        self.refresh_context_segments();

        // Stage: Compact (on-demand only — avoid per-turn cache invalidation)
        self.maybe_compact().await;

        // Stage: Model
        let (text, tool_calls, message_id) = match self.model_turn().await {
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
            self.hook_registry.fire_turn_end(turn_index);

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
                        let result = consolidator.consolidate();
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

            // Process steering messages
            while let Some(steer_msg) = self.steering_queue.pop_front() {
                self.context.add(steer_msg);
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
                hook_registry: &mut self.hook_registry,
                tool_execution_mode: self.tool_execution_mode,
                cancel_token: self.cancel.clone(),
                approval_resolver: Some(approval_resolver),
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
                .add(Message::tool(call.id.clone(), result.clone()));
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
            let items: Vec<crate::runtime::event::TodoItemPayload> = self
                .brain
                .todo_list
                .lock()
                .items
                .iter()
                .map(|item| crate::runtime::event::TodoItemPayload {
                    id: item.id.clone(),
                    description: item.description.clone(),
                    status: item.status.to_string(),
                })
                .collect();
            self.emit(RunEvent::TodoUpdated { items });
        }

        self.emit(RunEvent::TurnEnded { index: turn_index });
        self.hook_registry.fire_turn_end(turn_index);

        // Process steering messages (injected before next LLM call)
        while let Some(steer_msg) = self.steering_queue.pop_front() {
            self.context.add(steer_msg);
        }

        if turn_index == self.max_iterations - 1 {
            let summary = build_iteration_limit_summary(&self.context, self.max_iterations);
            self.emit(RunEvent::Error {
                message: summary.clone(),
            });
            return Ok(TurnOutcome::Stop(summary));
        }

        Ok(TurnOutcome::Continue)
    }

    // ── Model interaction ────────────────────────────────────────

    async fn model_turn(&mut self) -> Result<(String, Vec<ToolCall>, String), String> {
        const MAX_RECOVERY_ATTEMPTS: u32 = 3;

        for _attempt in 0..MAX_RECOVERY_ATTEMPTS {
            if self.cancel.is_cancelled() {
                return Err("aborted".to_string());
            }

            let messages = self.build_messages();
            let tools = self.registry.tool_definitions();

            // BeforeModel hook: SkipModel short-circuit
            let snapshot = self.snapshot_messages_for_hook(&messages);
            if let Some(preset) = self.hook_registry.fire_before_model(&snapshot) {
                self.recovery_ctx.record_success();
                self.hook_registry.fire_after_model(&preset, 0);
                return Ok((preset, Vec::new(), uuid::Uuid::new_v4().to_string()));
            }

            let stream = self
                .client
                .chat_completion_stream(&messages, &tools)
                .await
                .map_err(|e| format!("LLM request failed: {e}"))?;

            let collected: Result<(String, Vec<ToolCall>, String), String> = {
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
                Ok((text, tool_calls, message_id)) => {
                    self.recovery_ctx.record_success();
                    self.hook_registry.fire_after_model(&text, tool_calls.len());

                    return Ok((text, tool_calls, message_id));
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

    async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_tx: &broadcast::Sender<Envelope>,
    ) -> Result<(String, Vec<ToolCall>, String)> {
        let mut text_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;
        // Token accumulator: batches text/thinking deltas to cut IPC traffic.
        let mut tokens = TokenAccumulator::new();
        // Stable id for this model response; carried on every streaming delta
        // so the frontend routes by identity instead of position.
        let message_id = uuid::Uuid::new_v4().to_string();

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            if self.cancel.is_cancelled() {
                bail!("aborted");
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

        Ok((text_buffer, tool_calls, message_id))
    }

    /// Force an LLM compaction of the oldest turns regardless of current token
    /// count. Used by the recovery path when the model returns a context-too-long
    /// error. Falls back to `micro_compact` if the LLM call or JSON parse fails.
    async fn force_compact(&mut self, target_ratio: f64) {
        let remove_fraction = (1.0 - target_ratio).clamp(0.1, 0.6);
        let num_turns = (self.context.len().max(4) as f64 * remove_fraction) as usize;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }

    async fn try_recover(&mut self, error: &str) -> RecoveryOutcome {
        let action = self.recovery.determine_strategy(&self.recovery_ctx);
        match action {
            RecoveryAction::CompactContext { target_ratio } => {
                self.emit(RunEvent::Error {
                    message: format!(
                        "context too long; compacting to {:.0}% before retry",
                        target_ratio * 100.0
                    ),
                });
                self.force_compact(target_ratio).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::EscalateTokens { new_max_tokens } => {
                self.emit(RunEvent::Error {
                    message: format!("escalating max_tokens to {new_max_tokens}"),
                });
                self.client.set_max_tokens(new_max_tokens);
                RecoveryOutcome::Retry
            }
            RecoveryAction::Retry { delay_ms } => {
                self.emit(RunEvent::Error {
                    message: format!("retrying model call after {delay_ms}ms"),
                });
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::SwitchModel { model } => {
                self.emit(RunEvent::Error {
                    message: format!("switching to fallback model: {model}"),
                });
                // Model switching at runtime is complex — for now, just give up
                RecoveryOutcome::GiveUp
            }
            RecoveryAction::Fail => RecoveryOutcome::GiveUp,
        }
    }

    // ── Context management ────────────────────────────────────────

    fn build_messages(&self) -> Vec<Message> {
        let mut messages = self.context.messages();
        for processor in &self.context_processors {
            messages = (processor.transform)(messages);
        }
        messages
    }

    fn snapshot_messages_for_hook(&self, messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let content = m.content.as_deref().unwrap_or("");
                let preview = if content.len() > 500 {
                    let end = content.floor_char_boundary(500);
                    format!("{}...", &content[..end])
                } else {
                    content.to_string()
                };
                serde_json::json!({
                    "role": format!("{:?}", m.role),
                    "preview": preview
                })
            })
            .collect()
    }

    fn refresh_context_segments(&mut self) {
        // Segment 3: ENVIRONMENT — use working_dir if set
        let cwd = self.working_dir.clone().or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
        });
        let env_str = Context::build_environment_string(cwd.as_deref(), None, None);
        self.context.set_environment(&env_str);

        // Segment 4: TOOL CATALOG
        let tool_defs = self.registry.tool_definitions();
        let danger_map = build_danger_map(&tool_defs, &self.permission_policy);
        let tool_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
        self.context.set_tool_catalog(&tool_catalog);

        // Segment 5: ACTIVE MEMORY — project instructions + core memory + recall search
        {
            let mut mem_str = String::new();

            // Memory mode prompt (guides agent on how to use memory in this mode)
            let mode = self.brain.memory_mode();
            let mode_prompt = crate::prompt::memory_mode_prompt(&mode);
            mem_str.push_str(mode_prompt);
            mem_str.push_str("\n\n");

            // In Stateless mode, skip all project instructions and memory injection
            if mode != crate::config::MemoryMode::Stateless {
            // Layered project instructions (agverse.md):
            //   1. Global:   ~/.agverse/agverse.md
            //   2. Project:  {cwd}/agverse.md  (or AGENTS.md)
            //   3. Local:    {cwd}/agverse.local.md  (gitignore, personal)
            //   4. Rules:    {cwd}/.agverse/rules/*.md  (modular, path-scoped later)
            let cwd = self.working_dir.clone().or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.to_str().map(|s| s.to_string()))
            });
            let mut instructions = Vec::new();

            // 1. Global
            let global_path = crate::paths::get_global_agverse_md_path();
            if let Ok(content) = std::fs::read_to_string(&global_path) {
                instructions.push(("Global Project".to_string(), content));
            }

            let mut global_local_path = global_path.clone();
            global_local_path.set_file_name("agverse.local.md");
            if let Ok(content) = std::fs::read_to_string(&global_local_path) {
                instructions.push(("Global User Preferences".to_string(), content));
            }

            // Extract recent conversation text to match path-scoped rules
            let conversation_text = self.context.messages().iter().rev().take(5)
                .filter_map(|m| m.content.as_deref())
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();

            // 2. Project root
            if let Some(ref dir) = cwd {
                for name in &["agverse.md", "AGENTS.md"] {
                    let path = std::path::Path::new(dir).join(name);
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        instructions.push((format!("Project ({name})"), content));
                        break;
                    }
                }

                // 3. Local (personal, gitignored)
                let local_path = std::path::Path::new(dir).join("agverse.local.md");
                if let Ok(content) = std::fs::read_to_string(&local_path) {
                    instructions.push(("Project Local".to_string(), content));
                }

                // 4. Rules directory (.agverse/rules/*.md) - Path Scoped
                let rules_dir = std::path::Path::new(dir).join(".agverse/rules");
                if rules_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&rules_dir) {
                        let mut rule_files: Vec<_> = entries
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.path().extension().is_some_and(|ext| ext == "md")
                            })
                            .collect();
                        rule_files.sort_by_key(|e| e.path());
                        for entry in rule_files {
                            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                let path = entry.path();
                                let name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("rule");

                                let name_lower = name.to_lowercase();
                                if name_lower == "global" || name_lower == "default" || conversation_text.contains(&name_lower) {
                                    instructions.push((format!("Rule: {name}"), content));
                                }
                            }
                        }
                    }
                }
            }

            if !instructions.is_empty() {
                let mut parts = Vec::new();
                for (label, content) in &instructions {
                    parts.push(format!("## {label}\n{content}"));
                }
                mem_str.push_str(&format!("Project Instructions:\n{}\n\n", parts.join("\n\n")));
            }

            } // end non-stateless block

            if !mem_str.is_empty() {
                self.context.set_active_memory(&mem_str);
            }
        }

        // Segment 6: LOADED SKILLS — catalog + active skill content
        if let Some(ref sm) = self.brain.skill_manager {
            let mgr = sm.lock();
            let catalog = mgr.build_catalog();
            let active = mgr.build_active_context();
            let mut skills_str = String::new();
            if !catalog.is_empty() {
                skills_str.push_str(&catalog);
            }
            if !active.is_empty() {
                if !skills_str.is_empty() {
                    skills_str.push_str("\n\n");
                }
                skills_str.push_str(&active);
            }
            if !skills_str.is_empty() {
                self.context.set_loaded_skills(&skills_str);
            }
        }

        // Segment 7: EXECUTION PLAN — inject current todo list
        let plan_str = self.brain.todo_list.lock().to_context_string();
        if !plan_str.is_empty() {
            self.context.set_execution_plan(&plan_str);
        }
    }

    /// Compact strategy (Stage: Compact).
    ///
    /// Two-tier approach optimized for DeepSeek prefix caching:
    ///
    /// 1. **Chunked drop (preferred)**: When token usage exceeds 80% of the
    ///    context window, batch-drop the oldest 50% of conversation turns.
    ///    This causes a single-turn cache miss but leaves a stable prefix
    ///    for the next 10+ turns — maximizing long-term cache hits with
    ///    zero LLM overhead.
    ///
    /// 2. **LLM summarize (fallback)**: If chunked_drop wasn't sufficient
    ///    (e.g., a single tool result is dominating), fall back to the
    ///    LLM-based summary compact with `micro_compact()` as last resort.
    async fn maybe_compact(&mut self) {
        const COMPACT_THRESHOLD: f64 = 0.80;
        const CHUNK_KEEP_RECENT: usize = 20;

        let current = self.context.current_token_count();
        let threshold =
            (self.client.model.max_context_tokens as f64 * COMPACT_THRESHOLD) as usize;

        if current < threshold {
            return;
        }

        // Tier 1: Chunked drop — zero-cost, cache-friendly bulk removal.
        let keep = (self.context.len() / 2).max(4).min(CHUNK_KEEP_RECENT);
        if self.context.chunked_drop(keep) > 0 {
            tracing::info!(
                compact = "chunked_drop",
                tokens_before = current,
                tokens_after = self.context.current_token_count(),
                "Chunked drop compact applied"
            );
            // Check if this brought us below the threshold.
            if self.context.current_token_count() < threshold {
                return;
            }
        }

        // Also run trim_to_fit for snip/dedup/chunk compression.
        let _result = self.context.trim_to_fit();

        if self.context.current_token_count() < threshold {
            return;
        }

        // Tier 2: LLM summarize — expensive, but handles pathological cases.
        let num_turns = self.context.len().max(4) * 2 / 5;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => return,
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                // LLM call failed — fallback to micro_compact
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => {
                tracing::info!(
                    compact = "llm_summary",
                    turns_summarized = num_turns,
                    "LLM compact applied"
                );
                s
            }
            Err(_) => {
                // JSON parse failed — fallback to micro_compact
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }

    // ── Session memory ────────────────────────────────────────────

    /// Write a brief memory block after Run completion into the global agverse.md.
    /// Extracts user messages and assistant final answers into a compact markdown format.
    fn write_session_memory(&self, _final_text: &str) {
        let global_path = crate::paths::get_global_agverse_md_path();
        
        // If the file already exists, we do nothing. We let the Agent intelligently manage it via tools.
        if global_path.exists() {
            return;
        }

        if let Some(parent) = global_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let template = "\
# OS-Level Memory Architecture

You operate using an OS-level memory management system. Proactively manage your own context window to prevent memory bloat and maintain long-term reasoning capabilities.

## Core Memory (RAM)
This file (`~/.agverse/agverse.md`) is your Core Memory. It is 100% injected into your context on every turn. It is your ultimate source of truth for Project Overview, Architecture Decisions, Coding Conventions, and User Preferences.

## Archival Memory (Disk)
An underlying SQLite database contains historical conversation logs (Recall Memory) and long-term knowledge (Archival Memory). Use `conversation_search` and `archival_memory_search` tools to retrieve information when needed.

## Memory Management Directives (CRITICAL)
Your primary responsibility is to keep this Core Memory highly condensed, strictly structured, and up-to-date. You are forbidden from allowing this file to become a dump for raw conversational logs.

When you interact with the user, silently evaluate if the new information alters the global state:
- **Write/Replace:** If the user makes a new architectural decision, introduces a coding convention, or updates a global preference, use the `edit` tool to update the corresponding section below.
- **Compaction (Delete):** If an existing rule is deprecated, overridden, or resolved, delete or overwrite the old information to free up Core Memory space. Do not append contradictory rules.
- **Offload to Archival (Ignore):** If the information is a specific debugging step, a one-off script, or a transient thought, do NOT write it here. Trust that Archival Memory will capture it for future retrieval.

Never mention your memory management process to the user unless explicitly asked. Maintain a seamless conversational flow while quietly updating this file in the background.

---

# Project Overview

# Tech Stack & Commands

# Architecture Decisions

# Coding Conventions

# User Preferences

# Agent Instructions
";

        if let Err(e) = std::fs::write(&global_path, template) {
            tracing::warn!("failed to write agverse.md template: {e}");
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────

    /// Cancel-path cleanup: kill all processes, abort all tasks.
    async fn cancel_and_cleanup(&mut self) {
        // 1. Trigger cancellation (propagates to model stream + tool exec)
        self.cancel.cancel();

        // 2. Abort all background tasks (subagent, memory consolidation, etc.)
        self.join_set.abort_all();
        while self.join_set.join_next().await.is_some() {}

        // 3. Kill all child processes
        {
            let mut sup: MutexGuard<'_, ProcessSupervisor> = self.supervisor.lock();
            sup.kill_all();
        }

        // 4. Drop all pending approvals (resolvers get a dropped error)
        self.approval_resolver.clear();

        // 5. Clear queues
        self.steering_queue.clear();
    }

    /// Final cleanup (called on all exit paths, idempotent).
    fn cleanup_on_exit(&mut self) {
        // Kill any remaining processes (idempotent if already killed)
        {
            let mut sup: MutexGuard<'_, ProcessSupervisor> = self.supervisor.lock();
            sup.kill_all();
        }

        // Abort any remaining tasks
        self.join_set.abort_all();

        // Drop pending approvals
        self.approval_resolver.clear();
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // RAII safety net: even if cancel_and_cleanup wasn't called,
        // this ensures no leaks.
        self.cancel.cancel();
        self.join_set.abort_all();
        // Kill all supervised processes
        self.supervisor.lock().kill_all();
        // approval_resolver.clear() is called above (resolvers get dropped error)
    }
}

/// Error type for the run loop.
enum RunError {
    Cancelled,
    Failed(String),
}

/// Default directory for Run event logs.
pub(crate) fn default_runs_dir() -> String {
    crate::paths::get_runs_dir().to_string_lossy().into_owned()
}

// ── Helper functions (copied from agent/mod.rs for now) ───────────

fn build_danger_map(
    tools: &[crate::types::ToolDefinition],
    policy: &PermissionPolicy,
) -> HashMap<String, crate::permission::DangerLevel> {
    let mut map = HashMap::new();
    for tool in tools {
        let danger = policy.danger_level_for(&tool.function.name, "{}", None);
        map.insert(tool.function.name.clone(), danger);
    }
    map
}

fn build_iteration_limit_summary(context: &Context, max_iterations: usize) -> String {
    let messages = context.messages();

    let user_request = messages
        .iter()
        .rfind(|m| m.role == Role::User)
        .map(|m| m.content.as_deref().unwrap_or(""))
        .unwrap_or("your request");

    let mut tool_counts: HashMap<&str, usize> = HashMap::new();
    let mut total_tool_calls = 0;

    for msg in &messages {
        if msg.role == Role::Assistant {
            if let Some(ref calls) = msg.tool_calls {
                for call in calls {
                    *tool_counts.entry(&call.function.name).or_insert(0) += 1;
                    total_tool_calls += 1;
                }
            }
        }
    }

    let tool_summary: Vec<String> = tool_counts
        .iter()
        .map(|(name, count)| format!("  - {name}: {count} time(s)"))
        .collect();

    let mut msg = format!(
        "Reached the maximum number of steps ({max_iterations}) while working on: \"{user_request}\"\n\n",
    );

    if !tool_summary.is_empty() {
        msg.push_str(&format!("Tools used:\n{}\n\n", tool_summary.join("\n")));
    }

    if total_tool_calls == 0 {
        msg.push_str("No actions were taken. This may indicate a tool availability or model configuration issue.");
    } else {
        msg.push_str(&format!(
            "The task may be too complex for {max_iterations} steps. Try breaking it down or providing more specific instructions."
        ));
    }

    msg
}
