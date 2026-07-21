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
//!
//! ## Module structure
//!
//! This module is split into focused submodules, each responsible for a
//! distinct concern within the Run's lifecycle:
//!
//! - [`lifecycle`] — main entry point, turn loop, command polling, pause/resume
//! - [`turn`] — individual turn execution, model calls, stream collection
//! - [`compact`] — context compaction (chunked drop + LLM summarization)
//! - [`recovery`] — error recovery strategies (retry, compact, model switch)
//! - [`context`] — message building, context segment refresh, goal decomposition
//! - [`helpers`] — session memory, cleanup, teardown

mod compact;
mod context;
mod helpers;
mod lifecycle;
mod recovery;
mod turn;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use parking_lot::{Mutex, RwLock};

use crate::client::OpenAIClient;
use crate::config::ModelConfig;
use crate::context::ContextEngine as Context;
use crate::context::ContextUsageSnapshot;
use crate::context_processor::ContextProcessor;
use crate::error_recovery::{RecoveryContext, RecoveryEngine};
use crate::hooks::HookRegistry;
use crate::mode::AgentMode;
use crate::permission::PermissionPolicy;
use crate::runtime::approval::ApprovalResolver;
use crate::runtime::brain::Brain;
use crate::runtime::command::{RunCommand, SteerEntry};
use crate::runtime::event::{CacheMetrics, Envelope, RunEvent, RunId};
use crate::runtime::execution::ExecutionState;
use crate::runtime::input::InputResolver;
use crate::runtime::state::RunState;
use crate::runtime::supervisor::ProcessSupervisor;
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;
use crate::types::{Message, Role, ToolExecutionMode};

/// Threshold for detecting cache expiry due to idle time.
/// DeepSeek's prefix cache has an undocumented ~5–10 minute idle timeout.
/// We warn at 4 minutes to give headroom before the actual expiry.
const CACHE_IDLE_WARN_SECS: u64 = 240;

/// Result of running a single turn.
enum TurnOutcome {
    Final(String),
    Continue,
}

/// Outcome of a recovery attempt within model_turn.
enum RecoveryOutcome {
    Retry,
    GiveUp,
}

/// Error type for the run loop.
pub enum RunError {
    Cancelled,
    Failed(String),
}

/// A Run is the independent execution space for a single user request.
///
/// It is constructed by [`crate::runtime::RunManager`] and runs on its own
/// tokio task. Communication with the outside world is via commands (in)
/// and events (out).
pub struct Run {
    pub id: RunId,
    pub session_id: Option<String>,
    /// Lifecycle prompt id for this Run (from `create_prompt`), if any.
    pub prompt_id: Option<String>,

    /// The shared, reusable brain.
    brain: Arc<Brain>,
    /// Workspace-scoped skill state for this Run.
    skill_manager: Option<Arc<Mutex<SkillManager>>>,

    // ── Per-Run state (not shared) ────────────────────────────────
    state: RunState,
    context: Context,
    client: OpenAIClient,
    registry: ToolRegistry,
    permission_policy: PermissionPolicy,
    hook_registry: Arc<parking_lot::Mutex<HookRegistry>>,
    recovery: RecoveryEngine,
    recovery_ctx: RecoveryContext,
    context_processors: Vec<ContextProcessor>,
    tool_execution_mode: ToolExecutionMode,

    // ── Communication channels ────────────────────────────────────
    cmd_rx: mpsc::Receiver<RunCommand>,
    event_tx: mpsc::UnboundedSender<Envelope>,
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
    steering_queue: VecDeque<SteerEntry>,
    /// Queued follow-up messages — injected as user messages after the Run completes.
    follow_up_queue: VecDeque<Message>,

    // ── Pending approvals / clarification (per-Run, not global) ───
    approval_resolver: ApprovalResolver,
    input_resolver: InputResolver,

    // ── Configuration ─────────────────────────────────────────────
    max_iterations: usize,
    /// Working directory for this Run. When set, tools that don't specify
    /// an explicit working_dir will execute here instead of the process CWD.
    /// Used for worktree isolation — each concurrent Run can work in its own
    /// git worktree without file conflicts.
    working_dir: Option<String>,

    /// The agent mode for this Run (Ask / Plan / Build).
    /// Immutable after construction — mode changes take effect on the next Run.
    mode: AgentMode,

    /// Pinned goal set via `/goal` (injected into the per-turn execution_plan segment).
    goal: Option<String>,
    /// Whether the pinned goal has been marked completed.
    goal_completed: bool,
    /// How many times we've nudged the model to keep working a pinned goal
    /// after it tried to end with text-only while todos were still pending.
    goal_continue_nudges: u8,

    /// Runtime-owned task execution phase + active step + artifacts.
    /// Orthogonal to [`RunState`] (run lifetime).
    execution: ExecutionState,

    /// Last stable prefix fingerprint — used to detect drift across turns.
    last_prefix_fingerprint: String,

