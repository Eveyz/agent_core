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
pub mod webfetch;
pub mod write_file;

use crate::memory::MemoryManager;
use crate::types::{EventSender, FunctionSchema, ToolCall, ToolDefinition, ToolExecutionMode};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Callback for streaming tool progress updates.
pub type ToolUpdateFn = Arc<dyn Fn(&str) + Send + Sync>;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;

    /// Execute the tool and return a complete result string.
    async fn execute(&self, args: Value) -> Result<String>;

    /// Execute with streaming progress updates and optional event sender.
    /// - `on_update`: fire-and-forget progress callback (e.g. for stdout streaming)
    /// - `event_sender`: channel to emit structured `AgentEvent`s back to the parent
    ///
    /// Default delegates to `execute`, ignoring both callbacks.
    async fn execute_with_stream(
        &self,
        args: Value,
        on_update: Option<ToolUpdateFn>,
        event_sender: Option<EventSender>,
    ) -> Result<String> {
        let _ = on_update;
        let _ = event_sender;
        self.execute(args).await
    }

    /// Per-tool execution mode override. `None` means use global config.
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }
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
        registry.register(Box::new(webfetch::WebFetchTool));
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

    /// Execute all tool calls sequentially.
    pub async fn call_all(&self, calls: &[ToolCall]) -> Vec<String> {
        self.call_all_with_sender(calls, None).await
    }

    /// Execute all tool calls sequentially, forwarding events via sender.
    pub async fn call_all_with_sender(
        &self,
        calls: &[ToolCall],
        event_sender: Option<EventSender>,
    ) -> Vec<String> {
        let mut results = Vec::new();

        for call in calls {
            let result = self.call_one(call, None, event_sender.clone()).await;
            results.push(result);
        }

        results
    }

    /// Execute a single tool call with optional streaming callback and event sender.
    pub async fn call_one(
        &self,
        call: &ToolCall,
        on_update: Option<ToolUpdateFn>,
        event_sender: Option<EventSender>,
    ) -> String {
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
        match tool
            .execute_with_stream(args, on_update, event_sender)
            .await
        {
            Ok(output) => output,
            Err(e) => {
                format!("Error executing tool '{}': {}", name, e)
            }
        }
    }

    /// Determine the execution mode for a batch of tool calls.
    /// If any tool in the batch has `execution_mode: Sequential`, the whole batch runs sequentially.
    pub fn resolve_execution_mode(
        &self,
        calls: &[ToolCall],
        global_mode: ToolExecutionMode,
    ) -> ToolExecutionMode {
        for call in calls {
            if let Some(tool) = self.tools.get(&call.function.name)
                && let Some(mode) = tool.execution_mode()
                && mode == ToolExecutionMode::Sequential
            {
                return ToolExecutionMode::Sequential;
            }
        }
        global_mode
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Build a registry from tool names using the factory. Unknown names are silently skipped.
    pub fn from_names(names: &[String]) -> Self {
        let mut registry = Self::new();
        for name in names {
            if let Some(tool) = build_tool_by_name(name) {
                registry.register(tool);
            }
        }
        registry
    }

    pub fn clone_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

/// Factory: build a tool from its name. Returns None for unknown or memory tools
/// (which require Arc<Mutex<MemoryManager>> and can't be built from name alone).
pub fn build_tool_by_name(name: &str) -> Option<Box<dyn Tool>> {
    match name {
        "read_file" => Some(Box::new(read_file::ReadFileTool)),
        "write_file" => Some(Box::new(write_file::WriteFileTool)),
        "edit" => Some(Box::new(edit::EditTool)),
        "grep" => Some(Box::new(grep::GrepTool)),
        "glob" => Some(Box::new(glob::GlobTool)),
        "run_command" => Some(Box::new(run_command::RunCommandTool)),
        "webfetch" => Some(Box::new(webfetch::WebFetchTool)),
        "git_status" => Some(Box::new(git::GitStatusTool)),
        "git_diff" => Some(Box::new(git::GitDiffTool)),
        "git_log" => Some(Box::new(git::GitLogTool)),
        "git_commit" => Some(Box::new(git::GitCommitTool)),
        "git_show" => Some(Box::new(git::GitShowTool)),
        _ => None,
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
