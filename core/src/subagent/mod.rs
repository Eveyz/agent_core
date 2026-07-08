use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent_registry::memory::AgentMemoryStore;

use crate::client::OpenAIClient;
use crate::config::ModelConfig;
use crate::context::Context;
use crate::runtime::EventGuard;
use crate::runtime::supervisor::ProcessSupervisor;
use crate::tools::ToolRegistry;
use crate::types::{AgentEvent, EventSender, Message, MessageDelta, StreamEvent, ToolCall};

/// How the subagent's result should be formatted before returning to the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStrategy {
    /// Preserve current behaviour: all_text + tool_summary (backward-compat).
    Auto,
    /// Return complete last-turn text without truncation. Best for code / data.
    Full,
    /// Inject summarisation instruction; return only the last-turn text.
    Summary,
}

impl Default for ResultStrategy {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,

    // ── PLAN-0009 extensions (all optional, backward compatible) ──
    /// "provider/model" override; None = use the model_config passed to new().
    #[serde(default)]
    pub model: Option<String>,
    /// Skill names to inject into this subagent.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Permission mode override (paranoid/standard/developer/permissive/yolo).
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// None = per-agent memory disabled; Some(0/1/2) = stateless/standard/deep.
    #[serde(default)]
    pub memory_enabled: Option<u8>,
    /// Per-agent temperature override.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Result formatting strategy for the parent agent.
    #[serde(default)]
    pub result_strategy: ResultStrategy,
    /// Working directory for this subagent. When set, tools use this instead
    /// of the process CWD for relative path resolution. This avoids the race
    /// condition when multiple subagents run concurrently on the same thread pool.
    #[serde(default)]
    pub working_dir: Option<std::path::PathBuf>,
    /// Recursion depth from the top-level agent (0 = top-level Run's agent
    /// itself, 1 = its direct subagent, 2 = grand-subagent, …).
    /// Used by `spawn_single` to refuse spawning past the configured limit
    /// and to safely filter out meta-dispatch tools (`subagent`/`subagents`/
    /// `skill_list`/`skill_load`/`skill_deactivate`/`skill_reload`) so a
    /// subagent cannot recursively spawn new subagents beyond the limit.
    #[serde(default)]
    pub recursion_depth: u8,
}

/// Hard cap on subagent recursion depth.
pub const MAX_SUBAGENT_DEPTH: u8 = 3;

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are a focused sub-agent. Complete the given task and return the result. Be concise.".to_string(),
            tools: Vec::new(),
            max_iterations: 50,
            // Default matches ModelConfig default (128K).  When created
            // through spawn_single() it is overridden to the parent model's
            // actual value so modern 1M+ models get the full window.
            max_context_tokens: 128000,
            model: None,
            skills: Vec::new(),
            permission_mode: None,
            memory_enabled: None,
            temperature: None,
            result_strategy: ResultStrategy::Auto,
            working_dir: None,
            recursion_depth: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub subagent_id: String,
    pub role_name: String,
    /// Full accumulated text from all iterations.
    pub output: String,
    /// Text from only the final assistant turn (no intermediate reasoning).
    pub last_text: String,
    pub iterations_used: usize,
    pub success: bool,
}

pub struct Subagent {
    pub id: String,
    pub role_name: String,
    config: SubagentConfig,
    client: OpenAIClient,
    context: Context,
    registry: ToolRegistry,
    permission_policy: crate::permission::PermissionPolicy,
    hook_registry: Arc<parking_lot::Mutex<crate::hooks::HookRegistry>>,
    /// Per-agent memory store (PLAN-0009). When set, memories are injected
    /// before execution and persisted after.
    memory_store: Option<Arc<AgentMemoryStore>>,
    /// The agent definition id this subagent was built from (for memory keying).
    agent_id: Option<String>,
    pub session_id: Option<String>,
    /// Optional cancel token — propagated from the parent Run so canceling the
    /// parent also stops the subagent. When `None`, a new token is created.
    pub cancel_token: Option<CancellationToken>,
    /// Optional process supervisor — when set, the subagent's BashTool is
    /// replaced with a supervised version for process-group isolation.
    pub supervisor: Option<Arc<Mutex<ProcessSupervisor>>>,
}

