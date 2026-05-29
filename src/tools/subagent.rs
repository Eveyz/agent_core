use crate::config::ModelConfig;
use crate::subagent::{Subagent, SubagentConfig};
use crate::tools::{Tool, ToolRegistry, ToolUpdateFn};
use crate::types::EventSender;
use anyhow::Result;
use serde_json::Value;

pub fn register_subagent_tools(
    registry: &mut ToolRegistry,
    model_config: ModelConfig,
    available_tool_names: Vec<String>,
) {
    registry.register(Box::new(SubagentSpawnTool::new(
        model_config,
        available_tool_names,
    )));
}

struct SubagentSpawnTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
}

impl SubagentSpawnTool {
    fn new(model_config: ModelConfig, available_tools: Vec<String>) -> Self {
        Self {
            model_config,
            available_tools,
        }
    }
}

#[async_trait::async_trait]
impl Tool for SubagentSpawnTool {
    fn name(&self) -> &str {
        "subagent_spawn"
    }

    fn description(&self) -> &str {
        "Spawn a sub-agent for complex tasks that benefit from isolated context. Use for: multi-step research, tasks needing a clean slate, parallel work. Do NOT use for: simple file reads, single commands, quick searches — handle those yourself. Args: id (string), task (string), system_prompt (optional string), tools (optional array of tool names), max_iterations (optional int)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Unique sub-agent ID (e.g. 'researcher', 'writer')"
                },
                "task": {
                    "type": "string",
                    "description": "The task description for the sub-agent to complete"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Custom system prompt for the sub-agent"
                },
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tool names to give the sub-agent (empty = no tools, 'all' = all parent tools)"
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Max agent loop iterations (default: 5)"
                }
            },
            "required": ["id", "task"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        _on_update: Option<ToolUpdateFn>,
        event_sender: Option<EventSender>,
    ) -> Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'task'"))?;

        let system_prompt = args["system_prompt"]
            .as_str()
            .unwrap_or("You are a focused sub-agent. Complete the given task and return the result. Be concise.")
            .to_string();

        let max_iterations = args["max_iterations"].as_u64().unwrap_or(5) as usize;

        let tools_list: Vec<String> = if let Some(arr) = args["tools"].as_array() {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        } else {
            Vec::new()
        };

        let is_all = args["tools"]
            .as_str()
            .map(|s| s == "all")
            .or_else(|| {
                args["tools"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s == "all")
            })
            .unwrap_or(false);

        let config = SubagentConfig {
            system_prompt,
            tools: if is_all {
                self.available_tools.clone()
            } else {
                tools_list
            },
            max_iterations,
            max_context_tokens: 32000,
        };

        let mut subagent = Subagent::new(id, config, &self.model_config, ToolRegistry::new());

        let result = subagent.run_with_sender(task, event_sender).await?;

        let mut output = format!(
            "[Sub-agent '{}'] ({} iterations, {})\n\n{}",
            result.subagent_id,
            result.iterations_used,
            if result.success {
                "success"
            } else {
                "incomplete"
            },
            result.output
        );

        if !result.success {
            output.push_str("\n\nNote: Sub-agent did not complete within iteration limit.");
        }

        Ok(output)
    }
}
