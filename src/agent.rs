use anyhow::{Result, bail};
use futures::StreamExt;
use std::sync::{Arc, Mutex};

use crate::client::OpenAIClient;
use crate::config::Config;
use crate::context::Context;
use crate::hooks::{HookRegistry, PreToolResult};
use crate::memory::MemoryManager;
use crate::permission::{PermissionDecision, PermissionPolicy};
use crate::prompt::PromptBuilder;
use crate::tools::ToolRegistry;
use crate::types::{AgentEvent, Message, StreamEvent, ToolCall};

pub struct AgentBuilder {
    config: Config,
    tools: Vec<Box<dyn crate::tools::Tool>>,
    system_prompt: Option<String>,
    enable_memory: bool,
    permission_policy: Option<PermissionPolicy>,
    hook_registry: Option<HookRegistry>,
}

impl AgentBuilder {
    pub fn from_config(path: &str) -> Result<Self> {
        let config = Config::load(path)?;
        Ok(Self {
            config,
            tools: Vec::new(),
            system_prompt: None,
            enable_memory: false,
            permission_policy: None,
            hook_registry: None,
        })
    }

    pub fn from_env() -> Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            config,
            tools: Vec::new(),
            system_prompt: None,
            enable_memory: false,
            permission_policy: None,
            hook_registry: None,
        })
    }

    pub fn with_tool(mut self, tool: impl crate::tools::Tool + 'static) -> Self {
        self.tools.push(Box::new(tool));
        self
    }

    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = Some(prompt.to_string());
        self
    }

    pub fn with_memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    pub fn with_permission_policy(mut self, policy: PermissionPolicy) -> Self {
        self.permission_policy = Some(policy);
        self
    }

    pub fn with_hook_registry(mut self, registry: HookRegistry) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    pub fn build(self) -> Result<Agent> {
        let default_model_name = self.config.default_model.clone();
        let model_config = self.config.default_model().clone();

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
                let m = MemoryManager::new(
                    "~/.agent_core/memory.db",
                    "BAAI/bge-small-en-v1.5",
                    2000,
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

        for tool in self.tools {
            registry.register(tool);
        }

        let mut context = Context::new(&system_prompt, model_config.max_context_tokens);

        if let Some(ref mem) = memory {
            if let Ok(m) = mem.lock() {
                let core_memory_str = m.core().to_context_string();
                if !core_memory_str.is_empty() {
                    context.set_core_memory(&core_memory_str);
                }
            }
        }

        let client = OpenAIClient::new(model_config);

        Ok(Agent {
            config: self.config,
            current_model_name: default_model_name,
            client,
            registry,
            context,
            memory,
            permission_policy: self.permission_policy.unwrap_or_default(),
            hook_registry: self.hook_registry.unwrap_or_default(),
        })
    }
}

pub struct Agent {
    config: Config,
    current_model_name: String,
    client: OpenAIClient,
    registry: ToolRegistry,
    context: Context,
    memory: Option<Arc<Mutex<MemoryManager>>>,
    permission_policy: PermissionPolicy,
    hook_registry: HookRegistry,
}

impl Agent {
    pub async fn run(&mut self, input: &str) -> Result<String> {
        self.run_with_events(input, |_| {}).await
    }

    pub async fn run_with_events(
        &mut self,
        input: &str,
        on_event: impl Fn(AgentEvent),
    ) -> Result<String> {
        self.context.add(Message::user(input));

        if let Some(ref mem) = self.memory {
            if let Ok(m) = mem.lock() {
                let _ = m.store_conversation("user", input);
            }
        }

        let max_iterations = self.client.model.max_iterations;

        for iteration in 0..max_iterations {
            self.context.trim_to_fit();

            let messages = self.context.messages();
            let tools = self.registry.tool_definitions();

            let stream = match self.client.chat_completion_stream(&messages, &tools).await {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = format!("LLM request failed: {e}");
                    on_event(AgentEvent::Error(err_msg.clone()));
                    // Return error as final answer so the loop doesn't break silently
                    return Ok(format!("I encountered an error communicating with the model: {e}. Please try again."));
                }
            };

            let (text, tool_calls) = match self.collect_stream(stream, &on_event).await {
                Ok(r) => r,
                Err(e) => {
                    let err_msg = format!("Stream error: {e}");
                    on_event(AgentEvent::Error(err_msg));
                    return Ok(format!("I encountered an error reading the model response: {e}. Please try again."));
                }
            };

            if tool_calls.is_empty() {
                self.context.add(Message::assistant(&text));
                on_event(AgentEvent::FinalAnswer(text.clone()));

                if let Some(ref mem) = self.memory {
                    if let Ok(m) = mem.lock() {
                        let _ = m.store_conversation("assistant", &text);
                    }
                    self.refresh_core_memory_in_context();
                    self.maybe_consolidate();
                }

                return Ok(text);
            }

            if !text.is_empty() {
                self.context
                    .add(Message::assistant_with_tools(&text, tool_calls.clone()));
            }

            for call in &tool_calls {
                on_event(AgentEvent::ToolStart(call.function.name.clone()));
            }

            let results = self.execute_tools_with_hooks(&tool_calls, &on_event).await;

            for (call, result) in tool_calls.iter().zip(&results) {
                on_event(AgentEvent::ToolResult(result.clone()));
                self.context
                    .add(Message::tool(call.id.clone(), result.clone()));
            }

            if iteration == max_iterations - 1 {
                let msg = format!(
                    "Reached iteration limit ({max_iterations}). Stopping to prevent infinite loop. Last response:\n{text}"
                );
                on_event(AgentEvent::Error(msg.clone()));
                return Ok(msg);
            }
        }

