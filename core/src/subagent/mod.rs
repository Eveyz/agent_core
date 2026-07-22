use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent_registry::memory::AgentMemoryStore;

use crate::client::OpenAIClient;
use crate::config::ModelConfig;
use crate::context::Context;
use crate::runtime::supervisor::ProcessSupervisor;
use crate::runtime::EventGuard;
use crate::tools::ToolRegistry;
use crate::types::{AgentEvent, EventSender, Message, MessageDelta, StreamEvent, ToolCall};
use transcript::{TranscriptOutcome, TranscriptRecorder};

pub mod handoff;
pub mod spec;
pub mod transcript;

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PersonaKey(String);

impl PersonaKey {
    pub fn parse(value: &str) -> Result<Self> {
        let valid = !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if !valid {
            anyhow::bail!("invalid persona key: expected 1-64 ASCII letters, digits, '-' or '_'");
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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

#[derive(Default, Clone)]
struct StreamPartial {
    text: String,
    thinking: String,
}

impl StreamPartial {
    fn merge_attempt(&mut self, attempt: &Self) {
        if attempt.text.len() > self.text.len() {
            self.text.clone_from(&attempt.text);
        }
        if attempt.thinking.len() > self.thinking.len() {
            self.thinking.clone_from(&attempt.thinking);
        }
    }

    fn recoverable_text(&self) -> String {
        crate::hygiene::wrap_thinking(&self.thinking, &self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMemoryIdentity {
    agent_id: String,
    memory_key: String,
}

impl AgentMemoryIdentity {
    pub fn new(agent_id: impl Into<String>, memory_key: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            memory_key: memory_key.into(),
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    pub fn memory_key(&self) -> &str {
        &self.memory_key
    }
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
    memory_identity: Option<AgentMemoryIdentity>,
    pub session_id: Option<String>,
    /// Prompt id for artifact redirects (inherited from parent Run when present).
    pub prompt_id: Option<String>,
    pub parent_run_id: Option<String>,
    /// Optional cancel token — propagated from the parent Run so canceling the
    /// parent also stops the subagent. When `None`, a new token is created.
    pub cancel_token: Option<CancellationToken>,
    /// Optional process supervisor — when set, the subagent's ShellTool is
    /// replaced with a supervised version for process-group isolation.
    pub supervisor: Option<Arc<Mutex<ProcessSupervisor>>>,
    transcript: Option<Mutex<TranscriptRecorder>>,
    approval_resolver: Option<crate::runtime::ApprovalResolver>,
}

impl Subagent {
    pub fn new(
        role_name: &str,
        mut config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
        permission_config: crate::permission::PermissionConfig,
    ) -> Self {
        if !config
            .system_prompt
            .starts_with(spec::SUBAGENT_PROMPT_SCHEMA)
            && !config
                .system_prompt
                .starts_with(&format!("[{}]", spec::SUBAGENT_PROMPT_SCHEMA))
        {
            config.system_prompt = spec::PromptLayers {
                base: config.system_prompt.clone(),
                output_contract: spec::output_contract(config.result_strategy).to_string(),
                ..Default::default()
            }
            .render();
        }
        let client = OpenAIClient::new(model_config.clone());
        let context = Context::new(&config.system_prompt, config.max_context_tokens);

        // Inherit the parent's permission posture (mode, sandbox paths,
        // blacklist, config rules, auto-allow level, persistent whitelist) so a
        // sandboxed/strict parent cannot spawn a less-strict subagent. The
        // subagent gets a fresh runtime whitelist for its own approvals.
        let permission_policy = crate::permission::PermissionPolicy::with_builtin_defaults()
            .with_config(&permission_config);

        let registry = Self::wire_working_dir(registry, &config);

        let id = uuid::Uuid::new_v4().to_string();
        let transcript = TranscriptRecorder::in_default_root(&id).map(Mutex::new);
        Self {
            id,
            role_name: role_name.to_string(),
            config,
            client,
            context,
            registry,
            permission_policy,
            hook_registry: Arc::new(parking_lot::Mutex::new(crate::hooks::HookRegistry::new())),
            memory_store: None,
            memory_identity: None,
            session_id: None,
            prompt_id: None,
            parent_run_id: None,
            cancel_token: None,
            supervisor: None,
            transcript,
            approval_resolver: None,
        }
    }

    /// Builder: attach a `ProcessSupervisor` (propagated from the parent Run)
    /// so the subagent's shell commands are managed within the same process group.
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

    pub fn with_approval_resolver(mut self, resolver: crate::runtime::ApprovalResolver) -> Self {
        self.approval_resolver = Some(resolver);
        self
    }

    pub fn with_runtime_scope(
        mut self,
        session_id: Option<String>,
        prompt_id: Option<String>,
        parent_run_id: Option<String>,
    ) -> Self {
        self.session_id = session_id.clone();
        self.prompt_id = prompt_id;
        self.parent_run_id = parent_run_id.clone();
        if let Some(recorder) = &self.transcript {
            recorder.lock().set_scope(session_id, parent_run_id);
        }
        self
    }

    pub fn approval_resolver(&self) -> Option<&crate::runtime::ApprovalResolver> {
        self.approval_resolver.as_ref()
    }

    /// Replace the ShellTool in the registry with a supervised version.
    fn wire_supervisor_to_registry(&mut self, supervisor: &Arc<Mutex<ProcessSupervisor>>) {
        if self.registry.has("shell") {
            let working_dir = self
                .config
                .working_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            self.registry
                .register(Box::new(crate::tools::shell::ShellTool::with_supervisor(
                    supervisor.clone(),
                    working_dir,
                )));
        }
    }

    /// Replace the ShellTool in `registry` with one whose
    /// `default_working_dir` is set to the subagent's `working_dir`, if any.
    /// This is how the subagent's shell commands execute in the intended
    /// directory WITHOUT touching the process-global CWD (which would race
    /// with concurrent subagents on the same tokio runtime).
    fn wire_working_dir(mut registry: ToolRegistry, config: &SubagentConfig) -> ToolRegistry {
        if let Some(ref wd) = config.working_dir {
            if registry.has("shell") {
                registry.register(Box::new(
                    crate::tools::shell::ShellTool::with_default_working_dir(Some(
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
        memory_identity: Option<AgentMemoryIdentity>,
    ) -> Self {
        let mut sa = Self::new(role_name, config, model_config, registry, permission_config);
        sa.memory_store = memory_store;
        sa.memory_identity = memory_identity;
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
        let result = self.run_inner(task, event_sender).await;
        let outcome = match &result {
            Ok(result) if result.success => TranscriptOutcome::Succeeded,
            Ok(_) => TranscriptOutcome::Failed,
            Err(error) if error.to_string().contains("cancel") => TranscriptOutcome::Cancelled,
            Err(_) => TranscriptOutcome::Failed,
        };
        if let Err(error) = self.finalize_transcript(outcome) {
            tracing::warn!(subagent_id = %self.id, error = %error,
                "Failed to finalize subagent transcript");
        }
        result
    }

    async fn run_inner(
        &mut self,
        task: &str,
        event_sender: Option<EventSender>,
    ) -> Result<SubagentResult> {
        // NOTE: We intentionally do NOT touch the process-global CWD here.
        // Modifying std::env::set_current_dir() races with concurrent
        // subagents sharing the same tokio runtime. The subagent's working
        // directory is instead plumbed into the ShellTool via
        // `default_working_dir` (set in Subagent::new). File-based tools
        // (edit/read_file/write_file/grep) take absolute paths anyway.

        // PLAN-0009: inject relevant memories before the task is added.
        self.inject_memory(task);

        self.context
            .add(Message::user_with_model(task, &self.client.model.model_id));
        self.checkpoint_transcript(None);

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

            let base_messages = self.context.messages();
            let tools = self.registry.tool_definitions();

            const MAX_STREAM_ATTEMPTS: u32 = 3;
            let mut stream_error: Option<anyhow::Error> = None;
            let mut text = String::new();
            let mut thinking = String::new();
            let mut reasoning_blob = crate::types::ReasoningState::default();
            let mut tool_calls = Vec::new();
            let mut message_id = String::new();
            let mut retry_checkpoint = StreamPartial::default();

            for attempt in 0..MAX_STREAM_ATTEMPTS {
                let mut attempt_messages = base_messages.clone();
                crate::hygiene::inject_stream_retry_hint(
                    &mut attempt_messages,
                    &retry_checkpoint.thinking,
                    &retry_checkpoint.text,
                );
                let stream = match self
                    .client
                    .chat_completion_stream(&attempt_messages, &tools)
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

                let mut attempt_partial = StreamPartial::default();
                match self
                    .collect_stream(stream, event_sender.as_ref(), &mut attempt_partial)
                    .await
                {
                    Ok((t, th, blob, tc, mid)) => {
                        text = t;
                        thinking = th;
                        reasoning_blob = blob;
                        tool_calls = tc;
                        message_id = mid;
                        stream_error = None;
                        break;
                    }
                    Err(e) => {
                        retry_checkpoint.merge_attempt(&attempt_partial);
                        self.checkpoint_transcript(Some(&retry_checkpoint.recoverable_text()));
                        if self.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                            return Err(e);
                        }
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

            // Some reasoning-heavy models (e.g. DeepSeek via NVIDIA) put the
            // entire final answer in `reasoning_content` with an empty `content`
            // field.  Promote thinking → text so the UI and parent agent see it.
            if tool_calls.is_empty() && text.trim().is_empty() && !thinking.trim().is_empty() {
                text = thinking.trim().to_string();
                thinking.clear();
                if let Some(ref tx) = event_sender {
                    let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                        subagent_id: self.id.clone(),
                        message_id: message_id.clone(),
                        delta: MessageDelta::Text(text.clone()),
                    });
                }
            }

            last_text = text.clone();
            if !all_text.is_empty() && !text.is_empty() {
                all_text.push_str("\n\n");
            }
            all_text.push_str(&text);
            tool_call_count += tool_calls.len();

            if tool_calls.is_empty() {
                self.record_final_response(&text, &thinking, reasoning_blob);
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
                let content = crate::hygiene::wrap_thinking(&thinking, &text);
                let mut msg = Message::assistant_with_tools(&content, tool_calls.clone());
                let mut reasoning = reasoning_blob;
                if !thinking.trim().is_empty() && reasoning.text.is_none() {
                    reasoning.text = Some(thinking.trim().to_string());
                }
                if !reasoning.is_empty() {
                    msg = msg.with_reasoning(reasoning);
                }
                self.context.add(msg);
                self.checkpoint_transcript(None);
            }

            // Execute tools, emitting SubagentToolStart/SubagentToolEnd events
            let results = {
                let cancel = self
                    .cancel_token
                    .clone()
                    .unwrap_or_else(|| tokio_util::sync::CancellationToken::new());
                let mut orchestrator = crate::runtime::tool_orchestrator::ToolOrchestrator {
                    registry: &self.registry,
                    permission_policy: &mut self.permission_policy,
                    hook_registry: self.hook_registry.clone(),
                    tool_execution_mode: crate::types::ToolExecutionMode::Sequential,
                    cancel_token: cancel,
                    approval_resolver: self.approval_resolver.clone(),
                    input_resolver: None, // ask_user only on main Run (v1)
                    session_id: self.session_id.clone(),
                    prompt_id: self.prompt_id.clone(),
                    run_id: self.parent_run_id.clone().or_else(|| Some(self.id.clone())),
                    working_dir: self
                        .config
                        .working_dir
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
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
                let is_error = crate::runtime::execution::tool_result_is_error(result);
                if let Some(ref tx) = event_sender {
                    let _ = tx.send(AgentEvent::SubagentToolEnd {
                        subagent_id: self.id.clone(),
                        tool_call_id: call.id.clone(),
                        tool_name: call.function.name.clone(),
                        result: result.clone(),
                        is_error,
                    });
                }
                self.context.add(Message::tool(
                    call.id.clone(),
                    result.clone(),
                    Some(call.function.name.clone()),
                ));
                self.checkpoint_transcript(None);
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
            format!(
                "{}... [truncated, total {} chars]",
                &all_text[..1000],
                all_text.len()
            )
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
        partial: &mut StreamPartial,
    ) -> Result<(
        String,
        String,
        crate::types::ReasoningState,
        Vec<ToolCall>,
        String,
    )> {
        use crate::client::streaming::{TokenAccumulator, ToolCallAccumulator};
        use futures::StreamExt;

        let mut text_buffer = String::new();
        let mut thinking_buffer = String::new();
        let mut reasoning_blob = crate::types::ReasoningState::default();
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
        loop {
            let flush_delay = tokens.pending_flush_delay();
            let next = tokio::select! {
                _ = async {
                    match &self.cancel_token {
                        Some(t) => t.cancelled().await,
                        None => std::future::pending().await,
                    }
                }, if self.cancel_token.is_some() => {
                    anyhow::bail!("subagent cancelled");
                }
                _ = tokio::time::sleep(flush_delay.unwrap_or(std::time::Duration::ZERO)),
                    if flush_delay.is_some() => {
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
                    continue;
                }
                item = stream.next() => item,
            };
            let Some(event) = next else { break };
            let event = event?;
            match event {
                StreamEvent::TextDelta(delta) => {
                    tokens.push_text(&delta);
                    text_buffer.push_str(&delta);
                    partial.text.push_str(&delta);
                    self.checkpoint_transcript(Some(&partial.recoverable_text()));
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
                    thinking_buffer.push_str(&delta);
                    partial.thinking.push_str(&delta);
                    self.checkpoint_transcript(Some(&partial.recoverable_text()));
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
                StreamEvent::ReasoningBlob {
                    encrypted_content,
                    signature,
                    summary,
                } => {
                    if let Some(blob) = encrypted_content {
                        if !blob.is_empty() {
                            reasoning_blob.encrypted_content = Some(blob);
                        }
                    }
                    if let Some(sig) = signature {
                        if !sig.is_empty() {
                            match &mut reasoning_blob.signature {
                                Some(existing) => existing.push_str(&sig),
                                None => reasoning_blob.signature = Some(sig),
                            }
                        }
                    }
                    if let Some(s) = summary {
                        if !s.is_empty() {
                            reasoning_blob.summary = Some(s);
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

        Ok((
            text_buffer,
            thinking_buffer,
            reasoning_blob,
            tool_calls,
            message_id,
        ))
    }

    fn record_final_response(
        &mut self,
        text: &str,
        thinking: &str,
        mut reasoning: crate::types::ReasoningState,
    ) {
        let content = crate::hygiene::wrap_thinking(thinking, text);
        let mut message = Message::assistant(&content);
        if !thinking.trim().is_empty() && reasoning.text.is_none() {
            reasoning.text = Some(thinking.trim().to_string());
        }
        if !reasoning.is_empty() {
            message = message.with_reasoning(reasoning);
        }
        self.context.add(message);
    }

    /// Inject relevant memories from the per-agent store into the context's
    /// active-memory segment (appended to any existing content).
    fn inject_memory(&mut self, task: &str) {
        let (Some(store), Some(identity)) = (&self.memory_store, &self.memory_identity) else {
            return;
        };
        let injection = store.build_context_injection(identity.memory_key(), task, 2000);
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
            self.context
                .set_active_memory(&format!("{existing}\n\n{injection}"));
        }
    }

    /// Persist the conversation turn (user task + assistant output) to the
    /// per-agent memory store. Errors are logged and swallowed.
    fn persist_memory(&self, task: &str, output: &str) {
        let (Some(store), Some(identity)) = (&self.memory_store, &self.memory_identity) else {
            return;
        };
        if let Err(e) = store.store(
            identity.memory_key(),
            identity.agent_id(),
            "user",
            task,
            "conversation",
        ) {
            tracing::warn!("failed to persist agent memory (user): {e}");
        }
        if !output.is_empty() {
            if let Err(e) = store.store(
                identity.memory_key(),
                identity.agent_id(),
                "assistant",
                output,
                "conversation",
            ) {
                tracing::warn!("failed to persist agent memory (assistant): {e}");
            }
        }
    }

    fn checkpoint_transcript(&self, partial_assistant: Option<&str>) {
        if let Some(recorder) = &self.transcript {
            recorder
                .lock()
                .checkpoint(self.context.raw_messages(), partial_assistant);
        }
    }

    fn finalize_transcript(&self, outcome: TranscriptOutcome) -> Result<std::path::PathBuf> {
        let recorder = self
            .transcript
            .as_ref()
            .context("subagent transcript recorder is unavailable")?;
        recorder
            .lock()
            .finalize(self.context.raw_messages(), outcome)
    }

    pub fn transcript_path(&self) -> Option<std::path::PathBuf> {
        self.transcript
            .as_ref()
            .and_then(|recorder| recorder.lock().persisted_path().map(ToOwned::to_owned))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Consume the subagent and return its conversation messages.
    /// Useful for saving the subagent's session.
    pub fn into_messages(self) -> Vec<Message> {
        self.context.raw_messages().to_vec()
    }

    /// Get a reference to the subagent's context messages (non-consuming).
    pub fn messages(&self) -> Vec<Message> {
        self.context.raw_messages().to_vec()
    }
}

pub struct SubagentManager {
    subagents: HashMap<String, SubagentConfig>,
}

#[cfg(test)]
mod identity_tests {
    use super::{PersonaKey, Subagent, SubagentConfig};
    use crate::config::ModelConfig;
    use crate::permission::PermissionConfig;
    use crate::tools::ToolRegistry;
    use crate::types::{ReasoningState, Role};

    #[test]
    fn persona_key_accepts_only_a_safe_single_path_component() {
        assert!(PersonaKey::parse("code-reviewer").is_ok());
        assert!(PersonaKey::parse("reviewer_2").is_ok());
        for unsafe_value in [
            "../secret",
            "nested/agent",
            "nested\\agent",
            ".",
            "",
            "agent name",
        ] {
            assert!(PersonaKey::parse(unsafe_value).is_err());
        }
    }

    #[test]
    fn final_response_is_part_of_the_canonical_transcript() {
        let mut subagent = Subagent::new(
            "reviewer",
            SubagentConfig::default(),
            &ModelConfig::default(),
            ToolRegistry::new(),
            PermissionConfig::default(),
        );
        subagent.record_final_response("final answer", "", ReasoningState::default());
        let final_message = subagent.messages().pop().expect("final assistant message");
        assert_eq!(final_message.role, Role::Assistant);
        assert_eq!(final_message.content.as_deref(), Some("final answer"));
    }

    #[test]
    fn child_approval_resolver_is_the_parent_run_resolver() {
        let parent = crate::runtime::ApprovalResolver::new();
        let subagent = Subagent::new(
            "reviewer",
            SubagentConfig::default(),
            &ModelConfig::default(),
            ToolRegistry::new(),
            PermissionConfig::default(),
        )
        .with_approval_resolver(parent.clone());

        let (tx, _rx) = tokio::sync::oneshot::channel();
        subagent
            .approval_resolver()
            .expect("delegated resolver")
            .insert("child-prompt".into(), tx);
        assert_eq!(parent.len(), 1);
    }
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