    /// Cumulative cache hit/miss metrics across all turns of this Run.
    cache_metrics: CacheMetrics,
    /// Timestamp when the most recent turn ended.
    last_turn_end_time: Option<Instant>,

    /// Cached tool catalog to avoid rebuilding every turn.
    /// Stores (fingerprint, rendered_catalog_string).
    tool_catalog_cache: Option<(String, String)>,

    /// Names of script tools currently registered (for diff on deactivation).
    registered_script_tools: Vec<String>,

    /// Path to the per-session context snapshot file for mid-turn persistence.
    /// Written after each assistant message to prevent data loss on disconnect.
    session_snapshot_path: Option<std::path::PathBuf>,

    /// Monotonic generation for background snapshot writes. A newer save
    /// bumps this so an in-flight older write can skip its disk I/O and
    /// avoid clobbering a fresher snapshot.
    session_snapshot_gen: Arc<AtomicU64>,

    /// Shared, read-only **full** transcript snapshot for persistence and
    /// side-channel `/btw` queries. Always the uncompressed conversation —
    /// never the compactable model window. Refreshed at turn boundaries;
    /// read via `RunHandle::context_snapshot()`.
    context_snapshot: Arc<RwLock<Vec<Message>>>,

    /// Shared context usage breakdown for the Context Usage popover.
    /// Tracks the live **model window** (`context`), which may be compacted.
    usage_snapshot: Arc<RwLock<ContextUsageSnapshot>>,

    /// Immutable (w.r.t. compaction) full conversation transcript.
    /// Every real user/assistant/tool/(injected system) message is appended
    /// here; `maybe_compact` only mutates `context.messages`. This is what
    /// gets persisted to SQLite / crash snapshots so UI resume stays full.
    full_transcript: Vec<Message>,
}