        bail!("unexpected end of agent loop")
    }

    async fn execute_tools_with_hooks(
        &self,
        calls: &[ToolCall],
        on_event: &impl Fn(AgentEvent),
    ) -> Vec<String> {
        let mut results = Vec::new();

        for call in calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_default();

            // Permission check
            match self
                .permission_policy
                .check(&call.function.name, &call.function.arguments)
            {
                PermissionDecision::Deny(reason) => {
                    results.push(format!("Permission denied: {}", reason));
                    continue;
                }
                PermissionDecision::Ask(reason) => {
                    on_event(AgentEvent::ToolStart(format!(
                        "[APPROVAL NEEDED] {}: {}",
                        call.function.name, reason
                    )));
                }
                PermissionDecision::Allow => {}
            }

            // Pre-tool hook
            match self.hook_registry.fire_pre_tool_use(&call.function.name, &args) {
                PreToolResult::Veto(reason) => {
                    results.push(format!("Hook vetoed: {}", reason));
                    continue;
                }
                PreToolResult::Proceed(modified_args) => {
                    let tool = self.registry.get(&call.function.name);
                    match tool {
                        Some(t) => {
                            match t.execute(modified_args.clone()).await {
                                Ok(output) => {
                                    let final_output = self.hook_registry.fire_post_tool_use(
                                        &call.function.name,
                                        &modified_args,
                                        &output,
                                    );
                                    results.push(final_output);
                                }
                                Err(e) => {
                                    results.push(format!(
                                        "Error executing tool '{}': {}",
                                        call.function.name, e
                                    ));
                                }
                            }
                        }
                        None => {
                            results.push(format!(
                                "Tool '{}' not found. Available: {}",
                                call.function.name,
                                self.registry.list_names().join(", ")
                            ));
                        }
                    }
                }
            }
        }

        results
    }

    async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        on_event: &impl Fn(AgentEvent),
    ) -> Result<(String, Vec<ToolCall>)> {
        use crate::client::streaming::ToolCallAccumulator;

        let mut text_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event?;
            match event {
                StreamEvent::ThinkingDelta(delta) => {
                    if !delta.is_empty() {
                        on_event(AgentEvent::Thinking(delta));
                    }
                }
                StreamEvent::TextDelta(delta) => {
                    if !delta.is_empty() {
                        text_buffer.push_str(&delta);
                        on_event(AgentEvent::Thought(delta));
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
        if let Some(ref mem) = self.memory {
            if let Ok(m) = mem.lock() {
                let core_str = m.core().to_context_string();
                self.context.set_core_memory(&core_str);
            }
        }
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

    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        let model = self
            .config
            .get_model(name)
            .ok_or_else(|| anyhow::anyhow!("model '{}' not found", name))?
            .clone();

        self.current_model_name = name.to_string();
        self.context.set_max_tokens(model.max_context_tokens);
        self.client = OpenAIClient::new(model);

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

    pub fn set_temperature(&mut self, temp: f64) {
        self.client.set_temperature(temp);
    }

    pub fn set_max_tokens(&mut self, max: u32) {
        self.client.set_max_tokens(max);
    }

    pub fn clear_context(&mut self) {
        self.context.clear();
        if let Some(ref mem) = self.memory {
            if let Ok(mut m) = mem.lock() {
                m.new_session();
            }
        }
    }

    pub fn context_token_count(&self) -> usize {
        self.context.current_token_count()
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
}
