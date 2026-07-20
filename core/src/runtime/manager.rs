//! RunManager — creates and tracks Runs, routes commands.
//!
//! The RunManager owns the shared [`Brain`] and a map of active [`Run`]s.
//! It is the primary interface for the frontend/CLI bridge layer.
//!
//! ## Lifecycle
//!
//! ```text
//! create_run() → RunId
//!   ├── command(run_id, Start)        → Run begins executing
//!   ├── command(run_id, Steer{...})   → Inject mid-run message
//!   ├── command(run_id, Cancel)       → Kill the run + all children
//!   └── subscribe(run_id)             → Get event stream
//! ```

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mode::AgentMode;
use crate::permission::ApprovalChoice;
use crate::reflector::Reflector;
use crate::runtime::approval::ApprovalResolver;
use crate::runtime::input::{ClarificationAnswers, InputResolver};
use crate::runtime::brain::Brain;
use crate::runtime::command::RunCommand;
use crate::runtime::event::{Envelope, RunEvent, RunId};
use crate::runtime::event_log::EventLog;
use crate::runtime::run::{Run, default_runs_dir};
use crate::runtime::state::RunState;
use crate::session::SessionManager;
use crate::worktree::WorktreeManager;
use crate::types::Message;
use crate::context::ContextUsageSnapshot;

/// Capacity for the event broadcast channel per Run.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Capacity for the command channel per Run.
const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// Result of [`RunManager::create_run`] / [`RunManager::create_run_with_workdir`].
///
/// `prompt_id` is the canonical `prompts` table id when a session is attached
/// and a [`SessionManager`] is configured. Clients (Tauri, CLI) must use this
/// for transcript rewind — never the ephemeral `run_id`.
#[derive(Debug, Clone)]
pub struct CreateRunResult {
    pub run_id: RunId,
    pub prompt_id: Option<String>,
}

/// A handle to an active (or recently completed) Run.
pub struct RunHandle {
    pub id: RunId,
    pub session_id: Option<String>,
    /// Canonical session prompt id (source of truth for rewind), if any.
    pub prompt_id: Option<String>,
    /// Sender for commands (Start, Pause, Cancel, Steer, Approve, etc.)
    cmd_tx: mpsc::Sender<RunCommand>,
    /// Broadcast sender for events. Subscribers call `.subscribe()` on this.
    pub event_tx: broadcast::Sender<Envelope>,
    /// The tokio task running the Run's loop.
    join_handle: Option<JoinHandle<()>>,
    /// Shared state for querying (read-only, updated by the Run task).
    state: Arc<RwLock<RunState>>,
    /// Per-Run approval resolver — resolved directly by `approve_tool`
    /// to avoid actor deadlock (bypassing the command channel).
    pub approval_resolver: ApprovalResolver,
    /// Per-Run clarification resolver — resolved directly by `answer_input`.
    pub input_resolver: InputResolver,
    /// CancellationToken — cancelled immediately on `cancel_run()` so that
    /// hot-path checks in collect_stream() and ToolOrchestrator respond
    /// without waiting for the next poll_commands() turn boundary.
    pub cancel_token: CancellationToken,
    /// Shared context snapshot (refreshed by the Run at turn boundaries).
    context_snapshot: Arc<RwLock<Vec<Message>>>,
    /// Shared context usage breakdown (refreshed with context_snapshot).
    usage_snapshot: Arc<RwLock<ContextUsageSnapshot>>,
}

impl RunHandle {
    /// Send a command to the Run.
    pub fn command(&self, cmd: RunCommand) -> Result<()> {
        self.cmd_tx
            .try_send(cmd)
            .context("failed to send command to run (channel full or closed)")
    }

    /// Subscribe to the Run's event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    /// Current state of the Run (best-effort, may be slightly stale).
    pub fn state(&self) -> RunState {
        *self.state.read()
    }

    /// Whether the Run has finished (terminal state).
    pub fn is_done(&self) -> bool {
        self.state().is_terminal()
    }

    /// Best-effort read-only snapshot of the Run's current context messages.
    /// Refreshed at turn boundaries; may be slightly stale during a turn.
    /// Used by side-channel `/btw` queries that must not touch the main Run.
    pub fn context_snapshot(&self) -> Vec<Message> {
        self.context_snapshot.read().clone()
    }

    /// Best-effort read-only context usage breakdown.
    pub fn usage_snapshot(&self) -> ContextUsageSnapshot {
        self.usage_snapshot.read().clone()
    }

