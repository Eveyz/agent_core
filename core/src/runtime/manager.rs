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

use crate::context::ContextUsageSnapshot;
use crate::mode::AgentMode;
use crate::permission::ApprovalChoice;
use crate::reflector::Reflector;
use crate::runtime::approval::ApprovalResolver;
use crate::runtime::brain::Brain;
use crate::runtime::command::{RunCommand, SteerEntry};
use crate::runtime::event::{Envelope, RunEvent, RunId};
use crate::runtime::event_log::EventLog;
use crate::runtime::input::{ClarificationAnswers, InputResolver};
use crate::runtime::intent::UserIntent;
use crate::runtime::run::{Run, ScopedToolFactory, default_runs_dir};
use crate::runtime::state::RunState;
use crate::runtime::steering::{SteerAcceptError, SteeringController};
use crate::session::SessionManager;
use crate::types::{ImageAttachment, Message};
use crate::worktree::WorktreeManager;

/// Capacity for the event broadcast channel per Run.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Capacity for the command channel per Run.
const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// Arguments for [`RunManager::create_run_from`].
///
/// Slash strings must be parsed into [`UserIntent`] before they reach a Run.
pub struct CreateRunRequest {
    pub intent: UserIntent,
    pub session_id: Option<String>,
    pub working_dir: Option<String>,
    pub history: Vec<Message>,
    pub initial_goal: Option<String>,
    pub initial_goal_completed: bool,
    pub user_images: Vec<ImageAttachment>,
    pub existing_prompt_id: Option<String>,
    pub scoped_tool_factory: Option<ScopedToolFactory>,
}