impl Run {
    /// Construct a new Run (called by RunManager, not by users directly).
    pub(crate) fn new(
        id: RunId,
        session_id: Option<String>,
        prompt_id: Option<String>,
        brain: Arc<Brain>,
        model_config: ModelConfig,
        cmd_rx: mpsc::Receiver<RunCommand>,
        event_tx: mpsc::UnboundedSender<Envelope>,
        seq: Arc<AtomicU64>,
        working_dir: Option<String>,
        history: Vec<crate::types::Message>,
        mode: AgentMode,
        context_snapshot: Arc<RwLock<Vec<Message>>>,
        usage_snapshot: Arc<RwLock<ContextUsageSnapshot>>,
        initial_goal: Option<String>,
        initial_goal_completed: bool,
        session_manager: Option<Arc<crate::session::SessionManager>>,
    ) -> anyhow::Result<Self> {
        let client = brain.build_client()?;
        let permission_policy = brain.build_permission_policy();
        let recovery = brain.build_recovery();

        let identity = brain.identity_text();
        let max_context_tokens = model_config.max_context_tokens;
        let max_iterations = model_config.max_iterations;

        // Build the supervisor BEFORE the registry so we can inject it
        // into the ShellTool. The supervisor is owned by the Run and
        // shared with the tool via Arc<Mutex>.
        let supervisor = Arc::new(Mutex::new(ProcessSupervisor::new()));
        let working_dir_for_tool = working_dir.clone();
        let cancel_token = CancellationToken::new();
        let approval_resolver = ApprovalResolver::new();

        // Skill discovery follows a stable workspace-scoped manager rather
        // than mutating the desktop process' global path precedence.
        let skill_manager = brain.skill_manager_for_workspace(
            working_dir.as_deref().map(std::path::Path::new),
        )?;

        let mut registry = brain.build_tool_registry_for(
            mode,
            session_id.as_deref(),
            prompt_id.as_deref(),
        );
        if let Some(ref manager) = skill_manager {
            registry.remove_all(&[
                "skill_search",
                "skill_list_resources",
                "skill_read_resource",
                "skill_list",
                "skill_load",
                "skill_deactivate",
                "skill_reload",
            ]);
            crate::tools::skill::register_skill_tools(&mut registry, manager.clone());
        }
        // Replace the default ShellTool with a supervised version
        // (only present in Build mode — in other modes shell was already removed)
        if mode == AgentMode::Build {
            registry.register(Box::new(crate::tools::shell::ShellTool::with_supervisor(
                supervisor.clone(),
                working_dir_for_tool,
            )));
        }

        // Replace subagent tools with supervised + cancel-aware versions.
        if registry.has("subagent") {
            let available_tools: Vec<String> = registry
                .list_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            registry.remove_all(&["subagent", "subagents"]);
            crate::tools::subagent::register_subagent_tools(
                &mut registry,
                model_config.clone(),
                available_tools,
                session_manager,
                brain.permission_config().clone(),
                Some(supervisor.clone()),
                Some(cancel_token.clone()),
                0,
                skill_manager.clone(),
                Some(approval_resolver.clone()),
                Some(brain.todo_lists.clone()),
            );
        }

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

        // Seed both tracks from session history (full archive + model window).
        let full_transcript = history.clone();
        for msg in history {
            context.add(msg);
        }

        // Compute initial stable prefix fingerprint
        let last_prefix_fingerprint = context.stable_prefix_fingerprint();

        // Per-session context snapshot for mid-turn persistence.
        let session_snapshot_path = session_id.as_ref().map(|sid| {
            crate::paths::get_agverse_dir()
                .join("sessions")
                .join(format!("{}.messages.json", sid))
        });

        crate::runtime::approval::register_run_resolver(&id, approval_resolver.clone());

        // Seed shared snapshots so Context Usage / btw work before the first turn.
        // Persistence snapshot = full transcript; usage = live model window.
        *context_snapshot.write() = full_transcript.clone();
        *usage_snapshot.write() = context.usage_snapshot();

        Ok(Self {
            id,
            session_id,
            prompt_id,
            brain,
            skill_manager,
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
            cancel: cancel_token,
            supervisor,
            join_set: JoinSet::new(),
            steering_queue: VecDeque::new(),
            follow_up_queue: VecDeque::new(),
            approval_resolver,
            input_resolver: InputResolver::new(),
            working_dir,
            mode,
            max_iterations,
            last_prefix_fingerprint,
            cache_metrics: CacheMetrics::default(),
            last_turn_end_time: None,
            tool_catalog_cache: None,
            registered_script_tools: Vec::new(),
            session_snapshot_path,
            session_snapshot_gen: Arc::new(AtomicU64::new(0)),
            context_snapshot,
            usage_snapshot,
            full_transcript,
            goal: initial_goal,
            goal_completed: initial_goal_completed,
            goal_continue_nudges: 0,
            execution: ExecutionState::new(),
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

    /// Full (uncompressed) conversation transcript for this Run.
    pub(crate) fn full_transcript(&self) -> &[Message] {
        &self.full_transcript
    }

    /// Append a real conversation message to both the full transcript and the
    /// compactable model window. Compaction must never call this — it only
    /// mutates `context.messages` in place.
    pub(crate) fn append_conversation(&mut self, msg: Message) {
        self.full_transcript.push(msg.clone());
        self.context.add(msg);
    }

    /// Refresh shared snapshots at turn boundaries.
    ///
    /// - `context_snapshot` ← **full transcript** (canonical persist / UI resume)
    /// - `usage_snapshot` ← **live model window** (Context Usage ring)
    ///
    /// Dynamic system segments are reconstructed per turn and must never be
    /// written back as conversation history.
    pub(crate) fn refresh_context_snapshot(&self) {
        *self.context_snapshot.write() = self.full_transcript.clone();
        *self.usage_snapshot.write() = self.context.usage_snapshot();
    }

    /// Todo / plan store for this Run's session.
    pub(crate) fn session_plan_store(&self) -> Arc<crate::todo::SessionPlanStore> {
        self.brain.todo_lists.clone()
    }

    /// Active plan items for this session (detached snapshot for read-only sync).
    pub(crate) fn session_todos(&self) -> Arc<parking_lot::Mutex<crate::todo::TodoList>> {
        Arc::new(parking_lot::Mutex::new(
            self.brain
                .todo_lists
                .active_list(self.session_id.as_deref()),
        ))
    }

    /// Emit TodoUpdated from the current plans snapshot.
    pub(crate) fn emit_plans_updated(&mut self) {
        let snap = self
            .brain
            .todo_lists
            .snapshot(self.session_id.as_deref());
        let items: Vec<crate::runtime::event::TodoItemPayload> = snap
            .items
            .iter()
            .map(|item| crate::runtime::event::TodoItemPayload {
                id: item.id.clone(),
                description: item.description.clone(),
                status: item.status.to_string(),
            })
            .collect();
        let parked: Vec<crate::runtime::event::ParkedPlanPayload> = snap
            .parked
            .iter()
            .map(|p| crate::runtime::event::ParkedPlanPayload {
                id: p.id.clone(),
                title: p.title.clone(),
                completed: p.completed,
                total: p.total,
                updated_at: p.updated_at.clone(),
                source_prompt_id: p.source_prompt_id.clone(),
            })
            .collect();
        self.emit(crate::runtime::event::RunEvent::TodoUpdated {
            items,
            parked,
            active_plan_id: snap.active_plan_id,
            active_plan_title: snap.active_plan_title,
        });
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

    /// Access the per-Run clarification resolver (used by RunManager for direct resolution).
    pub fn input_resolver(&self) -> &InputResolver {
        &self.input_resolver
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
            ts: chrono::Utc::now(),
            seq,
            event_id: uuid::Uuid::new_v4().to_string(),
            run_id: self.id.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.current_turn_id.clone(),
            parent_call_id: None,
            event,
        }
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
        crate::runtime::approval::unregister_run_resolver(&self.id);
        // approval_resolver.clear() is called above (resolvers get dropped error)
    }
}

/// Default directory for Run event logs.
pub(crate) fn default_runs_dir() -> String {
    crate::paths::get_runs_dir().to_string_lossy().into_owned()
}

// ── Helper functions ─────────────────────────────────────────────

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
