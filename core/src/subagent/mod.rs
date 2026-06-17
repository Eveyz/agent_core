use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::client::OpenAIClient;
use crate::config::ModelConfig;
use crate::context::Context;
use crate::tools::ToolRegistry;
use crate::types::{AgentEvent, EventSender, Message, MessageDelta, StreamEvent, ToolCall};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub max_iterations: usize,
    pub max_context_tokens: usize,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are a focused sub-agent. Complete the given task and return the result. Be concise.".to_string(),
            tools: Vec::new(),
            max_iterations: 5,
            max_context_tokens: 32000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub subagent_id: String,
    pub role_name: String,
    pub output: String,
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
    hook_registry: crate::hooks::HookRegistry,
}

impl Subagent {
    pub fn new(
        role_name: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
    ) -> Self {
        let client = OpenAIClient::new(model_config.clone());
        let context = Context::new(&config.system_prompt, config.max_context_tokens);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role_name: role_name.to_string(),
            config,
            client,
            context,
            registry,
            permission_policy: crate::permission::PermissionPolicy::with_builtin_defaults(),
            hook_registry: crate::hooks::HookRegistry::new(),
        }
    }

    pub async fn run(&mut self, task: &str) -> Result<SubagentResult> {
        self.run_with_sender(task, None).await
    }

    pub async fn run_with_sender(
        &mut self,
        task: &str,
        event_sender: Option<EventSender>,
    ) -> Result<SubagentResult> {
        self.context.add(Message::user(task));

        // Emit SubagentStart
        if let Some(ref tx) = event_sender {
            let _ = tx.send(AgentEvent::SubagentStart {
                subagent_id: self.id.clone(),
                role_name: self.role_name.clone(),
                task: task.to_string(),
            });
        }

        for iteration in 0..self.config.max_iterations {
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

            let stream = self
                .client
                .chat_completion_stream(&messages, &tools)
                .await?;

            let (text, tool_calls) = self.collect_stream(stream, event_sender.as_ref()).await?;

            if tool_calls.is_empty() {
                // Emit SubagentEnd
                if let Some(ref tx) = event_sender {
                    let _ = tx.send(AgentEvent::SubagentEnd {
                        subagent_id: self.id.clone(),
                        role_name: self.role_name.clone(),
                        success: true,
                        iterations_used: iteration + 1,
                    });
                }

                return Ok(SubagentResult {
                    subagent_id: self.id.clone(),
                    role_name: self.role_name.clone(),
                    output: text,
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
                let mut orchestrator = crate::agent::executor::ToolOrchestrator {
                    registry: &self.registry,
                    permission_policy: &mut self.permission_policy,
                    hook_registry: &mut self.hook_registry,
                    tool_execution_mode: crate::types::ToolExecutionMode::Sequential,
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                };
                
                let sender_clone = event_sender.clone();
                let subagent_id = self.id.clone();
                orchestrator.execute_tools(&tool_calls, &move |e| {
                    if let Some(ref tx) = sender_clone {
                        let mapped = match e {
                            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                                AgentEvent::SubagentToolStart {
                                    subagent_id: subagent_id.clone(),
                                    tool_call_id,
                                    tool_name,
                                    args,
                                }
                            }
                            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                                AgentEvent::SubagentToolEnd {
                                    subagent_id: subagent_id.clone(),
                                    tool_call_id,
                                    tool_name,
                                    result,
                                    is_error,
                                }
                            }
                            AgentEvent::ApprovalRequired { prompt_id, tool_name, tool_input, danger_level, explanation } => {
                                AgentEvent::SubagentApprovalRequired {
                                    subagent_id: subagent_id.clone(),
                                    prompt_id,
                                    tool_name,
                                    tool_input,
                                    danger_level,
                                    explanation,
                                }
                            }
                            _ => e,
                        };
                        let _ = tx.send(mapped);
                    }
                }).await
            };

            for (call, result) in tool_calls.iter().zip(&results) {
                self.context
                    .add(Message::tool(call.id.clone(), result.clone()));
            }
        }

        // Emit SubagentEnd (max iterations reached)
        if let Some(ref tx) = event_sender {
            let _ = tx.send(AgentEvent::SubagentEnd {
                subagent_id: self.id.clone(),
                role_name: self.role_name.clone(),
                success: false,
                iterations_used: self.config.max_iterations,
            });
        }

        Ok(SubagentResult {
            subagent_id: self.id.clone(),
            role_name: self.role_name.clone(),
            output: format!(
                "Subagent '{}' reached max iterations ({})",
                self.id, self.config.max_iterations
            ),
            iterations_used: self.config.max_iterations,
            success: false,
        })
    }

    async fn collect_stream(
        &self,
        stream: impl futures::Stream<Item = Result<StreamEvent>>,
        event_sender: Option<&EventSender>,
    ) -> Result<(String, Vec<ToolCall>)> {
        use crate::client::streaming::ToolCallAccumulator;
        use futures::StreamExt;

        let mut text_buffer = String::new();
        let mut accumulator = ToolCallAccumulator::new();
        let mut has_tool_calls = false;

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event?;
            match event {
                StreamEvent::TextDelta(delta) => {
                    // Emit SubagentMessageUpdate
                    if let Some(tx) = event_sender {
                        let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                            subagent_id: self.id.clone(),
                            delta: MessageDelta::Text(delta.clone()),
                        });
                    }
                    text_buffer.push_str(&delta);
                }
                StreamEvent::ThinkingDelta(delta) => {
                    if let Some(tx) = event_sender {
                        let _ = tx.send(AgentEvent::SubagentMessageUpdate {
                            subagent_id: self.id.clone(),
                            delta: MessageDelta::Thinking(delta),
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