impl CreateRunRequest {
    pub fn from_text(text: &str) -> Self {
        Self {
            intent: UserIntent::parse(text),
            session_id: None,
            working_dir: None,
            history: Vec::new(),
            initial_goal: None,
            initial_goal_completed: false,
            user_images: Vec::new(),
            existing_prompt_id: None,
            scoped_tool_factory: None,
        }
    }
}

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
    /// Broadcast sender for events. Subscribers call `.subscribe()` on this.
    pub(crate) event_tx: broadcast::Sender<Envelope>,
    /// Single durable event mailbox. Steers publish acceptance here immediately
    /// instead of waiting for the interrupted turn to unwind.
    producer_tx: mpsc::UnboundedSender<Envelope>,
    /// The tokio task running the Run's loop.
    join_handle: Option<JoinHandle<()>>,
    /// Shared state for querying (read-only, updated by the Run task).
    state: Arc<RwLock<RunState>>,
    /// Per-Run approval resolver — resolved directly by `approve_tool`
    /// to avoid actor deadlock (bypassing the command channel).
    pub(crate) approval_resolver: ApprovalResolver,
    /// Per-Run clarification resolver — resolved directly by `answer_input`.
    pub(crate) input_resolver: InputResolver,
    /// CancellationToken — cancelled immediately on `cancel_run()` so that
    /// hot-path checks in collect_stream() and ToolOrchestrator respond
    /// without waiting for the next poll_commands() turn boundary.
    pub(crate) cancel_token: CancellationToken,
    /// Wakeable mailbox used to interrupt a turn without ending the Run.
    pub(crate) steering: SteeringController,
    /// Shared context snapshot (refreshed by the Run at turn boundaries).
    context_snapshot: Arc<RwLock<Vec<Message>>>,
    /// Shared context usage breakdown (refreshed with context_snapshot).
    usage_snapshot: Arc<RwLock<ContextUsageSnapshot>>,
    /// Compactable conversation window, distinct from the canonical transcript.
    #[allow(dead_code)] // retained by the handle; queried by regression tests
    model_window_snapshot: Arc<RwLock<Vec<Message>>>,
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

    #[cfg(test)]
    fn model_window_snapshot(&self) -> Vec<Message> {
        self.model_window_snapshot.read().clone()
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

    pub fn config(&self) -> &crate::config::Config {
        self.brain.config()
    }

    pub fn permissions(&self) -> crate::permission::PermissionConfig {
        self.brain.permissions().clone()
    }

    pub fn mcp_config(&self) -> &crate::mcp::McpConfig {
        self.brain.mcp_config()
    }

    pub fn todo_store(&self) -> &crate::todo::SessionPlanStore {
        self.brain.todo_store()
    }

    pub fn has_reflection_daemon(&self) -> bool {
        self.brain.has_reflection_daemon()
    }

    pub fn set_temperature(&mut self, val: f64) {
        Arc::make_mut(&mut self.brain).set_temperature(val);
    }

    pub fn set_max_tokens(&mut self, val: u32) {
        Arc::make_mut(&mut self.brain).set_max_tokens(val);
    }

    pub fn set_tool_execution_mode(&mut self, mode: crate::types::ToolExecutionMode) {
        Arc::make_mut(&mut self.brain).set_tool_execution_mode(mode);
    }

    pub fn set_permission_mode(&mut self, mode: crate::permission::PermissionMode) {
        Arc::make_mut(&mut self.brain).set_permission_mode(mode);
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

    /// Full (uncompressed) conversation transcript for one Run. Safe to
    /// persist as the canonical session history — compaction never mutates it.
    /// Dynamic context segments are not included.
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

    /// Idle / cold-session usage estimate using the same restore path as
    /// [`Self::create_run`]: prefer the compacted model-window checkpoint,
    /// falling back to the canonical transcript when none is valid.
    pub fn estimate_usage_for_persisted_session(
        &self,
        session_manager: &SessionManager,
        session_id: &str,
    ) -> ContextUsageSnapshot {
        let active_model_id = self
            .brain()
            .current_model_config()
            .map(|model| model.model_id)
            .unwrap_or_else(|_| "unknown".to_string());
        let messages =
            session_manager.model_context_messages_for_usage(session_id, &active_model_id);
        self.estimate_usage_from_messages(&messages)
    }

    /// Active model id used when restoring idle session usage / checkpoints.
    pub fn current_model_id(&self) -> String {
        self.brain()
            .current_model_config()
            .map(|model| model.model_id)
            .unwrap_or_else(|_| "unknown".to_string())
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
        let mut request = CreateRunRequest::from_text(user_input);
        request.session_id = session_id;
        request.history = history;
        self.create_run_from(request).await
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
        let mut request = CreateRunRequest::from_text(user_input);
        request.session_id = session_id;
        request.working_dir = working_dir;
        request.history = history;
        request.initial_goal = initial_goal;
        request.initial_goal_completed = initial_goal_completed;
        self.create_run_from(request).await
    }

    /// Create a Run with optional user image attachments for multimodal models.
    ///
    /// When `existing_prompt_id` is set, that prompt row is reused (caller already
    /// created it — e.g. to persist images under the prompt folder first).
    pub async fn create_run_with_workdir_and_images(
        &self,
        user_input: &str,
        session_id: Option<String>,
        working_dir: Option<String>,
        history: Vec<crate::types::Message>,
        initial_goal: Option<String>,
        initial_goal_completed: bool,
        user_images: Vec<crate::types::ImageAttachment>,
        existing_prompt_id: Option<String>,
        scoped_tool_factory: Option<super::run::ScopedToolFactory>,
    ) -> Result<CreateRunResult> {
        self.create_run_from(CreateRunRequest {
            intent: UserIntent::parse(user_input),
            session_id,
            working_dir,
            history,
            initial_goal,
            initial_goal_completed,
            user_images,
            existing_prompt_id,
            scoped_tool_factory,
        })
        .await
    }

    /// Create a Run from a structured request. This is the external seam.
    pub async fn create_run_from(&self, request: CreateRunRequest) -> Result<CreateRunResult> {
        let CreateRunRequest {
            intent,
            session_id,
            working_dir,
            history,
            initial_goal,
            initial_goal_completed,
            user_images,
            existing_prompt_id,
            scoped_tool_factory,
        } = request;
        let run_id = uuid::Uuid::new_v4().to_string();

        // Canonical prompt row — sole source of truth for session rewind.
        let prompt_id: Option<String> = if let Some(pid) = existing_prompt_id {
            Some(pid)
        } else if let (Some(sid), Some(sm)) = (session_id.as_ref(), self.session_manager.as_ref()) {
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
        let checkpoint_model_id = model_config.model_id.clone();
        let model_history = if let (Some(session_id), Some(session_manager)) =
            (session_id.as_ref(), self.session_manager.as_ref())
        {
            let session_id = session_id.clone();
            let session_manager = session_manager.clone();
            let full_history = history.clone();
            let active_model_id = checkpoint_model_id.clone();
            match tokio::task::spawn_blocking(move || {
                session_manager.load_model_window_checkpoint(
                    &session_id,
                    &full_history,
                    &active_model_id,
                )
            })
            .await
            {
                Ok(Ok(Some(checkpoint))) => checkpoint,
                Ok(Ok(None)) => history.clone(),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "failed to load model-window checkpoint");
                    history.clone()
                }
                Err(error) => {
                    tracing::warn!(%error, "model-window checkpoint task failed");
                    history.clone()
                }
            }
        } else {
            history.clone()
        };

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
        let model_window_snapshot = Arc::new(RwLock::new(Vec::<Message>::new()));

        // Create the Run
        let mut run = match Run::new(
            run_id.clone(),
            session_id.clone(),
            prompt_id.clone(),
            self.brain.clone(),
            model_config,
            cmd_rx,
            producer_tx.clone(),
            seq.clone(),
            working_dir,
            history,
            model_history,
            mode,
            context_snapshot.clone(),
            usage_snapshot.clone(),
            model_window_snapshot.clone(),
            initial_goal,
            initial_goal_completed,
            self.session_manager.clone(),
            scoped_tool_factory,
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
        if !user_images.is_empty() {
            run.set_pending_user_images(user_images);
        }
        // Clone the approval resolver before spawning so we can store
        // it in the RunHandle for direct (non-command-channel) resolution.
        let approval_resolver = run.approval_resolver().clone();
        let input_resolver = run.input_resolver().clone();
        // Clone the cancel token so cancel_run() can trigger immediate
        // cancellation without waiting for the next poll_commands() cycle.
        let cancel_token = run.cancel_token();
        let steering = run.steering_controller();

        let writer_run_id = run_id.clone();
        let writer_broadcast = event_tx.clone();
        let writer_cancel = cancel_token.clone();
        let writer_state = shared_state.clone();
        let writer_snapshot = context_snapshot.clone();
        let writer_model_window = model_window_snapshot.clone();
        let writer_model_id = checkpoint_model_id.clone();
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
                        let model_window = writer_model_window.read().clone();
                        let model_id = writer_model_id.clone();
                        let status = status.to_string();
                        let _ = tokio::task::spawn_blocking(move || {
                            if !messages.is_empty() {
                                let transcript_saved = if let Err(e) =
                                    sm.save_canonical_transcript_for_prompt(&sid, &messages, &pid)
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to persist canonical transcript"
                                    );
                                    false
                                } else {
                                    true
                                };
                                if transcript_saved
                                    && let Err(e) = sm.save_model_window_checkpoint(
                                        &sid,
                                        &model_id,
                                        &messages,
                                        &model_window,
                                    )
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to persist model-window checkpoint"
                                    );
                                }
                            }
                            if let Err(e) = sm.finish_prompt(&pid, &status, &serde_json::json!({}))
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
            crate::paths::get_runs_dir().to_string_lossy().as_ref(),
        ) {
            // Because the current run's log hasn't been written yet (the log task hasn't processed RunCreated),
            // the last run in the directory is truly the "previous" run.
            if let Some(prev_run_id) = runs.last() {
                let diffs =
                    crate::reflector::diff_observer::DiffObserver::check_for_diffs(prev_run_id);
                if !diffs.is_empty() {
                    tracing::info!(
                        diff_count = diffs.len(),
                        "Diff Observer found manual edits to previous run's files"
                    );
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
        let intent_for_run = intent;
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

            run.run(intent_for_run).await;
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
                        crate::reflector::diff_observer::DiffObserver::take_snapshot(
                            &reflect_run_id,
                            &events,
                        );
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
            producer_tx,
            join_handle: Some(join_handle),
            state: shared_state,
            approval_resolver,
            input_resolver,
            cancel_token,
            steering,
            context_snapshot,
            usage_snapshot,
            model_window_snapshot,
        };

        self.runs.lock().await.insert(run_id.clone(), handle);

        Ok(CreateRunResult { run_id, prompt_id })
    }

    /// Send a command to a specific Run.
    pub async fn command(&self, run_id: &str, cmd: RunCommand) -> Result<()> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        match cmd {
            RunCommand::Steer { steer_id, message } => {
                Self::enqueue_steer(handle, steer_id, message)
            }
            other => handle.command(other),
        }
    }

    /// Interrupt the active turn, inject a user steer, and continue the same Run.
    pub async fn steer_run(&self, run_id: &str, steer_id: String, message: String) -> Result<()> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .with_context(|| format!("run '{run_id}' not found"))?;
        Self::enqueue_steer(handle, steer_id, message)
    }

    fn enqueue_steer(handle: &RunHandle, steer_id: String, message: String) -> Result<()> {
        anyhow::ensure!(
            !handle.state().is_terminal(),
            "run '{}' is already terminal",
            handle.id
        );
        let entry = SteerEntry {
            id: steer_id.clone(),
            message: RunCommand::steer_message(&message),
            raw_text: message.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        match handle.steering.accept(entry, |queue_depth| {
            handle.producer_tx.send(Envelope {
                seq: 0,
                event_id: uuid::Uuid::new_v4().to_string(),
                run_id: handle.id.clone(),
                session_id: handle.session_id.clone(),
                turn_id: None,
                parent_call_id: None,
                ts: chrono::Utc::now(),
                event: RunEvent::SteerQueued {
                    steer_id,
                    message,
                    queue_depth,
                },
            })
        }) {
            Ok(_) => Ok(()),
            Err(SteerAcceptError::Closed) => {
                anyhow::bail!("run '{}' no longer accepts steers", handle.id)
            }
            Err(SteerAcceptError::Publish(_)) => {
                anyhow::bail!("run '{}' event mailbox is closed", handle.id)
            }
        }
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
        test_config_with_base_url("http://127.0.0.1:1")
    }

    fn test_config_with_base_url(base_url: &str) -> Config {
        let toml = format!(
            r#"
default_model = "test/default"

[providers.test]
name = "test"
base_url = "{base_url}"
api_key = "sk-test"
max_context_tokens = 100000

[providers.test.models]
default = {{ model_id = "mock", max_context_tokens = 100000 }}
"#
        );
        let mut config: Config = toml::from_str(&toml).unwrap();
        config.rebuild_models();
        config
    }

    /// Build a transcript above the 80% compaction threshold (~80k tokens @ 100k max).
    fn oversized_transcript_for_compaction() -> Vec<Message> {
        let payload = "w ".repeat(5_000);
        let mut full = Vec::with_capacity(20);
        for i in 0..8 {
            full.push(Message::user(&format!("task-{i} {payload}")));
            full.push(Message::assistant(&format!("answer-{i} {payload}")));
        }
        full
    }

    #[tokio::test]
    async fn create_run_returns_id() {
        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run("hello", None, vec![])
            .await
            .unwrap()
            .run_id;
        assert!(!run_id.is_empty());
    }

    #[tokio::test]
    async fn create_run_restores_compacted_model_window_instead_of_full_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let storage = crate::memory::storage::Storage::new(
            dir.path().join("memory.db").to_string_lossy().as_ref(),
        )
        .unwrap();
        let session_manager = Arc::new(SessionManager::new(storage));
        let full = vec![
            Message::user("old user"),
            Message::assistant("old assistant"),
            Message::user("recent user"),
            Message::assistant("recent assistant"),
        ];
        let session_id = session_manager.save(None, &full, "/tmp", "mock").unwrap();
        session_manager
            .save_model_window_checkpoint(&session_id, "mock", &full, &full[2..])
            .unwrap();

        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain).with_session_manager(session_manager);
        let run_id = manager
            .create_run("next", Some(session_id), full)
            .await
            .unwrap()
            .run_id;
        let runs = manager.runs.lock().await;
        let window = runs.get(&run_id).unwrap().model_window_snapshot();

        assert_eq!(window.len(), 2);
        assert_eq!(
            window[0].content.as_deref(),
            Some("recent user"),
            "the next Run must not re-expand the canonical transcript"
        );
    }

    #[test]
    fn idle_usage_estimate_prefers_compacted_model_window() {
        let dir = tempfile::tempdir().unwrap();
        let storage = crate::memory::storage::Storage::new(
            dir.path().join("memory.db").to_string_lossy().as_ref(),
        )
        .unwrap();
        let session_manager = SessionManager::new(storage);
        let payload = "w ".repeat(5_000);
        let old_user = format!("old {payload}");
        let old_assistant = format!("old reply {payload}");
        let full = vec![
            Message::user(&old_user),
            Message::assistant(&old_assistant),
            Message::user("recent user"),
            Message::assistant("recent assistant"),
        ];
        let session_id = session_manager.save(None, &full, "/tmp", "mock").unwrap();
        session_manager
            .save_model_window_checkpoint(&session_id, "mock", &full, &full[2..])
            .unwrap();

        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);

        let full_estimate = manager.estimate_usage_from_messages(&full);
        let idle_estimate =
            manager.estimate_usage_for_persisted_session(&session_manager, &session_id);

        assert!(
            idle_estimate.used_tokens < full_estimate.used_tokens,
            "idle usage must track the compacted window, not the full transcript \
             (idle={}, full={})",
            idle_estimate.used_tokens,
            full_estimate.used_tokens
        );
        assert!(
            idle_estimate.conversation_tokens < full_estimate.conversation_tokens,
            "conversation bucket should shrink with the model window"
        );
    }

    #[test]
    fn idle_usage_includes_messages_appended_after_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let storage = crate::memory::storage::Storage::new(
            dir.path().join("memory.db").to_string_lossy().as_ref(),
        )
        .unwrap();
        let session_manager = SessionManager::new(storage);
        let full = vec![
            Message::user("old user"),
            Message::assistant("old assistant"),
            Message::user("recent user"),
            Message::assistant("recent assistant"),
        ];
        let session_id = session_manager.save(None, &full, "/tmp", "mock").unwrap();
        session_manager
            .save_model_window_checkpoint(&session_id, "mock", &full, &full[2..])
            .unwrap();

        let follow_up = "brand-new follow-up after compact";
        let mut extended = full.clone();
        extended.push(Message::user(follow_up));
        session_manager
            .save_canonical_transcript(&session_id, &extended)
            .unwrap();

        let brain = Brain::from_config(test_config()).unwrap();
        let manager = RunManager::new(brain);
        let messages = session_manager.model_context_messages_for_usage(&session_id, "mock");

        assert_eq!(
            messages.len(),
            3,
            "compacted window + post-checkpoint message"
        );
        assert_eq!(messages[0].content.as_deref(), Some("recent user"));
        assert_eq!(messages[2].content.as_deref(), Some(follow_up));

        let idle = manager.estimate_usage_for_persisted_session(&session_manager, &session_id);
        let window_only = manager.estimate_usage_from_messages(&full[2..]);
        assert!(
            idle.conversation_tokens > window_only.conversation_tokens,
            "idle usage must include turns appended after the checkpoint"
        );
    }

    #[tokio::test]
    async fn compaction_checkpoint_is_durable_before_terminal_and_prevents_next_run_reexpansion() {
        use crate::eval::mock_llm::{MockScript, MockStep, start_mock_server};

        let summary_json =
            r#"{"goal":"test","decisions":[],"files":{},"errors_open":[],"facts":[],"notes":[]}"#;
        let mock = start_mock_server(MockScript {
            steps: vec![
                MockStep::Text {
                    text: summary_json.into(),
                    cache_hit: 0,
                    cache_miss: 0,
                },
                MockStep::Text {
                    text: "done".into(),
                    cache_hit: 0,
                    cache_miss: 0,
                },
            ],
        })
        .await
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let storage = crate::memory::storage::Storage::new(
            dir.path().join("memory.db").to_string_lossy().as_ref(),
        )
        .unwrap();
        let session_manager = Arc::new(SessionManager::new(storage));
        let full = oversized_transcript_for_compaction();
        let full_message_count = full.len();
        let session_id = session_manager.save(None, &full, "/tmp", "mock").unwrap();
        let brain = Brain::from_config(test_config_with_base_url(&mock.base_url)).unwrap();
        let manager = RunManager::new(brain).with_session_manager(session_manager.clone());
        assert_eq!(manager.current_max_context_tokens(), 100_000);
        assert!(
            manager.estimate_usage_from_messages(&full).used_tokens >= 80_000,
            "fixture must begin above the active model's 80% threshold"
        );
        let first_run = manager
            .create_run("compact now", Some(session_id.clone()), full.clone())
            .await
            .unwrap()
            .run_id;
        assert!(
            manager
                .context_usage_for_run(&first_run)
                .await
                .unwrap()
                .used_tokens
                >= 80_000,
            "the actual Run must begin above its 80% threshold"
        );
        let mut checkpoint_full = full.clone();
        checkpoint_full.push(Message::user_with_model("compact now", "mock"));
        manager
            .command(&first_run, RunCommand::Start)
            .await
            .unwrap();

        let checkpoint_result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Some((source_len, window_len)) = session_manager
                    .model_window_checkpoint_shape(&session_id)
                    .unwrap()
                    && source_len == checkpoint_full.len()
                    && window_len < checkpoint_full.len()
                {
                    break session_manager
                        .load_model_window_checkpoint(&session_id, &checkpoint_full, "mock")
                        .unwrap()
                        .unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await;
        let checkpoint = match checkpoint_result {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                let shape = session_manager
                    .model_window_checkpoint_shape(&session_id)
                    .unwrap();
                let runs = manager.runs.lock().await;
                let handle = runs.get(&first_run).unwrap();
                panic!(
                    "compaction must be durable before the model request completes: {error}; \
                     state={:?}, checkpoint={shape:?}, window_len={}, used_tokens={}",
                    handle.state(),
                    handle.model_window_snapshot().len(),
                    handle.usage_snapshot().used_tokens,
                );
            }
        };
        assert!(checkpoint.len() < full.len());
        let resumed = session_manager.resume(&session_id).unwrap().unwrap();
        let resumed_full = resumed.messages;
        assert_eq!(
            resumed_full.len(),
            checkpoint_full.len(),
            "the canonical source referenced by the checkpoint must already be durable"
        );
        assert_eq!(
            resumed.prompts.last().map(|prompt| prompt.status.as_str()),
            Some("running"),
            "a live compaction checkpoint must not finish the prompt"
        );
        assert!(
            session_manager
                .get_meta(&session_id)
                .unwrap()
                .unwrap()
                .end_time
                .is_none(),
            "a live compaction checkpoint must not end the session"
        );

        let second_run = manager
            .create_run("next", Some(session_id), resumed_full)
            .await
            .unwrap()
            .run_id;
        let mut second_events = manager.subscribe(&second_run).await.unwrap();
        {
            let runs = manager.runs.lock().await;
            let second = runs.get(&second_run).unwrap();
            assert!(second.model_window_snapshot().len() < full_message_count);
            assert!(
                second.usage_snapshot().used_tokens < 80_000,
                "the next Run should wait for the active model's 80% threshold"
            );
        }
        manager
            .command(&second_run, RunCommand::Start)
            .await
            .unwrap();
        let compacted_again = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match second_events.recv().await.unwrap().event {
                    RunEvent::ContextCompacted { .. } => break true,
                    RunEvent::RunCompleted { .. }
                    | RunEvent::RunCancelled { .. }
                    | RunEvent::RunFailed { .. } => break false,
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(
            !compacted_again,
            "a restored window below the active model's 80% threshold must not compact again"
        );
        let _ = manager.cancel_run(&first_run).await;
        let _ = manager.cancel_run(&second_run).await;
        mock.shutdown().await;
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
        let run_id = manager
            .create_run("hello", None, vec![])
            .await
            .unwrap()
            .run_id;
        // Cancel before start should work
        manager.cancel_run(&run_id).await.unwrap();
    }

    #[tokio::test]
    async fn steer_interrupts_an_in_flight_model_request_and_continues_same_run() {
        use crate::eval::mock_llm::{MockScript, MockStep, start_mock_server};

        let mock = start_mock_server(MockScript {
            steps: vec![
                MockStep::DelayedText {
                    text: "obsolete answer".into(),
                    delay_ms: 2_000,
                },
                MockStep::Text {
                    text: "steered answer".into(),
                    cache_hit: 0,
                    cache_miss: 0,
                },
            ],
        })
        .await
        .unwrap();
        let brain = Brain::from_config(test_config_with_base_url(&mock.base_url)).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run("original request", None, vec![])
            .await
            .unwrap()
            .run_id;
        let mut events = manager.subscribe(&run_id).await.unwrap();
        manager.command(&run_id, RunCommand::Start).await.unwrap();

        loop {
            if matches!(
                events.recv().await.unwrap().event,
                RunEvent::TurnStarted { .. }
            ) {
                break;
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while mock.call_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first model request should be in flight");

        manager
            .steer_run(&run_id, "steer-1".into(), "new direction".into())
            .await
            .unwrap();

        let mut saw_queued = false;
        let mut saw_interrupted = false;
        let final_text = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap().event {
                    RunEvent::SteerQueued { steer_id, .. } => {
                        assert_eq!(steer_id, "steer-1");
                        saw_queued = true;
                    }
                    RunEvent::MessageInterrupted { reason, .. } => {
                        assert_eq!(reason, "user_steer");
                        assert!(saw_queued, "acceptance must precede interruption");
                        saw_interrupted = true;
                    }
                    RunEvent::RunCompleted { final_text } => break final_text,
                    RunEvent::RunFailed { error } => panic!("run failed: {error}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("steer should interrupt without waiting for the delayed response");

        assert!(saw_queued);
        assert!(saw_interrupted);
        assert_eq!(final_text, "steered answer");
        mock.shutdown().await;
    }

    #[tokio::test]
    async fn steer_interrupts_an_in_flight_tool_and_pairs_its_terminal_event() {
        use crate::eval::mock_llm::{MockScript, MockStep, MockToolCall, start_mock_server};

        let mock = start_mock_server(MockScript {
            steps: vec![
                MockStep::ToolCalls {
                    tools: vec![MockToolCall {
                        name: "shell".into(),
                        arguments: serde_json::json!({
                            "command": "sleep 2; echo obsolete"
                        }),
                    }],
                    cache_hit: 0,
                    cache_miss: 0,
                },
                MockStep::Text {
                    text: "continued after tool interrupt".into(),
                    cache_hit: 0,
                    cache_miss: 0,
                },
            ],
        })
        .await
        .unwrap();
        let mut config = test_config_with_base_url(&mock.base_url);
        config.permissions.mode = crate::permission::PermissionMode::Yolo;
        let brain = Brain::from_config(config).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run("run the command", None, vec![])
            .await
            .unwrap()
            .run_id;
        let mut events = manager.subscribe(&run_id).await.unwrap();
        manager.command(&run_id, RunCommand::Start).await.unwrap();

        loop {
            if matches!(
                events.recv().await.unwrap().event,
                RunEvent::ToolStarted { .. }
            ) {
                break;
            }
        }
        manager
            .steer_run(&run_id, "steer-tool".into(), "stop that command".into())
            .await
            .unwrap();

        let mut interrupted_tool = false;
        let final_text = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap().event {
                    RunEvent::ToolEnded {
                        result, is_error, ..
                    } if is_error => {
                        assert!(result.to_ascii_lowercase().contains("interrupt"));
                        interrupted_tool = true;
                    }
                    RunEvent::RunCompleted { final_text } => break final_text,
                    RunEvent::RunFailed { error } => panic!("run failed: {error}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("steer should not wait for the long-running tool");

        assert!(interrupted_tool);
        assert_eq!(final_text, "continued after tool interrupt");
        mock.shutdown().await;
    }

    #[tokio::test]
    async fn steer_preserves_partial_model_text_when_interrupting_a_stream() {
        use crate::eval::mock_llm::{MockScript, MockStep, start_mock_server};

        let mock = start_mock_server(MockScript {
            steps: vec![
                MockStep::StreamingText {
                    first: "useful partial".into(),
                    rest: " obsolete tail".into(),
                    delay_ms: 2_000,
                },
                MockStep::Text {
                    text: "replacement answer".into(),
                    cache_hit: 0,
                    cache_miss: 0,
                },
            ],
        })
        .await
        .unwrap();
        let brain = Brain::from_config(test_config_with_base_url(&mock.base_url)).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run("original request", None, vec![])
            .await
            .unwrap()
            .run_id;
        let mut events = manager.subscribe(&run_id).await.unwrap();
        manager.command(&run_id, RunCommand::Start).await.unwrap();

        loop {
            if matches!(
                events.recv().await.unwrap().event,
                RunEvent::ModelStreaming { .. }
            ) {
                break;
            }
        }
        manager
            .steer_run(&run_id, "steer-partial".into(), "replace it".into())
            .await
            .unwrap();

        let partial = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let RunEvent::MessageInterrupted {
                    partial_message, ..
                } = events.recv().await.unwrap().event
                {
                    break partial_message;
                }
            }
        })
        .await
        .expect("stream should be interrupted promptly")
        .expect("visible partial output should be retained");
        assert!(
            partial
                .content
                .as_deref()
                .is_some_and(|text| text.contains("useful partial"))
        );
        assert!(
            !partial
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("obsolete tail")
        );
        mock.shutdown().await;
    }

    #[tokio::test]
    async fn steer_is_rejected_after_terminal_event_even_if_state_mirror_lags() {
        use crate::eval::mock_llm::{MockScript, MockStep, start_mock_server};

        let mock = start_mock_server(MockScript {
            steps: vec![MockStep::Text {
                text: "done".into(),
                cache_hit: 0,
                cache_miss: 0,
            }],
        })
        .await
        .unwrap();
        let brain = Brain::from_config(test_config_with_base_url(&mock.base_url)).unwrap();
        let manager = RunManager::new(brain);
        let run_id = manager
            .create_run("finish", None, vec![])
            .await
            .unwrap()
            .run_id;
        let mut events = manager.subscribe(&run_id).await.unwrap();
        manager.command(&run_id, RunCommand::Start).await.unwrap();
        loop {
            if matches!(
                events.recv().await.unwrap().event,
                RunEvent::RunCompleted { .. }
            ) {
                break;
            }
        }

        manager
            .steer_run(&run_id, "late-steer".into(), "too late".into())
            .await
            .expect_err("terminal runs must reject steers");
        mock.shutdown().await;
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
        let id1 = manager
            .create_run("task 1", None, vec![])
            .await
            .unwrap()
            .run_id;
        let id2 = manager
            .create_run("task 2", None, vec![])
            .await
            .unwrap()
            .run_id;

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
