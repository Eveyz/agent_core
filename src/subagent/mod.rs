use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::client::OpenAIClient;
use crate::config::ModelConfig;
use crate::context::Context;
use crate::tools::ToolRegistry;
use crate::types::{Message, StreamEvent, ToolCall};

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
        self.context.add(Message::user(task));

        for iteration in 0..self.config.max_iterations {
            self.context.trim_to_fit();

            let messages = self.context.messages();
            let tools = self.registry.tool_definitions();

            let stream = self
                .client
                .chat_completion_stream(&messages, &tools)
                .await?;

            let (text, tool_calls) = self.collect_stream(stream).await?;

            if tool_calls.is_empty() {
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

            match self.registry.call_all(&tool_calls).await {
                results => {
                    for (call, result) in tool_calls.iter().zip(&results) {
                        self.context
                            .add(Message::tool(call.id.clone(), result.clone()));
                    }
                }
            }
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
                    text_buffer.push_str(&delta);
                }
                StreamEvent::ToolCallDelta { .. } => {
                    has_tool_calls = true;
                    accumulator.push(event);
                }
                StreamEvent::Done => break,
                _ => {}
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
