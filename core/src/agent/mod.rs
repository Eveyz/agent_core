use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub mod executor;
use crate::client::OpenAIClient;
use crate::config::{Config, ModelConfig};
use crate::context::Context;
use crate::hooks::{HookRegistry, PreToolResult};
use crate::memory::MemoryManager;
use crate::permission::{
    ApprovalChoice, ApprovalScope, PermissionDecision, PermissionPolicy, ToolPermissionPattern,
    WhitelistEntry,
};
use crate::skills::SkillManager;
use crate::prompt::{self, PromptBuilder};
use crate::tools::{ToolRegistry, ToolUpdateFn};
use crate::types::{
    AgentEvent, AgentState, Message, MessageDelta, StreamEvent, ToolCall, ToolExecutionMode,
    ToolResultRecord,
};

/// Callback type for transform_context: receives messages, returns transformed messages.
pub type TransformContextFn = Box<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

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
    transform_context: Option<TransformContextFn>,
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// Share approvals map from parent agent — subagents use this to avoid
    /// deadlocks when awaiting approval in TUI mode.
    pending_approvals_override: Option<
        Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalChoice>>>>,
    >,
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
            transform_context: None,
            skill_manager: None,
            pending_approvals_override: None,
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
            transform_context: None,
            skill_manager: None,
            pending_approvals_override: None,
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
            transform_context: None,
            skill_manager: None,
            pending_approvals_override: None,
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
        self.transform_context = Some(Box::new(f));
        self
    }

    /// Attach a SkillManager for auto-trigger and catalog management.
    pub fn with_skill_manager(mut self, mgr: Arc<Mutex<SkillManager>>) -> Self {
        self.skill_manager = Some(mgr);
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
            transform_context: self.transform_context,
            skill_manager: self.skill_manager,
            current_session_id: None,
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
    transform_context: Option<TransformContextFn>,
    /// Skill manager for auto-trigger and catalog
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// Tracks which session's messages are currently loaded in context
    current_session_id: Option<String>,
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

        on_event(AgentEvent::AgentStart);
        self.state = AgentState::Streaming;
        // Token is one-shot, but if we need to reset we could create a new one, 
        // however usually a new run uses the same token or creates a new one. 
        // Here we just ensure we have a fresh token.
        self.cancel_token = CancellationToken::new();

        let result = self.run_loop(&on_event).await;

        self.state = AgentState::Idle;
        self.steering_queue.clear();
        self.follow_up_queue.clear();
        on_event(AgentEvent::AgentEnd {
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
            }       on_event(AgentEvent::TurnStart { turn_index });

            // Refresh per-turn context segments
            self.refresh_context_segments();

            self.context.trim_to_fit();

            // Stage 4 LLM compaction: if still near limit, summarize old turns
            self.maybe_llm_compact().await;

            // Build messages with optional transform
            let raw_messages = self.context.messages();
            let messages = if let Some(ref transform) = self.transform_context {
                transform(raw_messages)
            } else {
                raw_messages
            };
            let tools = self.registry.tool_definitions();

            let stream = match self.client.chat_completion_stream(&messages, &tools).await {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = format!("LLM request failed: {e}");
                    on_event(AgentEvent::Error(err_msg.clone()));
                    return Ok(format!(
                        "I encountered an error communicating with the model: {e}. Please try again."
                    ));
                }
            };

            let (text, tool_calls) = match self.collect_stream(stream, &on_event).await {
                Ok(r) => r,
                Err(e) => {
                    if self.cancel_token.is_cancelled() {
                        return Ok("Agent aborted by user.".to_string());
                    }
                    let err_msg = format!("Stream error: {e}");
                    on_event(AgentEvent::Error(err_msg));
                    return Ok(format!(
                        "I encountered an error reading the model response: {e}. Please try again."
                    ));
                }
            };

            if tool_calls.is_empty() {
                // No tool calls — this is the final answer
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
                    continue;
                }

                return Ok(text);
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

            // Execute tools
            self.state = AgentState::ExecutingTools;
            let tool_results = {
                let mut orchestrator = executor::ToolOrchestrator {
                    registry: &self.registry,
                    permission_policy: &mut self.permission_policy,
                    hook_registry: &mut self.hook_registry,
                    tool_execution_mode: self.tool_execution_mode,
                    cancel_token: self.cancel_token.clone(),
                };
                orchestrator.execute_tools(&tool_calls, &on_event).await
            };
            self.state = AgentState::Streaming;

            // Add tool results to context and emit events
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
                return Ok(summary);
            }
        }

        bail!("unexpected end of agent loop")
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
        while let Some(event) = stream.next().await {
            if cancel.is_cancelled() {
                break;
            }

            let event = event?;
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
        self.transform_context = Some(Box::new(f));
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

    /// Approve a pending permission request.
    /// Call this after receiving `AgentEvent::ApprovalRequired`.
    pub fn approve(&self, prompt_id: &str, choice: ApprovalChoice) -> bool {
        if let Ok(mut pending) = crate::permission::global_pending_approvals().lock() {
            if let Some(tx) = pending.remove(prompt_id) {
                return tx.send(choice).is_ok();
            }
        }
        false
    }

    /// Deny a pending permission request (shorthand for approve with Deny).
    pub fn deny_approval(&self, prompt_id: &str) -> bool {
        self.approve(prompt_id, ApprovalChoice::Deny)
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
