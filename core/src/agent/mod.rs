use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub mod executor;
mod scheduler;
use crate::client::OpenAIClient;
use crate::config::{Config, ModelConfig};
use crate::context::Context;
use crate::error_recovery::{RecoveryAction, RecoveryContext, RecoveryEngine};
use crate::hooks::{HookRegistry, PreToolResult};
use crate::memory::MemoryManager;
use crate::permission::{
    ApprovalChoice, ApprovalScope, PermissionDecision, PermissionPolicy, ToolPermissionPattern,
    WhitelistEntry,
};
use crate::skills::SkillManager;
use crate::trace::TraceCollector;
use crate::prompt::{self, PromptBuilder};
use crate::tools::{ToolRegistry, ToolUpdateFn};
use crate::types::{
    AgentEvent, AgentState, Message, MessageDelta, StreamEvent, ToolCall, ToolExecutionMode,
    ToolResultRecord,
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


    pub fn build(self) -> Result<Agent> {
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
                )?;
                Some(Arc::new(Mutex::new(m)))
            } else {
                let m =
                    MemoryManager::new("~/.agent_core/memory.db", "BAAI/bge-small-en-v1.5", 2000)?;
                Some(Arc::new(Mutex::new(m)))
            }
        } else {
            None
        };

        let mut registry = ToolRegistry::with_defaults();

        if let Some(ref mem) = memory {
            crate::tools::register_memory_tools(&mut registry, mem.clone());
        }

        for tool in self.tools {
            registry.register(tool);
        }

        // ── 7-segment context engine setup ──────────────────────────
        let (identity_text, principles_text) = if self.system_prompt.is_some()
            || model_config.system_prompt.is_some()
        {
            (system_prompt.clone(), prompt::DEFAULT_PRINCIPLES.to_string())
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

        // Segment 5: ACTIVE MEMORY
        if let Some(ref mem) = memory
            && let Ok(m) = mem.lock()
        {
            let core_memory_str = m.core().to_context_string();
            if !core_memory_str.is_empty() {
                context.set_active_memory(&core_memory_str);
            }
        }

        // Segment 6: LOADED SKILLS — catalog of available skills
        if let Some(ref mgr) = self.skill_manager {
            if let Ok(mgr) = mgr.lock() {
                let catalog = mgr.build_catalog();
                if !catalog.is_empty() {
                    context.set_loaded_skills(&catalog);
                }
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

        Ok(Agent {
            id,
            name,
            config: self.config,
            current_model_name: default_model_name,
            client,
            registry,
            context,
            memory,
            permission_policy,
            hook_registry: self.hook_registry.unwrap_or_default(),
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
    permission_policy: PermissionPolicy,
    hook_registry: HookRegistry,
    state: AgentState,
    tool_execution_mode: ToolExecutionMode,
    steering_queue: VecDeque<Message>,
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
}

/// Result of a single recovery attempt within `model_turn`.
enum RecoveryOutcome {
    Retry,
    GiveUp,
}

/// Named stages of a single turn. Used for structured logging and as the
/// spine for future per-stage hooks. Introduced in Phase E so the control
/// flow is explicit without rewriting the loop as an external pipeline.
#[derive(Debug, Clone, Copy)]
enum Stage {
    Refresh,
    Compact,
    Model,
    Dispatch,
    Execute,
    Observe,
}

impl Stage {
    const fn as_str(self) -> &'static str {
        match self {
            Stage::Refresh => "refresh",
            Stage::Compact => "compact",
            Stage::Model => "model",
            Stage::Dispatch => "dispatch",
            Stage::Execute => "execute",
            Stage::Observe => "observe",
        }
    }
}

/// Outcome of a single turn, returned by [`Agent::run_turn`].
enum TurnOutcome {
    /// The agent produced a final answer; the run is complete.
    Final(String),
    /// The turn completed and the loop should continue.
    Continue,
    /// The run must stop with this user-facing message.
    Stop(String),
}

impl Agent {
    pub async fn run(&mut self, input: &str) -> Result<String> {
        self.run_with_events(input, |_| {}).await
    }

    #[tracing::instrument(skip_all, fields(input = %input))]
    pub async fn run_with_events(
        &mut self,
        input: &str,
        on_event: impl Fn(AgentEvent) + Send + Sync,
    ) -> Result<String> {
        // Compose the user's event callback with the trace side-channel so
        // tracing is transparent to callers. Recording is best-effort and
        // never propagates errors into the run.
        let trace = self.trace.clone();
        let traced = move |ev: AgentEvent| {
            if let Some(ref tc) = trace {
                if let Ok(mut guard) = tc.lock() {
                    guard.record(&ev);
                }
            }
            on_event(ev);
        };

        self.context.add(Message::user(input));

        if let Some(ref mem) = self.memory
            && let Ok(m) = mem.lock()
        {
            let _ = m.store_conversation("user", input);
        }

        // ── Skill auto-trigger ──────────────────────────────────────────
        // Check user message against skill triggers, auto-load matching skills
        if let Some(ref mgr) = self.skill_manager {
            let matched: Vec<(String, String)> = {
                let mgr = mgr.lock().unwrap();
                let matched = mgr.check_triggers(input);
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

            // Inject into context
            for (_, text) in &matched {
                self.context.set_loaded_skills(text);
            }

            // Activate matched skills
            if let Ok(mut mgr) = mgr.lock() {
                for (name, _) in &matched {
                    mgr.activate(name);
                }
            }
        }

        traced(AgentEvent::AgentStart);
        self.hook_registry.fire_session_start(&self.id);
        self.state = AgentState::Streaming;

        let result = self.run_loop(&traced).await;

        // The cancel token is owned by the caller (Tauri/CLI), which creates a
        // fresh one before each run. Core must NOT recreate it here — doing so
        // would replace the token the caller holds, making every abort cancel a
        // stale instance while the running loop watches a different one.
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
        self.hook_registry.fire_session_end(&self.id);
        traced(AgentEvent::AgentEnd {
            messages: self.context.messages(),
        });

        result
    }

    #[tracing::instrument(skip_all)]
    async fn run_loop(&mut self, on_event: &(impl Fn(AgentEvent) + Send + Sync)) -> Result<String> {
        let max_iterations = self.client.model.max_iterations;

        for turn_index in 0..max_iterations {
            tracing::info!(turn = turn_index, "Starting turn {}", turn_index);
            if self.cancel_token.is_cancelled() {
                self.state = AgentState::Aborted;
                return Ok("Agent aborted by user during tool execution.".to_string());
            }
            on_event(AgentEvent::TurnStart { turn_index });
            self.hook_registry.fire_turn_start(turn_index);

            match self.run_turn(turn_index, max_iterations, on_event).await {
                TurnOutcome::Final(text) => return Ok(text),
                TurnOutcome::Continue => {}
                TurnOutcome::Stop(msg) => return Ok(msg),
            }
        }

        bail!("unexpected end of agent loop")
    }

    /// Execute one turn of the ReAct loop, segmented into named stages:
    /// `Refresh → Compact → Model → Dispatch → Execute → Observe`.
    ///
    /// Control flow is identical to the pre-Phase-E inline implementation;
    /// the staging is structural (named stages + logging) only, with no
    /// external processor abstraction.
    async fn run_turn(
        &mut self,
        turn_index: usize,
        max_iterations: usize,
        on_event: &(impl Fn(AgentEvent) + Send + Sync),
    ) -> TurnOutcome {
        // ── Stage: Refresh ───────────────────────────────────────────
        tracing::debug!(turn = turn_index, stage = Stage::Refresh.as_str());
        self.refresh_context_segments();
        self.context.trim_to_fit();

        // ── Stage: Compact ───────────────────────────────────────────
        // LLM compaction: if still near limit, summarize old turns.
        tracing::debug!(turn = turn_index, stage = Stage::Compact.as_str());
        self.maybe_llm_compact().await;

        // ── Stage: Model ─────────────────────────────────────────────
        // Invoke the model with loop-level recovery (Phase A). This
        // centralizes HTTP-stream errors and applies context compaction /
        // token escalation / retry instead of abandoning the turn on the
        // first failure.
        tracing::debug!(turn = turn_index, stage = Stage::Model.as_str());
        let (text, tool_calls) = match self.model_turn(on_event).await {
            Ok(r) => r,
            Err(e) => {
                if self.cancel_token.is_cancelled() {
                    return TurnOutcome::Stop("Agent aborted by user.".to_string());
                }
                on_event(AgentEvent::Error(e.clone()));
                return TurnOutcome::Stop(format!(
                    "I encountered an error communicating with the model: {e}. Please try again."
                ));
            }
        };

        // ── Stage: Dispatch ──────────────────────────────────────────
        tracing::debug!(turn = turn_index, stage = Stage::Dispatch.as_str());
        if tool_calls.is_empty() {
            // No tool calls — this is the final answer.
            let assistant_msg = Message::assistant(&text);
            self.context.add(assistant_msg.clone());
            on_event(AgentEvent::MessageEnd {
                message: assistant_msg.clone(),
            });
            on_event(AgentEvent::TurnEnd {
                turn_index,
                assistant_message: assistant_msg,
                tool_results: vec![],
            });
            self.hook_registry.fire_turn_end(turn_index);

            if let Some(ref mem) = self.memory {
                if let Ok(m) = mem.lock() {
                    let _ = m.store_conversation("assistant", &text);
                }
                self.refresh_core_memory_in_context();
                self.maybe_consolidate();
            }

            // Check for follow-up messages
            if let Some(follow_up) = self.follow_up_queue.pop_front() {
                self.context.add(follow_up);
                return TurnOutcome::Continue;
            }

            return TurnOutcome::Final(text);
        }

        // Add assistant message with tool calls
        let assistant_msg = if !text.is_empty() {
            let msg = Message::assistant_with_tools(&text, tool_calls.clone());
            self.context.add(msg.clone());
            msg
        } else {
            let msg = Message::assistant_with_tools("", tool_calls.clone());
            self.context.add(msg.clone());
            msg
        };

        on_event(AgentEvent::MessageEnd {
            message: assistant_msg.clone(),
        });

        // ── Stage: Execute ───────────────────────────────────────────
        tracing::debug!(turn = turn_index, stage = Stage::Execute.as_str());
        self.state = AgentState::ExecutingTools;
        let tool_results = {
            let mut orchestrator = executor::ToolOrchestrator {
                registry: &self.registry,
                permission_policy: &mut self.permission_policy,
                hook_registry: &mut self.hook_registry,
                tool_execution_mode: self.tool_execution_mode,
                cancel_token: self.cancel_token.clone(),
                approval_resolver: None,
            };
            orchestrator.execute_tools(&tool_calls, &on_event).await
        };
        self.state = AgentState::Streaming;

        // ── Stage: Observe ───────────────────────────────────────────
        tracing::debug!(turn = turn_index, stage = Stage::Observe.as_str());
        let mut result_records = Vec::new();
        for (call, result) in tool_calls.iter().zip(&tool_results) {
            let is_error = result.starts_with("Error")
                || result.starts_with("Permission denied")
                || result.starts_with("Hook vetoed");

            result_records.push(ToolResultRecord {
                tool_call_id: call.id.clone(),
                tool_name: call.function.name.clone(),
                result: result.clone(),
                is_error,
            });

            on_event(AgentEvent::ToolExecutionEnd {
                tool_call_id: call.id.clone(),
                tool_name: call.function.name.clone(),
                result: result.clone(),
                is_error,
            });

            self.context
                .add(Message::tool(call.id.clone(), result.clone()));
        }

        on_event(AgentEvent::TurnEnd {
            turn_index,
            assistant_message: assistant_msg,
            tool_results: result_records,
        });
        self.hook_registry.fire_turn_end(turn_index);

        // Check for steering messages (injected before next LLM call)
        if let Some(steer_msg) = self.steering_queue.pop_front() {
            self.context.add(steer_msg);
        }

        // Check for follow-up messages (only when no more tool calls pending)
        if let Some(follow_up) = self.follow_up_queue.pop_front() {
            self.context.add(follow_up);
        }

        if turn_index == max_iterations - 1 {
            let summary = build_iteration_limit_summary(&self.context, max_iterations);
            on_event(AgentEvent::Error(summary.clone()));
            return TurnOutcome::Stop(summary);
        }

        TurnOutcome::Continue
    }



    /// Build the outgoing message list for this turn: refresh-independent
    /// snapshot of context messages with optional `transform_context` applied.
    fn build_messages(&self) -> Vec<Message> {
        let mut messages = self.context.messages();
        for processor in &self.context_processors {
            messages = (processor.transform)(messages);
        }
        messages
    }

    /// Serialize messages to JSON values for the `BeforeModel` hook snapshot.
    /// Only the role + a truncated content preview is kept to bound cost.
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
                serde_json::json!({ "role": format!("{:?}", m.role), "preview": preview })
            })
            .collect()
    }

    /// Invoke the model for one turn with loop-level recovery.
    ///
    /// This wraps `chat_completion_stream` + `collect_stream` and consults the
    /// [`RecoveryEngine`] on failure. Unlike the HTTP-layer retry/fallback in
    /// [`crate::client::OpenAIClient`], this handles *whole-turn* recovery:
    /// context-too-long (compact then retry), token truncation (escalate
    /// `max_tokens` then retry), and a bounded number of generic retries.
    async fn model_turn(
        &mut self,
        on_event: &(impl Fn(AgentEvent) + Send + Sync),
    ) -> Result<(String, Vec<ToolCall>), String> {
        const MAX_RECOVERY_ATTEMPTS: u32 = 3;

        for _attempt in 0..MAX_RECOVERY_ATTEMPTS {
            if self.cancel_token.is_cancelled() {
                return Err("aborted".to_string());
            }

            let messages = self.build_messages();
            let tools = self.registry.tool_definitions();

            // BeforeModel hook: allows SkipModel short-circuit (testing/cache).
            let snapshot = self.snapshot_messages_for_hook(&messages);
            if let Some(preset) = self.hook_registry.fire_before_model(&snapshot) {
                self.recovery_ctx.record_success();
                self.hook_registry.fire_after_model(&preset, 0);
                return Ok((preset, Vec::new()));
            }

            // Acquire the stream. On error we propagate an owned String so the
            // immutable borrow of `self.client` ends before we call the
            // `&mut self` recovery path below.
            let stream = self
                .client
                .chat_completion_stream(&messages, &tools)
                .await
                .map_err(|e| format!("LLM request failed: {e}"))?;

            // Collect the stream within a scope that only borrows `self`
            // immutably (cancel token). The result is owned, so the borrow is
            // released by the time we reach the recovery path.
            let collected: Result<(String, Vec<ToolCall>), String> = {
                let cancel = self.cancel_token.clone();
                let res = self.collect_stream(stream, on_event).await;
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
                Ok((text, tool_calls)) => {
                    self.recovery_ctx.record_success();
                    self.hook_registry
                        .fire_after_model(&text, tool_calls.len());
                    return Ok((text, tool_calls));
                }
                Err(msg) => {
                    self.recovery_ctx.record_error(&msg);
                    match self.try_recover(&msg, on_event).await {
                        RecoveryOutcome::Retry => continue,
                        RecoveryOutcome::GiveUp => return Err(msg),
                    }
                }
            }
        }

        Err("exhausted recovery attempts".to_string())
    }

    /// Apply one recovery action based on the current [`RecoveryContext`].
    /// Returns whether the turn should be retried or given up.
    async fn try_recover(
        &mut self,
        error: &str,
        on_event: &(impl Fn(AgentEvent) + Send + Sync),
    ) -> RecoveryOutcome {
        let action = self.recovery.determine_strategy(&self.recovery_ctx);
        match action {
            RecoveryAction::CompactContext { target_ratio } => {
                on_event(AgentEvent::Error(format!(
                    "context too long; compacting to {:.0}% before retry",
                    target_ratio * 100.0
                )));
                self.force_compact(target_ratio).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::EscalateTokens { new_max_tokens } => {
                on_event(AgentEvent::Error(format!(
                    "escalating max_tokens to {new_max_tokens}"
                )));
                self.client.set_max_tokens(new_max_tokens);
                RecoveryOutcome::Retry
            }
            RecoveryAction::Retry { delay_ms } => {
                on_event(AgentEvent::Error(format!(
                    "retrying model call after {delay_ms}ms"
                )));
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::SwitchModel { model } => {
                if self.config.get_model(&model).is_some()
                    && self.switch_model(&model).is_ok()
                {
                    on_event(AgentEvent::Error(format!(
                        "switched to fallback model: {model}"
                    )));
                    RecoveryOutcome::Retry
                } else {
                    on_event(AgentEvent::Error(format!(
                        "recovery requested fallback model '{model}' which is unavailable; giving up: {error}"
                    )));
                    RecoveryOutcome::GiveUp
                }
            }
            RecoveryAction::Fail => {
                on_event(AgentEvent::Error(format!("unrecoverable: {error}")));
                RecoveryOutcome::GiveUp
            }
        }
    }

    /// Force an LLM compaction of the oldest turns regardless of current token
    /// count. `target_ratio` is the desired remaining fraction of context after
    /// compaction (e.g. 0.8 means summarize the oldest 20%).
    async fn force_compact(&mut self, target_ratio: f64) {
        let remove_fraction = (1.0 - target_ratio).clamp(0.1, 0.6);
        let num_turns = (self.context.len().max(4) as f64 * remove_fraction) as usize;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => return,
        };
        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => return,
        };
        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => return,
        };
        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }

    async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        on_event: &(impl Fn(AgentEvent) + Send + Sync),
    ) -> Result<(String, Vec<ToolCall>)> {
        use crate::client::streaming::ToolCallAccumulator;

        let mut text_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;
        let cancel = self.cancel_token.clone();

        tokio::pin!(stream);
        loop {
            tokio::select! {
                // Active cancellation: wake immediately when the token fires,
                // even if the model stream is stalled (network block, server
                // holding the connection open with no tokens). The old
                // reactive check only ran after `stream.next()` returned a new
                // event, so a stalled stream could never be interrupted.
                _ = cancel.cancelled() => {
                    return Err(anyhow::anyhow!("aborted"));
                }
                next = stream.next() => {
                    let event = match next {
                        None => break,
                        Some(e) => e?,
                    };
                    match event {
                        StreamEvent::ThinkingDelta(delta) => {
                            if !delta.is_empty() {
                                on_event(AgentEvent::MessageUpdate {
                                    delta: MessageDelta::Thinking(delta),
                                });
                            }
                        }
                        StreamEvent::TextDelta(delta) => {
                            if !delta.is_empty() {
                                text_buffer.push_str(&delta);
                                on_event(AgentEvent::MessageUpdate {
                                    delta: MessageDelta::Text(delta),
                                });
                            }
                        }
                        StreamEvent::ToolCallDelta { .. } => {
                            has_tool_calls = true;
                            accumulator.push(event);
                        }
                        StreamEvent::Done => break,
                    }
                }
            }
        }

        let tool_calls = if has_tool_calls {
            accumulator.into_tool_calls()
        } else {
            vec![]
        };

        Ok((text_buffer, tool_calls))
    }

    fn refresh_core_memory_in_context(&mut self) {
        if let Some(ref mem) = self.memory
            && let Ok(m) = mem.lock()
        {
            let core_str = m.core().to_context_string();
            self.context.set_active_memory(&core_str);
        }
    }

    /// Refresh per-turn context segments: environment, tool catalog,
    /// active memory, and execution plan.
    fn refresh_context_segments(&mut self) {
        // Segment 3: ENVIRONMENT
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()));
        let env_str = Context::build_environment_string(cwd.as_deref(), None, None);
        self.context.set_environment(&env_str);

        // Segment 4: TOOL CATALOG
        let tool_defs = self.registry.tool_definitions();
        let danger_map = build_danger_map(&tool_defs, &self.permission_policy);
        let tool_catalog = Context::build_tool_catalog_string(&tool_defs, &danger_map);
        self.context.set_tool_catalog(&tool_catalog);

        // Segment 5: ACTIVE MEMORY — refresh from memory manager if enabled
        if let Some(ref mem) = self.memory
            && let Ok(m) = mem.lock()
        {
            let core_str = m.core().to_context_string();
            if !core_str.is_empty() {
                self.context.set_active_memory(&core_str);
            }
        }

        // Segment 7: EXECUTION PLAN — placeholder, can be filled by external hooks
        // (the todo/task modules can push updates via set_execution_plan)
    }

    /// Stage 4 LLM compaction: if tokens are critically high (>95%),
    /// request an LLM summary of old turns and apply it.
    /// Also writes the summary to Recall Memory for long-term retention.
    async fn maybe_llm_compact(&mut self) {
        let current = self.context.current_token_count();
        let critical = (self.client.model.max_context_tokens as f64 * 0.95) as usize;

        if current < critical {
            return;
        }

        // Summarize the oldest ~40% of turns
        let num_turns = self.context.len().max(4) * 2 / 5;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => return,
        };

        // Build a lightweight summarization request
        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => return, // summarization failed, skip
        };

        // Parse the LLM response as TurnSummary
        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => return, // couldn't parse summary
        };

        // Apply summary — replace old messages with compressed version
        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }

    fn maybe_consolidate(&self) {
        if let Some(ref mem) = self.memory {
            let should_consolidate = match mem.lock() {
                Ok(m) => !m.session_id().is_empty(),
                Err(_) => false,
            };

            if should_consolidate {
                let memory = mem.clone();
                tokio::spawn(async move {
                    let result = match memory.lock() {
                        Ok(m) => m.consolidate(),
                        Err(e) => {
                            eprintln!("[memory] lock error during consolidation: {e}");
                            return;
                        }
                    };
                    match result {
                        Ok(report) => {
                            if report.deduped_recall > 0 || report.deduped_archival > 0 {
                                eprintln!(
                                    "[memory] consolidated: {} recall, {} archival records removed",
                                    report.deduped_recall, report.deduped_archival
                                );
                            }
                        }
                        Err(e) => eprintln!("[memory] consolidation error: {e}"),
                    }
                });
            }
        }
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
    pub fn steer(&mut self, message: Message) {
        self.steering_queue.push_back(message);
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
            && let Ok(mut m) = mem.lock()
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

    pub fn memory(&self) -> Option<std::sync::MutexGuard<'_, MemoryManager>> {
        self.memory.as_ref().and_then(|m| m.lock().ok())
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

    pub fn hook_registry(&self) -> &HookRegistry {
        &self.hook_registry
    }

    pub fn hook_registry_mut(&mut self) -> &mut HookRegistry {
        &mut self.hook_registry
    }

    pub fn current_model_config(&self) -> &crate::config::ModelConfig {
        self.config.get_model(&self.current_model_name).unwrap()
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

fn build_iteration_limit_summary(
    context: &crate::context::Context,
    max_iterations: usize,
) -> String {
    let messages = context.messages();

    let user_request = messages
        .iter()
        .rfind(|m| m.role == crate::types::Role::User)
        .map(|m| m.content.as_deref().unwrap_or(""))
        .unwrap_or("your request");

    let mut tool_counts: HashMap<&str, usize> = HashMap::new();
    let mut tool_errors: Vec<&str> = Vec::new();
    let mut total_tool_calls = 0;

    for msg in &messages {
        if msg.role == crate::types::Role::Assistant
            && let Some(ref calls) = msg.tool_calls
        {
            for call in calls {
                *tool_counts.entry(&call.function.name).or_insert(0) += 1;
                total_tool_calls += 1;
            }
        }
        if msg.role == crate::types::Role::Tool
            && let Some(ref content) = msg.content
        {
            let trimmed = content.trim();
            if (trimmed.starts_with("Error") || trimmed.starts_with("Failed to fetch"))
                && let Some(ref name) = msg.name
            {
                tool_errors.push(name);
            }
        }
    }

    let tool_summary: Vec<String> = tool_counts
        .iter()
        .map(|(name, count)| format!("  - {name}: {count} time(s)"))
        .collect();

    let mut msg = format!(
        "I've reached the maximum number of steps ({max_iterations}) while working on: \"{user_request}\"\n\n",
    );

    if !tool_summary.is_empty() {
        msg.push_str(&format!(
            "Here is a summary of the tools used:\n{}\n\n",
            tool_summary.join("\n")
        ));
    }

    let webfetch_count = tool_counts.get("webfetch").copied().unwrap_or(0);
    let webfetch_errors = tool_errors.iter().filter(|&&n| n == "webfetch").count();

    if total_tool_calls == 0 {
        msg.push_str("I wasn't able to take any action. This may indicate an issue with tool availability or model configuration.");
    } else if webfetch_count > 0 && webfetch_errors as f64 >= webfetch_count as f64 * 0.5 {
        msg.push_str(&format!(
            "Web fetching was attempted {webfetch_count} time(s) but encountered {webfetch_errors} errors. \
             Many websites block automated access or return errors when fetched programmatically. \
             To get better results:\n\
             - Provide the information you're looking for directly rather than URLs to fetch\n\
             - Use a search-based approach or specify known accessible URLs\n\
             - If you have a specific webpage in mind, paste its relevant content instead of asking me to fetch it\n\
             - Consider using an API or RSS feed endpoint instead of a regular webpage"
        ));
    } else if tool_errors.len() as f64 >= total_tool_calls as f64 * 0.5 {
        msg.push_str(&format!(
            "{total_tool_calls} total tool call(s) were made with high failure rate. \
             Please provide more specific guidance or simplify the task so I can complete it within the step limit."
        ));
    } else {
        msg.push_str(&format!(
            "The task may be more complex than can be completed in {max_iterations} steps. \
             Please try breaking it down into smaller sub-tasks, or provide more specific instructions."
        ));
    }

    msg
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
        assert!(any_tagged, "processors not applied in order: {:?}", messages);
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
    use std::sync::{Arc, Mutex};

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
                ev_clone.lock().unwrap().push(tag);
            })
            .await
            .unwrap();

        assert_eq!(result, "42");
        let recorded = events.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["AgentStart", "TurnStart", "MessageEnd", "TurnEnd", "AgentEnd"],
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
                    *tc_clone.lock().unwrap() += 1;
                }
            })
            .await
            .unwrap();

        assert_eq!(result, "done");
        // The final-answer branch returns after the first turn.
        assert_eq!(*turn_count.lock().unwrap(), 1);
    }
}
