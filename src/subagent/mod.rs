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
    pub output: String,
    pub iterations_used: usize,
    pub success: bool,
}

pub struct Subagent {
    id: String,
    config: SubagentConfig,
    client: OpenAIClient,
    context: Context,
    registry: ToolRegistry,
}

impl Subagent {
    pub fn new(
        id: &str,
        config: SubagentConfig,
        model_config: &ModelConfig,
        registry: ToolRegistry,
    ) -> Self {
        let client = OpenAIClient::new(model_config.clone());
        let context = Context::new(&config.system_prompt, config.max_context_tokens);

        Self {
            id: id.to_string(),
            config,
            client,
            context,
            registry,
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
                        success: true,
                        iterations_used: iteration + 1,
                    });
                }

                return Ok(SubagentResult {
                    subagent_id: self.id.clone(),
                    output: text,
                    iterations_used: iteration + 1,
                    success: true,
                });
            }

            if !text.is_empty() {
                self.context
                    .add(Message::assistant_with_tools(&text, tool_calls.clone()));
            }

            // Execute tools, emitting SubagentToolStart/SubagentToolEnd events
            let results = self
                .registry
                .call_all_with_sender(&tool_calls, event_sender.clone())
                .await;

            for (call, result) in tool_calls.iter().zip(&results) {
                // Emit SubagentToolStart
                if let Some(ref tx) = event_sender {
                    let args: serde_json::Value =
                        serde_json::from_str(&call.function.arguments).unwrap_or_default();
                    let _ = tx.send(AgentEvent::SubagentToolStart {
                        subagent_id: self.id.clone(),
                        tool_call_id: call.id.clone(),
                        tool_name: call.function.name.clone(),
                        args,
                    });
                }

                // Emit SubagentToolEnd
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
                    .add(Message::tool(call.id.clone(), result.clone()));
            }
        }

        // Emit SubagentEnd (max iterations reached)
        if let Some(ref tx) = event_sender {
            let _ = tx.send(AgentEvent::SubagentEnd {
                subagent_id: self.id.clone(),
                success: false,
                iterations_used: self.config.max_iterations,
            });
        }

        Ok(SubagentResult {
            subagent_id: self.id.clone(),
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
