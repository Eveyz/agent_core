use anyhow::Result;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub mod executor;
mod scheduler;
use crate::client::OpenAIClient;
use crate::config::{Config, ModelConfig};
use crate::context::Context;
use crate::error_recovery::{RecoveryContext, RecoveryEngine};
use crate::hooks::HookRegistry;
use crate::memory::MemoryManager;
use crate::runtime::brain::Brain;
use crate::runtime::command::{RunCommand, SteerEntry};
use crate::runtime::manager::RunManager;
use crate::permission::PermissionPolicy;
use crate::todo::TodoList;
use crate::prompt::{self, PromptBuilder};
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;
use crate::trace::TraceCollector;
use crate::types::{
    AgentEvent, AgentState, Message, ToolExecutionMode,
};

/// Callback type for transform_context: receives messages, returns transformed messages.
pub type TransformContextFn = Box<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

/// A named, ordered context processor applied to the outgoing message list
/// just before each LLM call. Multiple processors run in registration order;
/// each receives the output of the previous one. This is the lightweight
/// "BeforeModel processor" seam — no trait, no control-flow change, just an
/// ordered list of transforms.
pub struct ContextProcessor {
    pub name: String,
    pub transform: TransformContextFn,
}

pub struct AgentBuilder {
    config: Config,
    id: Option<String>,
    name: Option<String>,
    tools: Vec<Box<dyn crate::tools::Tool>>,
    system_prompt: Option<String>,
    enable_memory: bool,
    permission_policy: Option<PermissionPolicy>,
    hook_registry: Option<HookRegistry>,
    tool_execution_mode: ToolExecutionMode,
    context_processors: Vec<ContextProcessor>,
    recovery: Option<RecoveryEngine>,
    /// Optional directory for execution trace (.jsonl) recording. When set,
    /// every [`AgentEvent`] is appended to `<dir>/<agent_id>.jsonl`.
    trace_dir: Option<String>,
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
}

impl AgentBuilder {
    pub fn from_config(path: &str) -> Result<Self> {
        let config = Config::load(path)?;
        Ok(Self {
            config,
            id: None,
            name: None,
            tools: Vec::new(),
            system_prompt: None,
            enable_memory: false,
            permission_policy: None,
            hook_registry: None,
            tool_execution_mode: ToolExecutionMode::Parallel,
            context_processors: Vec::new(),
            trace_dir: None,
            recovery: None,
            skill_manager: None,
        })
    }