impl Subagent {
    pub fn new(
        role_name: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
        permission_config: crate::permission::PermissionConfig,
    ) -> Self {
        let client = OpenAIClient::new(model_config.clone());
        let context = Context::new(&config.system_prompt, config.max_context_tokens);

        // Inherit the parent's permission posture (mode, sandbox paths,
        // blacklist, config rules, auto-allow level, persistent whitelist) so a
        // sandboxed/strict parent cannot spawn a less-strict subagent. The
        // subagent gets a fresh runtime whitelist for its own approvals.
        let permission_policy = crate::permission::PermissionPolicy::with_builtin_defaults()
            .with_config(&permission_config);

        let registry = Self::wire_working_dir(registry, &config);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role_name: role_name.to_string(),
            config,
            client,
            context,
            registry,
            permission_policy,
            hook_registry: Arc::new(parking_lot::Mutex::new(crate::hooks::HookRegistry::new())),
            memory_store: None,
            agent_id: None,
            session_id: None,
            cancel_token: None,
            supervisor: None,
        }
    }

    /// Builder: attach a `ProcessSupervisor` (propagated from the parent Run)
    /// so the subagent's bash commands are managed within the same process group.
    pub fn with_supervisor(mut self, sv: Arc<Mutex<ProcessSupervisor>>) -> Self {
        self.wire_supervisor_to_registry(&sv);
        self.supervisor = Some(sv);
        self
    }

    /// Builder: attach a `CancellationToken` from the parent Run.
    pub fn with_cancel_token(mut self, ct: CancellationToken) -> Self {
        self.cancel_token = Some(ct);
        self
    }

    /// Replace the BashTool in the registry with a supervised version.
    fn wire_supervisor_to_registry(&mut self, supervisor: &Arc<Mutex<ProcessSupervisor>>) {
        if self.registry.has("bash") {
            let working_dir = self
                .config
                .working_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            self.registry.register(Box::new(
                crate::tools::bash::BashTool::with_supervisor(
                    supervisor.clone(),
                    working_dir,
                ),
            ));
        }
    }

    /// Replace the BashTool in `registry` with one whose
    /// `default_working_dir` is set to the subagent's `working_dir`, if any.
    /// This is how the subagent's bash commands execute in the intended
    /// directory WITHOUT touching the process-global CWD (which would race
    /// with concurrent subagents on the same tokio runtime).
    fn wire_working_dir(mut registry: ToolRegistry, config: &SubagentConfig) -> ToolRegistry {
        if let Some(ref wd) = config.working_dir {
            if registry.has("bash") {
                registry.register(Box::new(
                    crate::tools::bash::BashTool::with_default_working_dir(Some(
                        wd.to_string_lossy().to_string(),
                    )),
                ));
            }
        }
        registry
    }

    /// Construct a subagent with an optional per-agent memory store.
    ///
    /// `agent_id` is used as the memory isolation key. When both
    /// `memory_store` and `agent_id` are `Some`, the subagent injects relevant
    /// memories before execution and persists the conversation afterward.
    pub fn new_with_memory(
        role_name: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
        permission_config: crate::permission::PermissionConfig,
        memory_store: Option<Arc<AgentMemoryStore>>,
        agent_id: Option<String>,
    ) -> Self {
        let mut sa = Self::new(role_name, config, model_config, registry, permission_config);
        sa.memory_store = memory_store;
        sa.agent_id = agent_id;
        sa
    }

    pub async fn run(&mut self, task: &str) -> Result<SubagentResult> {
        self.run_with_sender(task, None).await
    }

    pub async fn run_with_sender(
        &mut self,
        task: &str,
        event_sender: Option<EventSender>,
    ) -> Result<SubagentResult> {
        // NOTE: We intentionally do NOT touch the process-global CWD here.
        // Modifying std::env::set_current_dir() races with concurrent
        // subagents sharing the same tokio runtime. The subagent's working
        // directory is instead plumbed into the BashTool via
        // `default_working_dir` (set in Subagent::new). File-based tools
        // (edit/read_file/write_file/grep) take absolute paths anyway.

        // PLAN-0009: inject relevant memories before the task is added.
        self.inject_memory(task);

        // Inject strategy-specific instructions as a system message before the task.
        // This ensures the subagent understands how its output will be consumed.
        match self.config.result_strategy {
            ResultStrategy::Summary => {
                self.context.add(Message::system(
                    "CRITICAL: In your final response (when you have no more tool calls to make), \
                    provide ONLY a concise summary of your key findings and conclusions. \
                    Do NOT repeat raw data, tool outputs, or intermediate reasoning. \
                    Filter out noise, ads, boilerplate, and irrelevant content. \
                    Only return actionable key findings."
                ));
            }
            ResultStrategy::Full => {
                self.context.add(Message::system(
                    "CRITICAL: Output ALL findings and data verbatim in your final response. \
                    Do NOT summarize, paraphrase, or omit anything. \
                    Your complete response will be forwarded directly to the main agent \
                    as the authoritative result. Include every detail from the tools you executed."
                ));
            }
            ResultStrategy::Auto => {
                // Default behaviour — no extra instruction.
            }
        }

        self.context.add(Message::user_with_model(task, &self.client.model.model_id));

        // Emit SubagentStart
        if let Some(ref tx) = event_sender {
            let _ = tx.send(AgentEvent::SubagentStart {
                subagent_id: self.id.clone(),
                role_name: self.role_name.clone(),
                task: task.to_string(),
            });
        }

        // RAII guard: if run_with_sender returns Err(?) early (e.g. stream
        // error) or panics, Drop emits SubagentEnd{success:false} so the
        // frontend never sees an orphaned spinner. On success paths we call
        // guard.complete() before the explicit SubagentEnd emit.
        let sa_id = self.id.clone();
        let sa_role = self.role_name.clone();
        let guard_tx = event_sender.clone();
        let mut guard = EventGuard::new(move || {
            if let Some(ref tx) = guard_tx {
                let _ = tx.send(AgentEvent::SubagentEnd {
                    subagent_id: sa_id.clone(),
                    role_name: sa_role.clone(),
                    success: false,
                    iterations_used: 0,
                });
            }
        });

        let mut all_text = String::new();
        let mut last_text = String::new();
        let mut tool_call_count = 0;

        for iteration in 0..self.config.max_iterations {
            if self.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                anyhow::bail!("subagent cancelled");
            }
            // Emit SubagentTurnStart
            if let Some(ref tx) = event_sender {
                let _ = tx.send(AgentEvent::SubagentTurnStart {
                    subagent_id: self.id.clone(),
                    turn_index: iteration,
                });
            }

            self.context.trim_to_fit();

            let messages = self.context.messages();
            let tools = self.registry.tool_definitions();

            const MAX_STREAM_ATTEMPTS: u32 = 3;
            let mut stream_error: Option<anyhow::Error> = None;
            let mut text = String::new();
            let mut tool_calls = Vec::new();

            for attempt in 0..MAX_STREAM_ATTEMPTS {
                let stream = match self
                    .client
                    .chat_completion_stream(&messages, &tools)
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        if attempt + 1 < MAX_STREAM_ATTEMPTS {
                            let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt));
                            tracing::warn!(attempt, delay_ms = delay.as_millis(), error = %e,
                                "subagent: stream request failed, retrying");
                            if let Some(cancel) = &self.cancel_token {
                                tokio::select! {
                                    _ = tokio::time::sleep(delay) => {}
                                    _ = cancel.cancelled() => anyhow::bail!("subagent cancelled"),
                                }
                            } else {
                                tokio::time::sleep(delay).await;
                            }
                            continue;
                        }
                        stream_error = Some(e);
                        break;
                    }
                };

                match self.collect_stream(stream, event_sender.as_ref()).await {
                    Ok((t, tc)) => {
                        text = t;
                        tool_calls = tc;
                        stream_error = None;
                        break;
                    }
                    Err(e) => {
                        if attempt + 1 < MAX_STREAM_ATTEMPTS {
                            let delay = std::time::Duration::from_millis(1000 * 2u64.pow(attempt));
                            tracing::warn!(attempt, delay_ms = delay.as_millis(), error = %e,
                                "subagent: SSE stream dropped mid-response, retrying");
                            if let Some(cancel) = &self.cancel_token {
                                tokio::select! {
                                    _ = tokio::time::sleep(delay) => {}
                                    _ = cancel.cancelled() => anyhow::bail!("subagent cancelled"),
                                }
                            } else {
                                tokio::time::sleep(delay).await;
                            }
                            continue;
                        }
                        stream_error = Some(e);
                    }
                }
            }

            if let Some(e) = stream_error {
                return Err(e);
            }

            last_text = text.clone();
            if !all_text.is_empty() && !text.is_empty() {
                all_text.push_str("\n\n");
            }
            all_text.push_str(&text);
            tool_call_count += tool_calls.len();

            if tool_calls.is_empty() {
                guard.complete();
                // Emit SubagentEnd
                if let Some(ref tx) = event_sender {
                    let _ = tx.send(AgentEvent::SubagentEnd {
                        subagent_id: self.id.clone(),
                        role_name: self.role_name.clone(),
                        success: true,
                        iterations_used: iteration + 1,
                    });
                }

                self.persist_memory(task, &all_text);
                return Ok(SubagentResult {
                    subagent_id: self.id.clone(),
                    role_name: self.role_name.clone(),
                    output: all_text,
                    last_text: last_text.clone(),
                    iterations_used: iteration + 1,
                    success: true,
                });
            }

            if !text.is_empty() || !tool_calls.is_empty() {
                self.context
                    .add(Message::assistant_with_tools(&text, tool_calls.clone()));
            }

            // Execute tools, emitting SubagentToolStart/SubagentToolEnd events
            let results = {
                let cancel = self.cancel_token.clone()
                    .unwrap_or_else(|| tokio_util::sync::CancellationToken::new());
                let mut orchestrator = crate::runtime::tool_orchestrator::ToolOrchestrator {
                    registry: &self.registry,
                    permission_policy: &mut self.permission_policy,
                    hook_registry: self.hook_registry.clone(),
                    tool_execution_mode: crate::types::ToolExecutionMode::Sequential,
                    cancel_token: cancel,
                    approval_resolver: None,
                    session_id: self.session_id.clone(),
                    working_dir: self.config.working_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
                };

                let sender_clone = event_sender.clone();
                let subagent_id = self.id.clone();
                orchestrator
                    .execute_tools(&tool_calls, &move |e, _call_id: &str| {
                        if let Some(ref tx) = sender_clone {
                            let mapped = match e {
                                AgentEvent::ToolExecutionStart {
                                    tool_call_id,
                                    tool_name,
                                    args,
                                } => AgentEvent::SubagentToolStart {
                                    subagent_id: subagent_id.clone(),
                                    tool_call_id,
                                    tool_name,
                                    args,
                                },
                                AgentEvent::ToolExecutionEnd {
                                    tool_call_id,
                                    tool_name,
                                    result,
                                    is_error,
                                } => AgentEvent::SubagentToolEnd {
                                    subagent_id: subagent_id.clone(),
                                    tool_call_id,
                                    tool_name,
                                    result,
                                    is_error,
                                },
                                AgentEvent::ApprovalRequired {
                                    prompt_id,
                                    tool_name,
                                    tool_input,
                                    danger_level,
                                    explanation,
                                } => AgentEvent::SubagentApprovalRequired {
                                    subagent_id: subagent_id.clone(),
                                    prompt_id,
                                    tool_name,
                                    tool_input,
                                    danger_level,
                                    explanation,
                                },
                                _ => e,
                            };
                            let _ = tx.send(mapped);
                        }
                    })
                    .await
            };

            // The orchestrator emits SubagentToolStart during execution, but —
            // like the top-level agent — the caller is responsible for emitting
            // the matching SubagentToolEnd once results are in. Without this,
            // the UI tool block never flips out of its "active" (spinning)
            // state. (executor::execute_tools does not emit ToolExecutionEnd
            // itself; it is emitted by the caller, mirroring agent/mod.rs.)
            for (call, result) in tool_calls.iter().zip(&results) {
                let is_error = result.starts_with("Error")
                    || result.starts_with("Permission denied")
                    || result.starts_with("Hook vetoed");
                if let Some(ref tx) = event_sender {
                    let _ = tx.send(AgentEvent::SubagentToolEnd {
                        subagent_id: self.id.clone(),
                        tool_call_id: call.id.clone(),
                        tool_name: call.function.name.clone(),
                        result: result.clone(),
                        is_error,
                    });
                }
                self.context
                    .add(Message::tool(call.id.clone(), result.clone(), Some(call.function.name.clone())));
            }
        }

        guard.complete();
        // Emit SubagentEnd (max iterations reached)
        if let Some(ref tx) = event_sender {
            let _ = tx.send(AgentEvent::SubagentEnd {
                subagent_id: self.id.clone(),
                role_name: self.role_name.clone(),
                success: false,
                iterations_used: self.config.max_iterations,
            });
        }

        let truncated_output = if all_text.len() > 1000 {
            format!("{}... [truncated, total {} chars]", &all_text[..1000], all_text.len())
        } else if all_text.is_empty() {
            "(no output produced)".to_string()
        } else {
            all_text.clone()
        };

        self.persist_memory(task, &all_text);
        Ok(SubagentResult {
            subagent_id: self.id.clone(),
            role_name: self.role_name.clone(),
            output: format!(
                "=== Subagent Suspended (max iterations reached) ===\n\
                 Task: {}\n\
                 Iterations used: {} / {}\n\
                 Tool calls executed: {}\n\
                 Last output from subagent:\n\
                 {}\n\
                 ---\n\
                 The subagent was suspended at its iteration limit. \
                 To continue, spawn a new subagent referencing this progress.",
                task,
                self.config.max_iterations,
                self.config.max_iterations,
                tool_call_count,
                truncated_output,
            ),
            last_text: last_text.clone(),
            iterations_used: self.config.max_iterations,
            success: false,
        })
    }

    async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_sender: Option<&EventSender>,
    ) -> Result<(String, Vec<ToolCall>)> {
        use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
        use futures::StreamExt;

        let mut text_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;
        let mut tokens = TokenAccumulator::new();
        let message_id = uuid::Uuid::new_v4().to_string();

        let flush_tokens =
            |tokens: &mut TokenAccumulator, tx: Option<&EventSender>, id: &str, msg_id: &str| {
                if let Some((text, thinking)) = tokens.force_flush() {
                    if let Some(tx) = tx {
                        if !text.is_empty() {
                            let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                subagent_id: id.to_string(),
                                message_id: msg_id.to_string(),
                                delta: MessageDelta::Text(text),
                            });
                        }
                        if !thinking.is_empty() {
                            let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                subagent_id: id.to_string(),
                                message_id: msg_id.to_string(),
                                delta: MessageDelta::Thinking(thinking),
                            });
                        }
                    }
                }
            };

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            if self.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                anyhow::bail!("subagent cancelled");
            }
            let event = event?;
            match event {
                StreamEvent::TextDelta(delta) => {
                    tokens.push_text(&delta);
                    text_buffer.push_str(&delta);
                    if tokens.should_flush() {
                        if let Some((text, thinking)) = tokens.flush() {
                            if let Some(tx) = event_sender {
                                if !text.is_empty() {
                                    let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                        subagent_id: self.id.clone(),
                                        message_id: message_id.clone(),
                                        delta: MessageDelta::Text(text),
                                    });
                                }
                                if !thinking.is_empty() {
                                    let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                        subagent_id: self.id.clone(),
                                        message_id: message_id.clone(),
                                        delta: MessageDelta::Thinking(thinking),
                                    });
                                }
                            }
                        }
                    }
                }
                StreamEvent::ThinkingDelta(delta) => {
                    tokens.push_thinking(&delta);
                    if tokens.should_flush() {
                        if let Some((text, thinking)) = tokens.flush() {
                            if let Some(tx) = event_sender {
                                if !text.is_empty() {
                                    let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                        subagent_id: self.id.clone(),
                                        message_id: message_id.clone(),
                                        delta: MessageDelta::Text(text),
                                    });
                                }
                                if !thinking.is_empty() {
                                    let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                                        subagent_id: self.id.clone(),
                                        message_id: message_id.clone(),
                                        delta: MessageDelta::Thinking(thinking),
                                    });
                                }
                            }
                        }
                    }
                }
                StreamEvent::ToolCallDelta { .. } => {
                    has_tool_calls = true;
                    accumulator.push(event);
                }
                StreamEvent::Done => break,
                StreamEvent::CompleteWithUsage { .. } => break,
            }
        }

        // Final flush: emit any remaining buffered text/thinking.
        flush_tokens(&mut tokens, event_sender, &self.id, &message_id);

        let tool_calls = if has_tool_calls {
            accumulator.into_tool_calls()
        } else {
            vec![]
        };

        Ok((text_buffer, tool_calls))
    }

    /// Inject relevant memories from the per-agent store into the context's
    /// active-memory segment (appended to any existing content).
    fn inject_memory(&mut self, task: &str) {
        let (Some(store), Some(agent_id)) = (&self.memory_store, &self.agent_id) else {
            return;
        };
        let injection = store.build_context_injection(agent_id, task, 2000);
        if injection.is_empty() {
            return;
        }
        let existing = self
            .context
            .segment("active_memory")
            .map(|s| s.content.as_str())
            .unwrap_or("")
            .to_string();
        if existing.is_empty() {
            self.context.set_active_memory(&injection);
        } else {
            self.context.set_active_memory(&format!("{existing}\n\n{injection}"));
        }
    }

    /// Persist the conversation turn (user task + assistant output) to the
    /// per-agent memory store. Errors are logged and swallowed.
    fn persist_memory(&self, task: &str, output: &str) {
        let (Some(store), Some(agent_id)) = (&self.memory_store, &self.agent_id) else {
            return;
        };
        if let Err(e) = store.store(agent_id, agent_id, "user", task, "conversation") {
            tracing::warn!("failed to persist agent memory (user): {e}");
        }
        if !output.is_empty() {
            if let Err(e) = store.store(agent_id, agent_id, "assistant", output, "conversation") {
                tracing::warn!("failed to persist agent memory (assistant): {e}");
            }
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Consume the subagent and return its conversation messages.
    /// Useful for saving the subagent's session.
    pub fn into_messages(self) -> Vec<Message> {
        self.context.messages()
    }

    /// Get a reference to the subagent's context messages (non-consuming).
    pub fn messages(&self) -> Vec<Message> {
        self.context.messages()
    }
}

pub struct SubagentManager {
    subagents: HashMap<String, SubagentConfig>,
}

impl Default for SubagentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            subagents: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: &str, config: SubagentConfig) {
        self.subagents.insert(name.to_string(), config);
    }

    pub fn get_config(&self, name: &str) -> Option<&SubagentConfig> {
        self.subagents.get(name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.subagents.keys().map(|s: &String| s.as_str()).collect()
    }
}