    /// Wait for the Run's task to complete.
    pub async fn join(mut self) -> Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle.await.context("run task panicked")?;
        }
        Ok(())
    }
}

/// The RunManager — owns the Brain and tracks all active Runs.
pub struct RunManager {
    brain: Arc<Brain>,
    runs: Mutex<HashMap<RunId, RunHandle>>,
    /// Optional session store. When set, create_run allocates a canonical
    /// prompt row and terminal events persist the transcript against it.
    session_manager: Option<Arc<SessionManager>>,
}

impl RunManager {
    /// Create a RunManager from a loaded Config.
    pub fn new(brain: Brain) -> Self {
        Self {
            brain: Arc::new(brain),
            runs: Mutex::new(HashMap::new()),
            session_manager: None,
        }
    }

    /// Attach the shared session store so prompt lifecycle is owned here
    /// (Tauri, CLI, and eval all see the same prompt_id source of truth).
    pub fn with_session_manager(mut self, session_manager: Arc<SessionManager>) -> Self {
        self.session_manager = Some(session_manager);
        self
    }

    /// Load config from a file and create a RunManager.
    pub fn load_config(path: &str) -> Result<Self> {
        let brain = Brain::load_config(path)?;
        Ok(Self::new(brain))
    }

    /// Access the shared Brain (for model switching, config queries, etc.)
    pub fn brain(&self) -> &Arc<Brain> {
        &self.brain
    }

