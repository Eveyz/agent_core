pub mod archival_memory;
pub mod core_memory;
pub mod edit;
pub mod git;
pub mod glob;
pub mod grep;
pub mod read_file;
pub mod recall_memory;
pub mod run_command;
pub mod skill;
pub mod subagent;
pub mod todo;
pub mod write_file;

use crate::memory::MemoryManager;
use crate::types::{FunctionSchema, ToolCall, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<String>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(read_file::ReadFileTool));
        registry.register(Box::new(write_file::WriteFileTool));
        registry.register(Box::new(edit::EditTool));
        registry.register(Box::new(grep::GrepTool));
        registry.register(Box::new(glob::GlobTool));
        registry.register(Box::new(run_command::RunCommandTool));
        registry.register(Box::new(git::GitStatusTool));
        registry.register(Box::new(git::GitDiffTool));
        registry.register(Box::new(git::GitLogTool));
        registry.register(Box::new(git::GitCommitTool));
        registry.register(Box::new(git::GitShowTool));
        registry
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionSchema {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters_schema(),
                },
            })
            .collect()
    }

    pub fn validate_args(&self, name: &str, args: &Value) -> Result<()> {
        let tool = self
            .tools
            .get(name)
            .with_context(|| format!("tool '{name}' not found"))?;

        let schema = tool.parameters_schema();
        let validator =
            jsonschema::validator_for(&schema).context("failed to create JSON schema validator")?;

        if let Err(e) = validator.validate(args) {
            anyhow::bail!("validation failed for tool '{name}': {e}");
        }

        Ok(())
    }

    pub async fn call_all(&self, calls: &[ToolCall]) -> Vec<String> {
        let mut results = Vec::new();

        for call in calls {
            let result = self.call_one(call).await;
            results.push(result);
        }

        results
    }

    async fn call_one(&self, call: &ToolCall) -> String {
        let tool = match self.tools.get(&call.function.name) {
            Some(t) => t,
            None => {
                return format!(
                    "Error: tool '{}' not found. Available tools: {}",
                    call.function.name,
                    self.list_names().join(", ")
                );
            }
        };

        let args: Value = match serde_json::from_str(&call.function.arguments) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "Error: invalid JSON arguments for tool '{}': {}. Raw: {}",
                    call.function.name, e, call.function.arguments
                );
            }
        };

        if let Err(e) = self.validate_args(&call.function.name, &args) {
            return format!(
                "Error: argument validation failed for '{}': {}",
                call.function.name, e
            );
        }

        let name = call.function.name.clone();
        match tool.execute(args).await {
            Ok(output) => output,
            Err(e) => {
                format!("Error executing tool '{}': {}", name, e)
            }
        }
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn clone_subset(&self, _names: &[&str]) -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn clone_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

pub fn register_memory_tools(registry: &mut ToolRegistry, memory: Arc<Mutex<MemoryManager>>) {
    registry.register(Box::new(core_memory::CoreMemoryAppendTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(core_memory::CoreMemoryReplaceTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(core_memory::CoreMemoryReadTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(recall_memory::ConversationSearchTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(recall_memory::ConversationSearchDateTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(archival_memory::ArchivalMemoryInsertTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(archival_memory::ArchivalMemorySearchTool::new(
        memory.clone(),
    )));
    registry.register(Box::new(archival_memory::ArchivalMemoryDeleteTool::new(
        memory,
    )));
}