    pub fn from_env() -> Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            config,
            id: None,
            name: None,
            tools: Vec::new(),
            system_prompt: None,
            enable_memory: false,
            permission_policy: None,
            hook_registry: None,
            tool_execution_mode: ToolExecutionMode::Parallel,
            context_processors: Vec::new(),
            trace_dir: None,
            recovery: None,
            skill_manager: None,
        })
    }

    pub fn with_tool(mut self, tool: impl crate::tools::Tool + 'static) -> Self {
        self.tools.push(Box::new(tool));
        self
    }

    pub fn with_id(mut self, id: String) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = Some(prompt.to_string());
        self
    }

    /// Build from an already-loaded Config (useful for tests).
    pub fn with_config(config: Config) -> Self {
        Self {
            config,
            id: None,
            name: None,
            tools: Vec::new(),
            system_prompt: None,
            enable_memory: false,
            permission_policy: None,
            hook_registry: None,
            tool_execution_mode: ToolExecutionMode::Parallel,
            context_processors: Vec::new(),
            trace_dir: None,
            recovery: None,
            skill_manager: None,
        }
    }

    pub fn with_permission_policy(mut self, policy: PermissionPolicy) -> Self {
        self.permission_policy = Some(policy);
        self
    }

    pub fn with_memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    pub fn with_hook_registry(mut self, registry: HookRegistry) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    pub fn with_tool_execution_mode(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution_mode = mode;
        self
    }

    pub fn with_transform_context(
        mut self,
        f: impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync + 'static,
    ) -> Self {
        self.context_processors.push(ContextProcessor {
            name: "transform_context".to_string(),
            transform: Box::new(f),
        });
        self
    }

    /// Append a named context processor. Processors run in registration order
    /// before each model call. See [`ContextProcessor`].
    pub fn with_context_processor(
        mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync + 'static,
    ) -> Self {
        self.context_processors.push(ContextProcessor {
            name: name.into(),
            transform: Box::new(f),
        });
        self
    }

    /// Attach a SkillManager for auto-trigger and catalog management.
    pub fn with_skill_manager(mut self, mgr: Arc<Mutex<SkillManager>>) -> Self {
        self.skill_manager = Some(mgr);
        self
    }

    /// Enable execution-trace recording. Every [`AgentEvent`] emitted during
    /// `run` / `run_with_events` is appended (best-effort, failure-proof) to
    /// `<dir>/<agent_id>.jsonl`. This is a pure side-channel and never alters
    /// agent control flow.
    pub fn with_trace(mut self, dir: impl Into<String>) -> Self {
        self.trace_dir = Some(dir.into());
        self
    }

    /// Attach a custom [`RecoveryEngine`] for loop-level error recovery
    /// (context compaction, token escalation, retry). Defaults to
    /// [`RecoveryEngine::default`] when not set. This complements — and does
    /// not replace — the HTTP-level retry/fallback already performed by
    /// [`crate::client::OpenAIClient`].
    pub fn with_recovery(mut self, engine: RecoveryEngine) -> Self {
        self.recovery = Some(engine);
        self
    }

    pub fn build(mut self) -> Result<Agent> {
        let default_model_name = self.config.default_model.clone();
        let model_config = self.config.default_model()?.clone();

        let system_prompt = if let Some(ref custom) = self.system_prompt {
            custom.clone()
        } else if let Some(ref model_prompt) = model_config.system_prompt {
            model_prompt.clone()
        } else {
            PromptBuilder::new().build()
        };

        let memory: Option<Arc<Mutex<MemoryManager>>> = if self.enable_memory {
            if let Some(ref mem_config) = self.config.memory {
                let m = MemoryManager::new(
                    &mem_config.db_path,
                    &mem_config.embedding_model,
                    mem_config.default_block_max_chars,
                    None,
                )?;
                Some(Arc::new(Mutex::new(m)))
            } else {
                let m =
                    MemoryManager::new(
                        &crate::paths::get_memory_db_path().to_string_lossy(),
                        "BAAI/bge-small-en-v1.5",
                        2000,
                        None,
                    )?;
                Some(Arc::new(Mutex::new(m)))
            }
        } else {
            None
        };

        let mut registry = ToolRegistry::with_defaults();

        if let Some(ref mem) = memory {
            crate::tools::register_memory_tools(&mut registry, mem.clone());
        }

        let todo_list: Arc<Mutex<TodoList>> = Arc::new(Mutex::new(TodoList::new()));
        crate::tools::todo::register_todo_tools(&mut registry, todo_list.clone());

        for tool in self.tools {
            registry.register(tool);
        }

        // ── 7-segment context engine setup ──────────────────────────
        let (identity_text, principles_text) =
            if self.system_prompt.is_some() || model_config.system_prompt.is_some() {
                (
                    system_prompt.clone(),
                    prompt::DEFAULT_PRINCIPLES.to_string(),
                )
            } else {
                (
                    prompt::DEFAULT_IDENTITY.to_string(),
                    prompt::DEFAULT_PRINCIPLES.to_string(),
                )
            };

        let mut context = Context::new(&identity_text, model_config.max_context_tokens);

        // Segment 2: PRINCIPLES
        let perm_mode_str = self
            .permission_policy
            .as_ref()
            .map(|p| format!("{:?}", p.mode()))
            .unwrap_or_else(|| "standard".to_string());
        let full_principles = format!(
            "{}\n\nPermission Mode: {} — tools may require user approval before execution.",
            principles_text, perm_mode_str
        );
        context.set_principles(&full_principles);

        // Segment 3: ENVIRONMENT — initial
        let initial_env = Context::build_environment_string(
            std::env::current_dir()
                .ok()
                .as_ref()
                .and_then(|p| p.to_str()),
            None,
            None,
        );
        context.set_environment(&initial_env);

        // Segment 4: TOOL CATALOG — initial
        let tool_defs = registry.tool_definitions();
        let danger_map = build_danger_map(
            &tool_defs,
            &crate::permission::PermissionPolicy::with_builtin_defaults(),
        );
        let tool_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
        context.set_tool_catalog(&tool_catalog);

        // Segment 5: ACTIVE MEMORY — managed per-turn in refresh_context_segments

        // Segment 6: LOADED SKILLS — catalog of available skills
        if let Some(ref mgr) = self.skill_manager {
            let mgr = mgr.lock();
            let catalog = mgr.build_catalog();
            if !catalog.is_empty() {
                context.set_loaded_skills(&catalog);
            }
        }

        // Build client with fallback chain
        let mut current_model = model_config.clone();
        let mut fallbacks = Vec::new();
        for _ in 0..3 {
            if let Some(ref fallback_name) = current_model.fallback_model {
                if let Some(fallback_cfg) = self.config.get_model(fallback_name) {
                    fallbacks.push(fallback_cfg.clone());
                    current_model = fallback_cfg.clone();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let mut client_opt = None;
        for fallback_cfg in fallbacks.into_iter().rev() {
            if let Some(child) = client_opt {
                client_opt = Some(OpenAIClient::with_fallback(fallback_cfg, Some(child)));
            } else {
                client_opt = Some(OpenAIClient::new(fallback_cfg));
            }
        }

        let client = if let Some(child) = client_opt {
            OpenAIClient::with_fallback(model_config.clone(), Some(child))
        } else {
            OpenAIClient::new(model_config.clone())
        };

        // Build permission policy with built-in defaults if none was provided
        let permission_policy = self.permission_policy.unwrap_or_else(|| {
            let mut policy = PermissionPolicy::with_builtin_defaults();
            // Apply config-level permission settings
            policy = policy.with_config(&self.config.permissions);
            policy
        });

        let id = self.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let name = self.name.unwrap_or_else(|| "Main Orchestrator".to_string());

        // Build the trace collector before the struct literal so `id` can be
        // borrowed here and moved into the agent below.
        let trace = match &self.trace_dir {
            Some(dir) => match TraceCollector::new(dir, &id) {
                Ok(tc) => Some(Arc::new(Mutex::new(tc))),
                Err(e) => {
                    tracing::warn!("failed to initialize trace collector: {e}");
                    None
                }
            },
            None => None,
        };

        // Build the backing runtime (Brain + RunManager) for run delegation.
        let mut brain = Brain::from_config(self.config.clone())?;

        // If AgentBuilder has hooks, share them with the Brain's hook_registry
        // so they fire during Run execution (e.g. BeforeModel hooks).
        let hook_registry = if let Some(agent_hooks) = self.hook_registry.take() {
            let shared = Arc::new(parking_lot::Mutex::new(agent_hooks));
            brain.hook_registry = shared.clone();
            shared
        } else {
            brain.hook_registry.clone()
        };

        let run_manager = RunManager::new(brain);

        Ok(Agent {
            id,
            name,
            config: self.config,
            current_model_name: default_model_name,
            client,
            registry,
            context,
            memory,
            todo_list,
            permission_policy,
            hook_registry,
            state: AgentState::Idle,
            tool_execution_mode: self.tool_execution_mode,
            steering_queue: VecDeque::new(),
            follow_up_queue: VecDeque::new(),
            cancel_token: CancellationToken::new(),
            context_processors: self.context_processors,
            recovery: self.recovery.unwrap_or_default(),
            recovery_ctx: RecoveryContext::new(
                &model_config.model_id,
                model_config.max_context_tokens,
            ),
            skill_manager: self.skill_manager,
            current_session_id: None,
            trace,
            consolidate_counter: 0,
            run_manager: Some(run_manager),
        })
    }
}

pub struct Agent {
    pub id: String,
    pub name: String,
    pub config: Config,
    current_model_name: String,
    client: OpenAIClient,
    registry: ToolRegistry,
    context: Context,
    memory: Option<Arc<Mutex<MemoryManager>>>,
    todo_list: Arc<Mutex<TodoList>>,
    permission_policy: PermissionPolicy,
    hook_registry: Arc<parking_lot::Mutex<HookRegistry>>,
    state: AgentState,
    tool_execution_mode: ToolExecutionMode,
    steering_queue: VecDeque<SteerEntry>,
    follow_up_queue: VecDeque<Message>,
    pub cancel_token: CancellationToken,
    context_processors: Vec<ContextProcessor>,
    /// Loop-level error recovery (context compaction / token escalation / retry).
    recovery: RecoveryEngine,
    recovery_ctx: RecoveryContext,
    /// Skill manager for auto-trigger and catalog
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// Tracks which session's messages are currently loaded in context
    current_session_id: Option<String>,
    /// Optional execution-trace sink (pure side-channel).
    trace: Option<Arc<Mutex<TraceCollector>>>,
    /// Incrementing turn counter used to gate periodic tasks (e.g. consolidation).
    consolidate_counter: u64,
    /// Backing runtime engine. Created at build time; used by run_with_events.
    run_manager: Option<RunManager>,
}

impl Agent {
    /// Run the agent on the given input synchronously (no event callbacks).
    pub async fn run(&mut self, input: &str) -> Result<String> {
        self.run_with_events(input, |_| {}).await
    }

    /// Run the agent, streaming events via the provided callback.
    ///
    /// This delegates to the RunManager/Run runtime, translating
    /// [`crate::runtime::event::RunEvent`]s back into [`AgentEvent`]s
    /// via the existing [`RunEvent::to_agent_event`] bridge.
    #[tracing::instrument(skip_all, fields(input = %input))]
    pub async fn run_with_events(
        &mut self,
        input: &str,
        on_event: impl Fn(AgentEvent) + Send + Sync,
    ) -> Result<String> {
        // Compose the user's event callback with the trace side-channel.
        let trace = self.trace.clone();
        let traced = move |ev: AgentEvent| {
            if let Some(ref tc) = trace {
                let mut guard = tc.lock();
                guard.record(&ev);
            }
            on_event(ev);
        };

        let run_manager = self.run_manager.as_mut()
            .expect("RunManager not initialized");

        // Snapshot current context as history for the Run.
        let history = self.context.messages();

        // ── Skill auto-trigger ──────────────────────────────────────────
        if let Some(ref mgr) = self.skill_manager {
            let matched: Vec<(String, String)> = {
                let mgr = mgr.lock();
                let matched = mgr.check_triggers(input);
                let mut result = Vec::new();
                for skill in &matched {
                    if let Ok(content) = mgr.load_content(skill) {
                        let dir = mgr
                            .source_dir_of(&skill.name)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        result.push((
                            skill.name.clone(),
                            format!(
                                "== Skill: {} (v{}) ==\nSkill directory: {}\n{}\n== End Skill: {} ==\n",
                                skill.name, skill.version, dir, content, skill.name
                            ),
                        ));
                    }
                }
                result
            };

            for (_, text) in &matched {
                self.context.set_loaded_skills(text);
            }

            let mut mgr = mgr.lock();
            for (name, _) in &matched {
                mgr.activate(name);
            }
        }

        // Create a Run via the RunManager.
        let run_id = run_manager.create_run(
            input,
            self.current_session_id.clone(),
            history,
        ).await?;

        let mut event_rx = run_manager.subscribe(&run_id).await?;

        // Send Start command to begin execution.
        run_manager.command(&run_id, RunCommand::Start).await?;

        // Remove the handle after the run to allow join.
        let handle = {
            let mut runs = run_manager.runs_mut().await;
            runs.remove(&run_id)
        };

        traced(AgentEvent::AgentStart);
        self.hook_registry.lock().fire_session_start(&self.id);
        self.state = AgentState::Streaming;

        // ── Event translation loop ─────────────────────────────────────
        let mut final_text = String::new();
        'event_loop: loop {
            match event_rx.recv().await {
                Ok(envelope) => {
                    if let Some(agent_ev) = envelope.event.to_agent_event() {
                        match &agent_ev {
                            AgentEvent::AgentEnd { messages } => {
                                // Sync conversation messages back into Agent's ContextEngine.
                                // Skip system messages — Agent's segments provide the system prompt.
                                self.context.clear();
                                for msg in messages {
                                    match msg.role {
                                        crate::types::Role::User
                                        | crate::types::Role::Assistant
                                        | crate::types::Role::Tool => {
                                            self.context.add(msg.clone());
                                        }
                                        _ => {}
                                    }
                                }
                                // Extract final text from last assistant message.
                                final_text = messages.iter()
                                    .rev()
                                    .find(|m| matches!(m.role, crate::types::Role::Assistant))
                                    .and_then(|m| m.content.clone())
                                    .unwrap_or_default();
                                traced(agent_ev);
                                break 'event_loop;
                            }
                            AgentEvent::Aborted { reason } => {
                                self.state = AgentState::Aborted;
                                final_text = reason.clone();
                                traced(agent_ev);
                                break 'event_loop;
                            }
                            _ => traced(agent_ev),
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break 'event_loop,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lagged = n, "agent event subscriber lagged");
                    continue;
                }
            }
        }

        // Wait for the Run task to fully complete (log flush, reflection, etc.).
        if let Some(handle) = handle {
            let _ = handle.join().await;
        }

        let was_aborted = self.cancel_token.is_cancelled();
        if was_aborted {
            self.state = AgentState::Aborted;
            traced(AgentEvent::Aborted {
                reason: "Cancelled by user".to_string(),
            });
        } else {
            self.state = AgentState::Idle;
        }
        self.steering_queue.clear();
        self.follow_up_queue.clear();
        self.hook_registry.lock().fire_session_end(&self.id);
        traced(AgentEvent::AgentEnd {
            messages: self.context.messages(),
        });

        Ok(final_text)
    }

    /// Build the outgoing message list. Used by tests to verify context
    /// processors chain correctly (the Run applies processors internally).
    fn build_messages(&self) -> Vec<Message> {
        let mut messages = self.context.messages();
        for processor in &self.context_processors {
            messages = (processor.transform)(messages);
        }
        messages
    }

    // ── Public control methods ──────────────────────────────────────

    /// Abort the current agent run. The stream will be cancelled and the loop
    /// will exit at the next iteration boundary.
    pub fn abort(&self) {
        self.cancel_token.cancel();
    }

    /// Get a reference to the current cancel token.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel_token
    }

    /// Replace the cancel token with a new one (for resetting between runs).
    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = token;
    }

    /// Inject a steering message. This is processed after the current turn
    /// completes, before the next LLM call. Use it to redirect the agent
    /// while tools are still running.
    pub fn steer(&mut self, entry: SteerEntry) {
        self.steering_queue.push_back(entry);
    }

    /// Queue a follow-up message. This is processed after the agent would
    /// normally stop (no more tool calls). Use it to add extra work.
    pub fn follow_up(&mut self, message: Message) {
        self.follow_up_queue.push_back(message);
    }

    pub fn clear_steering_queue(&mut self) {
        self.steering_queue.clear();
    }

    pub fn clear_follow_up_queue(&mut self) {
        self.follow_up_queue.clear();
    }

    pub fn clear_all_queues(&mut self) {
        self.steering_queue.clear();
        self.follow_up_queue.clear();
    }

    pub fn state(&self) -> &AgentState {
        &self.state
    }

    pub fn tool_execution_mode(&self) -> ToolExecutionMode {
        self.tool_execution_mode
    }

    pub fn set_tool_execution_mode(&mut self, mode: ToolExecutionMode) {
        self.tool_execution_mode = mode;
    }

    pub fn set_transform_context(
        &mut self,
        f: impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync + 'static,
    ) {
        self.context_processors.push(ContextProcessor {
            name: "transform_context".to_string(),
            transform: Box::new(f),
        });
    }

    /// Append a named context processor at runtime. See
    /// [`AgentBuilder::with_context_processor`].
    pub fn add_context_processor(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<Message>) -> Vec<Message> + Send + Sync + 'static,
    ) {
        self.context_processors.push(ContextProcessor {
            name: name.into(),
            transform: Box::new(f),
        });
    }

    /// Read-only access to the registered context processors.
    pub fn context_processors(&self) -> &[ContextProcessor] {
        &self.context_processors
    }

    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        let model = self
            .config
            .get_model(name)
            .ok_or_else(|| anyhow::anyhow!("model '{}' not found", name))?
            .clone();

        self.current_model_name = name.to_string();
        self.context.set_max_tokens(model.max_context_tokens);
        self.client.switch_model(model);

        Ok(())
    }

    pub fn current_model(&self) -> &str {
        &self.current_model_name
    }

    pub fn list_models(&self) -> Vec<(&str, bool)> {
        self.config
            .models
            .keys()
            .map(|name| (name.as_str(), name == &self.current_model_name))
            .collect()
    }

    /// Register a new model at runtime. Does NOT persist to config.toml —
    /// the caller is responsible for writing the file.
    pub fn register_model(&mut self, name: &str, model: ModelConfig) -> Result<()> {
        if self.config.models.contains_key(name) {
            anyhow::bail!("model '{}' already exists", name);
        }
        self.config.add_model(name.to_string(), model);
        Ok(())
    }

    pub fn model_config(&self, name: &str) -> Option<&ModelConfig> {
        self.config.get_model(name)
    }

    pub fn set_temperature(&mut self, temp: f64) {
        self.client.set_temperature(temp);
    }

    pub fn set_max_tokens(&mut self, max: u32) {
        self.client.set_max_tokens(max);
    }

    pub fn clear_context(&mut self) {
        self.context.clear();
        if let Some(ref mem) = self.memory
            && let mut m = mem.lock()
        {
            m.new_session();
        }
    }

    /// Rewind context to only keep messages up to index `keep_count` (exclusive).
    /// Returns (removed_count, total_before).
    pub fn rewind_context_to(&mut self, keep_count: usize) -> (usize, usize) {
        let total = self.context.raw_messages().len();
        let removed = self.context.truncate_to(keep_count);
        (removed, total)
    }

    pub fn context_token_count(&self) -> usize {
        self.context.current_token_count()
    }

    /// Get KV cache hints for the current context.
    pub fn context_cache_hint(&self) -> Option<crate::context::CacheHint> {
        Some(self.context.cache_hint())
    }

    /// Get a copy of the current conversation messages (for session save).
    pub fn context_messages(&self) -> Vec<Message> {
        self.context.messages()
    }

    /// Get mutable access to the context engine (for session resume).
    pub fn context_mut(&mut self) -> &mut crate::context::ContextEngine {
        &mut self.context
    }

    /// Get the current session ID whose messages are loaded in context.
    pub fn current_session_id(&self) -> Option<&str> {
        self.current_session_id.as_deref()
    }

    /// Set the current session ID (called after loading session history).
    pub fn set_current_session_id(&mut self, id: String) {
        self.current_session_id = Some(id);
    }

    pub fn memory(&self) -> Option<parking_lot::MutexGuard<'_, MemoryManager>> {
        self.memory.as_ref().map(|m| m.lock())
    }

    pub fn memory_enabled(&self) -> bool {
        self.memory.is_some()
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.registry
    }

    pub fn permission_policy(&self) -> &PermissionPolicy {
        &self.permission_policy
    }

    pub fn permission_policy_mut(&mut self) -> &mut PermissionPolicy {
        &mut self.permission_policy
    }

    pub fn hook_registry(&self) -> parking_lot::MutexGuard<'_, HookRegistry> {
        self.hook_registry.lock()
    }

    pub fn hook_registry_mut(&mut self) -> parking_lot::MutexGuard<'_, HookRegistry> {
        self.hook_registry.lock()
    }

    pub fn current_model_config(&self) -> &crate::config::ModelConfig {
        self.config
            .get_model(&self.current_model_name)
            .expect("current model not found in config (was it removed at runtime?)")
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Borrow the HTTP client for offline, read-only reflection work
    /// (e.g. enriching suggestion rationales). The client performs its own
    /// retry/fallback; callers must not mutate agent state through it.
    pub fn client_for_reflection(&self) -> Option<&crate::client::OpenAIClient> {
        Some(&self.client)
    }
}