    /// Mutable access to the runs map (used by Agent wrapper to remove handles).
    pub async fn runs_mut(&self) -> tokio::sync::MutexGuard<'_, HashMap<RunId, RunHandle>> {
        self.runs.lock().await
    }

    /// Look up the active (non-terminal) Run for a session and return a
    /// best-effort read-only snapshot of its context messages. Used by
    /// side-channel `/btw` queries that must not touch the main Run.
    pub async fn context_snapshot_for_session(&self, session_id: &str) -> Option<Vec<Message>> {
        let runs = self.runs.lock().await;
        for handle in runs.values() {
            if handle.session_id.as_deref() == Some(session_id) && !handle.is_done() {
                return Some(handle.context_snapshot());
            }
        }
        None
    }

    /// Canonical raw transcript for one Run. Dynamic context segments are not
    /// included, so the result is safe to persist as future model history.
    pub async fn context_snapshot_for_run(&self, run_id: &str) -> Option<Vec<Message>> {
        self.runs
            .lock()
            .await
            .get(run_id)
            .map(RunHandle::context_snapshot)
    }

    /// Context usage for a session's most recent run.
    /// Prefers an active (non-terminal) run; falls back to a completed run
    /// that has not been reaped yet.
    pub async fn context_usage_for_session(
        &self,
        session_id: &str,
    ) -> Option<ContextUsageSnapshot> {
        let runs = self.runs.lock().await;
        let mut completed: Option<ContextUsageSnapshot> = None;
        for handle in runs.values() {
            if handle.session_id.as_deref() != Some(session_id) {
                continue;
            }
            if !handle.is_done() {
                return Some(handle.usage_snapshot());
            }
            completed = Some(handle.usage_snapshot());
        }
        completed
    }

    /// Context usage for a specific run.
    pub async fn context_usage_for_run(&self, run_id: &str) -> Option<ContextUsageSnapshot> {
        let runs = self.runs.lock().await;
        runs.get(run_id).map(RunHandle::usage_snapshot)
    }

    /// Estimate context usage from persisted session messages when no live
    /// Run exists (idle / resumed old sessions). Rebuilds a lightweight
    /// Context with identity + principles + tool catalog + conversation so
    /// the ring is not stuck at 0%.
    pub fn estimate_usage_from_messages(&self, messages: &[Message]) -> ContextUsageSnapshot {
        use crate::context::Context;
        use std::collections::HashMap;

        let max = self.current_max_context_tokens();
        let brain = self.brain();
        let mut ctx = Context::new(&brain.identity_text(), max);

        let perm = format!("{:?}", brain.build_permission_policy().mode());
        ctx.set_principles(&brain.principles_text(&perm));

        let registry = brain.build_tool_registry(brain.mode());
        let defs = registry.tool_definitions();
        let empty_danger: HashMap<String, crate::permission::DangerLevel> = HashMap::new();
        let catalog = Context::build_tool_catalog_string(&defs, &empty_danger);
        ctx.set_tool_catalog(&catalog);

        for msg in messages {
            ctx.add(msg.clone());
        }
        ctx.usage_snapshot()
    }

    /// Resolved max context for the current default model (registry-aware).
    pub fn current_max_context_tokens(&self) -> usize {
        let brain = self.brain();
        let name = &brain.config.default_model;
        brain
            .config
            .get_model(name)
            .map(|m| m.max_context_tokens)
            .unwrap_or(crate::model_capabilities::DEFAULT_CONTEXT_TOKENS)
    }

    /// The current agent mode. New Runs inherit this mode.
    pub fn mode(&self) -> AgentMode {
        self.brain.mode()
    }

    /// Set the agent mode. Takes effect on the next Run — existing Runs
    /// keep their mode. The frontend should update its mode indicator
    /// after calling this.
    pub fn set_mode(&self, mode: AgentMode) {
        self.brain.set_mode(mode);
    }

    /// Create a new Run for a user request.
    ///
    /// Returns [`CreateRunResult`]. The Run starts in `Created` state — call
    /// `command(run_id, RunCommand::Start)` to begin execution.
    pub async fn create_run(
        &self,
        user_input: &str,
        session_id: Option<String>,
        history: Vec<crate::types::Message>,
    ) -> Result<CreateRunResult> {
        self.create_run_with_workdir(user_input, session_id, None, history, None, false)
            .await
    }

    /// Create a Run with an isolated working directory (for worktree isolation).
    /// When `working_dir` is set, the Run's tools execute in that directory
    /// instead of the process CWD, allowing multiple concurrent Runs to work
    /// in separate git worktrees without file conflicts.
    ///
    /// `initial_goal` / `initial_goal_completed` seed a session-level pinned goal
    /// so follow-up messages (after Stop) still inject PRIMARY GOAL.
    pub async fn create_run_with_workdir(
        &self,
        user_input: &str,
        session_id: Option<String>,
        working_dir: Option<String>,
        history: Vec<crate::types::Message>,
        initial_goal: Option<String>,
        initial_goal_completed: bool,
    ) -> Result<CreateRunResult> {
        let run_id = uuid::Uuid::new_v4().to_string();

        // Canonical prompt row — sole source of truth for session rewind.
        let prompt_id: Option<String> = if let (Some(sid), Some(sm)) =
            (session_id.as_ref(), self.session_manager.as_ref())
        {
            let sm = sm.clone();
            let sid = sid.clone();
            let model = self
                .brain
                .current_model_config()
                .map(|m| m.model_id.clone())
                .unwrap_or_else(|_| "unknown".to_string());
            let result = tokio::task::spawn_blocking(move || sm.create_prompt(&sid, &model))
                .await
                .context("create_prompt task failed")?
                .context("failed to create prompt")?;
            Some(result.0)
        } else {
            None
        };

        // Get the current model config
        let model_config = self.brain.current_model_config()?;

        // Create channels
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        // Producers write to one ordered mailbox. The writer persists each
        // envelope before publishing it to the lossy UI broadcast.
        let (producer_tx, mut producer_rx) = mpsc::unbounded_channel::<Envelope>();
        let (event_tx, _event_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        // Shared state for external querying
        let shared_state = Arc::new(RwLock::new(RunState::Created));

        // Monotonic per-Run sequence counter. Shared with the Run so that
        // RunCreated (seq 0) and the Run's own events form one sequence.
        let seq = Arc::new(AtomicU64::new(0));

        // Get the current mode from the brain
        let mode = self.brain.mode();

        // Shared context snapshot for side-channel `/btw` queries.
        let context_snapshot = Arc::new(RwLock::new(Vec::<Message>::new()));
        let max_ctx = model_config.max_context_tokens;
        let usage_snapshot = Arc::new(RwLock::new(ContextUsageSnapshot::empty(max_ctx)));

        // Create the Run
        let run = match Run::new(
            run_id.clone(),
            session_id.clone(),
            self.brain.clone(),
            model_config,
            cmd_rx,
            producer_tx.clone(),
            seq.clone(),
            working_dir,
            history,
            mode,
            context_snapshot.clone(),
            usage_snapshot.clone(),
            initial_goal,
            initial_goal_completed,
        ) {
            Ok(run) => run,
            Err(e) => {
                if let (Some(sm), Some(pid)) = (self.session_manager.as_ref(), prompt_id.as_ref()) {
                    let sm = sm.clone();
                    let pid = pid.clone();
                    let err = e.to_string();
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = sm.finish_prompt(
                            &pid,
                            "failed",
                            &serde_json::json!({ "setup_error": err }),
                        );
                    })
                    .await;
                }
                return Err(e);
            }
        };
        // Clone the approval resolver before spawning so we can store
        // it in the RunHandle for direct (non-command-channel) resolution.
        let approval_resolver = run.approval_resolver().clone();
        let input_resolver = run.input_resolver().clone();
        // Clone the cancel token so cancel_run() can trigger immediate
        // cancellation without waiting for the next poll_commands() cycle.
        let cancel_token = run.cancel_token();

        let writer_run_id = run_id.clone();
        let writer_broadcast = event_tx.clone();
        let writer_cancel = cancel_token.clone();
        let writer_state = shared_state.clone();
        let writer_snapshot = context_snapshot.clone();
        let writer_sm = self.session_manager.clone();
        let writer_session_id = session_id.clone();
        let writer_prompt_id = prompt_id.clone();
        let writer_handle = tokio::spawn(async move {
            let mut event_log = EventLog::new(&writer_run_id, &default_runs_dir());
            let mut next_seq = 0u64;
            while let Some(mut env) = producer_rx.recv().await {
                // Arrival at this single mailbox defines lifecycle order. This
                // closes the fetch_add/send scheduling race across producers.
                env.seq = next_seq;
                next_seq += 1;
                let terminal_status = match &env.event {
                    RunEvent::RunCompleted { .. } => Some("completed"),
                    RunEvent::RunCancelled { .. } => Some("cancelled"),
                    RunEvent::RunFailed { .. } => Some("failed"),
                    _ => None,
                };
                let is_terminal = terminal_status.is_some();
                if let Err(error) = event_log.append(env.clone()) {
                    tracing::error!(run_id = %writer_run_id, %error, "event writer failed; stopping publication");
                    writer_cancel.cancel();
                    *writer_state.write() = RunState::Failed;
                    let _ = writer_broadcast.send(Envelope {
                        seq: next_seq,
                        event_id: uuid::Uuid::new_v4().to_string(),
                        run_id: writer_run_id.clone(),
                        session_id: env.session_id.clone(),
                        turn_id: env.turn_id.clone(),
                        parent_call_id: env.parent_call_id.clone(),
                        ts: chrono::Utc::now(),
                        event: RunEvent::RunFailed {
                            error: format!("event log persistence failed: {error}"),
                        },
                    });
                    break;
                }

                // Persist transcript + finish prompt BEFORE broadcasting so
                // CLI and Tauri both see a consistent prompts-table identity.
                if let Some(status) = terminal_status {
                    if let (Some(sm), Some(sid), Some(pid)) = (
                        writer_sm.as_ref(),
                        writer_session_id.as_ref(),
                        writer_prompt_id.as_ref(),
                    ) {
                        let sm = sm.clone();
                        let sid = sid.clone();
                        let pid = pid.clone();
                        let messages = writer_snapshot.read().clone();
                        let status = status.to_string();
                        let _ = tokio::task::spawn_blocking(move || {
                            if !messages.is_empty() {
                                if let Err(e) =
                                    sm.save_canonical_transcript_for_prompt(&sid, &messages, &pid)
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to persist canonical transcript"
                                    );
                                }
                            }
                            if let Err(e) =
                                sm.finish_prompt(&pid, &status, &serde_json::json!({}))
                            {
                                tracing::warn!(error = %e, "failed to finish prompt");
                            }
                        })
                        .await;
                    }
                }

                let _ = writer_broadcast.send(env);
                if is_terminal {
                    break;
                }
            }
        });
        let _ = writer_handle;

        // Emit RunCreated through the writer, so it is always present in the
        // durable trace even when no frontend subscriber exists yet.
        let _ = producer_tx.send(Envelope {
            seq: seq.fetch_add(1, Ordering::Relaxed),
            event_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            turn_id: None,
            parent_call_id: None,
            ts: chrono::Utc::now(),
            event: RunEvent::RunCreated {
                id: run_id.clone(),
                session_id: session_id.clone(),
                prompt_id: prompt_id.clone(),
            },
        });

        // Continuous Learning: Diff Observer
        // Before starting a new run, we check if the user modified files that the previous run touched.
        if let Ok(runs) = crate::runtime::event_log::EventLog::list_runs(
            crate::paths::get_runs_dir().to_string_lossy().as_ref()
        ) {
            // Because the current run's log hasn't been written yet (the log task hasn't processed RunCreated),
            // the last run in the directory is truly the "previous" run.
            if let Some(prev_run_id) = runs.last() {
                let diffs = crate::reflector::diff_observer::DiffObserver::check_for_diffs(prev_run_id);
                if !diffs.is_empty() {
                    tracing::info!(diff_count = diffs.len(), "Diff Observer found manual edits to previous run's files");
                    if let Ok(client) = self.brain.build_client() {
                        crate::memory::diff_preference::DiffPreferenceEngine::spawn_analysis(
                            client,
                            diffs,
                            producer_tx.clone(),
                            seq.clone(),
                            run_id.clone(),
                        );
                    }
                }
            }
        }


        // Spawn the Run's task
        let user_input_owned = user_input.to_string();
        let state_clone = shared_state.clone();
        let event_tx_clone = event_tx.clone();
        let brain_for_reflect = self.brain.clone();
        let reflect_run_id = run_id.clone();
        let join_handle = tokio::spawn(async move {
            // We need to update shared_state as the Run progresses.
            // The Run owns its state internally, so we use a wrapper that
            // mirrors state changes via events.
            let mut rx = event_tx_clone.subscribe();
            let state_task = tokio::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    if let RunEvent::StateChanged { to, .. } = &ev.event {
                        let mut g = state_clone.write();
                        *g = *to;
                    }
                    if matches!(
                        ev.event,
                        RunEvent::RunCompleted { .. }
                            | RunEvent::RunCancelled { .. }
                            | RunEvent::RunFailed { .. }
                    ) {
                        break;
                    }
                }
            });

            run.run(&user_input_owned).await;
            // The state mirror task will exit when it sees the terminal event.
            // Give it a moment to process the last event.
            let _ = state_task.await;
            // Wait for the single writer to durably append the terminal event.
            let _ = writer_handle.await;

            // Offline reflection: analyze the Run's event log for improvement
            // suggestions. Only runs if the Brain has a Reflector configured.
            if let Some(ref reflector) = brain_for_reflect.reflector {
                let log_path = std::path::PathBuf::from(default_runs_dir())
                    .join(format!("{reflect_run_id}.jsonl"));
                match Reflector::load_event_log(&log_path).await {
                    Ok(events) => {
                        crate::reflector::diff_observer::DiffObserver::take_snapshot(&reflect_run_id, &events);
                        let suggestions = reflector.analyze(&events);
                        for sug in &suggestions {
                            match reflector.apply(sug).await {
                                Ok(crate::reflector::SuggestionAction::Applied) => {
                                    tracing::info!(
                                        suggestion = %sug.id,
                                        target = %sug.target,
                                        "reflector auto-applied skill"
                                    );
                                }
                                Ok(crate::reflector::SuggestionAction::NeedsApproval(diff)) => {
                                    tracing::info!(
                                        suggestion = %sug.id,
                                        "reflector suggestion needs approval: {diff}"
                                    );
                                    tracing::info!(suggestion = %sug.id, %diff, "reflector approval queued outside completed Run lifecycle");
                                }
                                Ok(crate::reflector::SuggestionAction::Forbidden) => {
                                    tracing::debug!(
                                        suggestion = %sug.id,
                                        "reflector suggestion forbidden"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        suggestion = %sug.id,
                                        "reflector apply error: {e}"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("reflector failed to load event log: {e}");
                    }
                }
            }
        });

        // Store the handle
        let handle = RunHandle {
            id: run_id.clone(),
            session_id,
            prompt_id: prompt_id.clone(),
            cmd_tx,
            event_tx,
            join_handle: Some(join_handle),
            state: shared_state,
            approval_resolver,
            input_resolver,
            cancel_token,
            context_snapshot,
            usage_snapshot,
        };

        self.runs.lock().await.insert(run_id.clone(), handle);

        Ok(CreateRunResult {
            run_id,
            prompt_id,
        })
    }

    /// Send a command to a specific Run.
    pub async fn command(&self, run_id: &str, cmd: RunCommand) -> Result<()> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        handle.command(cmd)
    }

    /// Subscribe to a Run's event stream.
    pub async fn subscribe(&self, run_id: &str) -> Result<broadcast::Receiver<Envelope>> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        Ok(handle.subscribe())
    }

    /// Get the current state of a Run.
    pub async fn run_state(&self, run_id: &str) -> Result<RunState> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        Ok(handle.state())
    }

    /// List all active Run IDs.
    pub async fn list_runs(&self) -> Vec<RunId> {
        self.runs.lock().await.keys().cloned().collect()
    }

    /// Cancel a specific Run.
    ///
    /// **Two-phase cancellation**: (1) the CancellationToken is cancelled
    /// immediately, so hot-path checks in collect_stream() and
    /// ToolOrchestrator bail out on the next iteration/boundary without
    /// waiting for poll_commands() to drain the command channel. (2) the
    /// RunCommand::Cancel is queued so that poll_commands() still performs
    /// the proper state transition to RunCancelled and cleanup on the next
    /// turn boundary.
    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        // Fast path: set the token immediately so in-flight work stops now.
        handle.cancel_token.cancel();
        // Slow path: queue the command for proper state transition.
        handle.command(RunCommand::Cancel)
    }

    /// Cancel all active Runs (used on app shutdown).
    pub async fn cancel_all(&self) {
        let runs = self.runs.lock().await;
        for handle in runs.values() {
            // Fast path: cancel the token immediately.
            handle.cancel_token.cancel();
            // Slow path: queue the command for state transition.
            let _ = handle.command(RunCommand::Cancel);
        }
    }

    /// Resolve a pending approval directly through the per-Run resolver.
    ///
    /// This bypasses the command channel to avoid actor deadlock: when a
    /// Run is blocked inside `run_turn()` waiting for an approval oneshot,
    /// it cannot process `RunCommand::Approve` from its command channel.
    /// By resolving the `ApprovalResolver` directly (shared via `RunHandle`),
    /// we wake the waiting oneshot without touching the command channel.
    ///
    /// Returns `true` if the approval was found and resolved.
    pub async fn resolve_approval(
        &self,
        run_id: Option<&str>,
        prompt_id: &str,
        choice: ApprovalChoice,
    ) -> bool {
        let runs = self.runs.lock().await;
        if let Some(id) = run_id {
            if let Some(handle) = runs.get(id) {
                return handle.approval_resolver.resolve(prompt_id, choice);
            }
        } else {
            for handle in runs.values() {
                if handle.approval_resolver.resolve(prompt_id, choice.clone()) {
                    return true;
                }
            }
        }
        false
    }

    /// Resolve a pending clarification directly through the per-Run resolver.
    ///
    /// Same deadlock-avoidance rationale as [`Self::resolve_approval`].
    pub async fn resolve_input(
        &self,
        run_id: Option<&str>,
        prompt_id: &str,
        answers: ClarificationAnswers,
    ) -> bool {
        let runs = self.runs.lock().await;
        if let Some(id) = run_id {
            if let Some(handle) = runs.get(id) {
                return handle.input_resolver.resolve(prompt_id, answers);
            }
        } else {
            for handle in runs.values() {
                if handle.input_resolver.resolve(prompt_id, answers.clone()) {
                    return true;
                }
            }
        }
        false
    }

    /// Remove completed Runs from the tracking map (garbage collection).
    pub async fn reap_completed(&self) -> usize {
        let mut runs = self.runs.lock().await;
        let before = runs.len();
        runs.retain(|_, h| !h.is_done());
        before - runs.len()
    }

    /// List all Run IDs that have persisted event logs (for replay/fork).
    pub fn list_logged_runs(&self) -> Result<Vec<RunId>> {
        let dir = crate::paths::get_runs_dir();
        EventLog::list_runs(&dir.to_string_lossy())
    }

    /// Load a persisted Run's event log for replay.
    pub fn load_run_log(&self, run_id: &str) -> Result<Vec<Envelope>> {
        let path = crate::paths::get_runs_dir().join(format!("{run_id}.jsonl"));
        EventLog::load(&path)
    }

    /// Replay envelopes with `seq > from_seq` from a Run's log (resync).
    ///
    /// Used by the frontend to recover events lost to broadcast lag (B2).
    pub fn replay_since(&self, run_id: &str, from_seq: u64) -> Result<Vec<Envelope>> {
        let path = crate::paths::get_runs_dir().join(format!("{run_id}.jsonl"));
        EventLog::replay_since(&path, from_seq)
    }

    /// Create a Run in an isolated git worktree.
    ///
    /// This creates a new git worktree with a fresh branch, then creates a
    /// Run with `working_dir` set to the worktree path. The Run's tools
    /// (shell, file operations) execute in the worktree, not the main repo.
    ///
    /// When the Run completes (or is cancelled), the worktree is NOT
    /// automatically removed — the caller should inspect the result first
    /// and call `cleanup_worktree()` when done.
    pub async fn create_run_in_worktree(
        &self,
        user_input: &str,
        session_id: Option<String>,
        repo_root: &str,
        branch_name: &str,
        history: Vec<crate::types::Message>,
    ) -> Result<(RunId, String)> {
        let mut wt = WorktreeManager::new(std::path::PathBuf::from(repo_root));
        let record = wt.create(&uuid::Uuid::new_v4().to_string(), branch_name)?;
        let worktree_path = record.path.to_string_lossy().to_string();

        let result = self
            .create_run_with_workdir(
                user_input,
                session_id,
                Some(worktree_path.clone()),
                history,
                None,
                false,
            )
            .await?;

        Ok((result.run_id, worktree_path))
    }

    /// Remove a git worktree by path. Called after a worktree-isolated
    /// Run has been inspected and the caller is done with it.
    pub fn cleanup_worktree(&self, repo_root: &str, worktree_path: &str) -> Result<()> {
        let mut wt = WorktreeManager::new(std::path::PathBuf::from(repo_root));
        // Find the worktree record by path
        let target_id = wt
            .list_all()
            .iter()
            .find(|r| r.path.to_string_lossy() == worktree_path)
            .map(|r| r.id.clone());
        if let Some(id) = target_id {
            wt.remove(&id)?;
            return Ok(());
        }
        anyhow::bail!("worktree not found: {worktree_path}")
    }

    /// Switch the active model for future Runs.
    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        let brain_mut = Arc::make_mut(&mut self.brain);
        brain_mut.switch_model(name)
    }

    /// Update the active configuration.
    pub fn update_config(&mut self, config: crate::config::Config) -> Result<()> {
        let brain_mut = Arc::make_mut(&mut self.brain);
        brain_mut.update_config(config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Config {
        let toml = r#"
default_model = "test/default"

[providers.test]
name = "test"
base_url = "http://127.0.0.1:1"
api_key = "sk-test"

[providers.test.models]
default = { model_id = "mock" }
"#;
        let mut config: Config = toml::from_str(toml).unwrap();
        config.rebuild_models();
        config
    }

    #[tokio::test]
    async fn create_run_returns_id() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager.create_run("hello", None, vec![]).await.unwrap().run_id;
        assert!(!run_id.is_empty());
    }

    #[tokio::test]
    async fn list_runs_shows_created_run() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let _id = manager.create_run("hello", None, vec![]).await.unwrap();
        let runs = manager.list_runs().await;
        assert_eq!(runs.len(), 1);
    }

    #[tokio::test]
    async fn cancel_run_sends_command() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager.create_run("hello", None, vec![]).await.unwrap().run_id;
        // Cancel before start should work
        manager.cancel_run(&run_id).await.unwrap();
    }

    #[tokio::test]
    async fn command_to_nonexistent_fails() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let result = manager.command("nonexistent", RunCommand::Cancel).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_run_with_workdir() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run_with_workdir("hello", None, Some("/tmp".to_string()), vec![], None, false)
            .await
            .unwrap()
            .run_id;
        assert!(!run_id.is_empty());
    }

    #[tokio::test]
    async fn multiple_concurrent_runs() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);

        // Create two runs — they should coexist
        let id1 = manager.create_run("task 1", None, vec![]).await.unwrap().run_id;
        let id2 = manager.create_run("task 2", None, vec![]).await.unwrap().run_id;

        let runs = manager.list_runs().await;
        assert_eq!(runs.len(), 2);
        assert!(runs.contains(&id1));
        assert!(runs.contains(&id2));
    }

    #[tokio::test]
    async fn cancel_all_cancels_every_run() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);

        let _id1 = manager.create_run("task 1", None, vec![]).await.unwrap();
        let _id2 = manager.create_run("task 2", None, vec![]).await.unwrap();
        assert_eq!(manager.list_runs().await.len(), 2);

        manager.cancel_all().await;
        // After cancel_all, runs are still tracked (they transition to Cancelled)
        // but they should eventually be done.
    }
}