/// Build a map from tool name → DangerLevel using the permission policy's built-in rules.
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Phase C: `transform_context` is now an ordered, named list. Both the
    /// legacy `with_transform_context` and the new `with_context_processor`
    /// append, and `build_messages` applies them in registration order.
    #[test]
    fn test_context_processors_registered_in_order() {
        let agent = AgentBuilder::with_config(test_config())
            .with_transform_context(|msgs| msgs)
            .with_context_processor("trim", |msgs| msgs)
            .with_context_processor("enrich", |msgs| msgs)
            .with_memory(false)
            .build()
            .unwrap();

        let names: Vec<&str> = agent
            .context_processors()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, vec!["transform_context", "trim", "enrich"]);
    }

    /// Phase C: `build_messages` applies processors in order, each receiving
    /// the previous one's output.
    #[test]
    fn test_build_messages_applies_processors_in_order() {
        // Processor 1 tags messages with "[a]"; processor 2 tags with "[b]".
        // The final system-prompt-bearing message should carry both tags in
        // order, proving sequential composition.
        let agent = AgentBuilder::with_config(test_config())
            .with_context_processor("a", |mut msgs| {
                for m in msgs.iter_mut() {
                    if let Some(c) = m.content.as_mut() {
                        c.push_str("[a]");
                    }
                }
                msgs
            })
            .with_context_processor("b", |mut msgs| {
                for m in msgs.iter_mut() {
                    if let Some(c) = m.content.as_mut() {
                        c.push_str("[b]");
                    }
                }
                msgs
            })
            .with_memory(false)
            .build()
            .unwrap();

        let messages = agent.build_messages();
        // The context always has at least the identity system prompt.
        let any_tagged = messages.iter().any(|m| {
            m.content
                .as_ref()
                .map(|c| c.contains("[a][b]"))
                .unwrap_or(false)
        });
        assert!(
            any_tagged,
            "processors not applied in order: {:?}",
            messages
        );
    }

    /// Phase D: enabling tracing via the builder must not break the build.
    #[test]
    fn test_trace_enabled_builds() {
        let dir = std::env::temp_dir().join(format!(
            "agent_core_trace_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let agent = AgentBuilder::with_config(test_config())
            .with_trace(dir.to_str().unwrap())
            .with_memory(false)
            .build()
            .unwrap();
        // The trace file path is <dir>/<agent_id>.jsonl.
        let expected = dir.join(format!("{}.jsonl", agent.id));
        assert!(expected.exists(), "trace file not created: {expected:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    use crate::hooks::{Hook, HookAction, HookEvent};
    use crate::types::AgentEvent;
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// A hook that short-circuits the model with a fixed preset answer, so
    /// `run_turn` can be exercised end-to-end without a live LLM.
    struct PresetAnswerHook {
        answer: String,
    }

    impl Hook for PresetAnswerHook {
        fn name(&self) -> &str {
            "preset_answer"
        }
        fn handle(&self, event: &HookEvent) -> Option<HookAction> {
            if let HookEvent::BeforeModel { .. } = event {
                Some(HookAction::SkipModel {
                    preset_text: self.answer.clone(),
                })
            } else {
                None
            }
        }
    }

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

    /// Phase E + B: a turn segmented into stages completes via the BeforeModel
    /// SkipModel short-circuit, returning the preset answer and emitting the
    /// expected event ordering (TurnStart → MessageEnd → TurnEnd → AgentEnd).
    #[tokio::test]
    async fn run_turn_completes_with_preset_answer() {
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let ev_clone = events.clone();

        let mut agent = AgentBuilder::with_config(test_config())
            .with_memory(false)
            .build()
            .unwrap();
        agent
            .hook_registry_mut()
            .register(Box::new(PresetAnswerHook {
                answer: "42".to_string(),
            }));

        let result = agent
            .run_with_events("what is the answer", move |ev| {
                let tag = match ev {
                    AgentEvent::AgentStart => "AgentStart",
                    AgentEvent::TurnStart { .. } => "TurnStart",
                    AgentEvent::MessageEnd { .. } => "MessageEnd",
                    AgentEvent::TurnEnd { .. } => "TurnEnd",
                    AgentEvent::AgentEnd { .. } => "AgentEnd",
                    AgentEvent::Error(_) => "Error",
                    _ => return,
                };
                ev_clone.lock().push(tag);
            })
            .await
            .unwrap();

        assert_eq!(result, "42");
        let recorded = events.lock().clone();
        assert_eq!(
            recorded,
            vec![
                "AgentStart",
                "TurnStart",
                "MessageEnd",
                "TurnEnd",
                "AgentEnd"
            ],
            "event ordering must be unchanged by Phase E staging"
        );
    }

    /// Phase E: the final-answer branch (no tool calls) is reachable and
    /// terminates the run after exactly one turn.
    #[tokio::test]
    async fn run_turn_final_answer_stops_loop() {
        let turn_count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let tc_clone = turn_count.clone();

        let mut agent = AgentBuilder::with_config(test_config())
            .with_memory(false)
            .build()
            .unwrap();
        agent
            .hook_registry_mut()
            .register(Box::new(PresetAnswerHook {
                answer: "done".to_string(),
            }));

        let result = agent
            .run_with_events("hi", move |ev| {
                if matches!(ev, AgentEvent::TurnStart { .. }) {
                    *tc_clone.lock() += 1;
                }
            })
            .await
            .unwrap();

        assert_eq!(result, "done");
        // The final-answer branch returns after the first turn.
        assert_eq!(*turn_count.lock(), 1);
    }
}
