pub mod archival_memory;
pub mod bash;
pub mod core_memory;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read_file;
pub mod recall_memory;
pub mod skill;
pub mod subagent;
pub mod tavily_search;
pub mod todo;
pub mod webfetch;
pub mod write_file;

use crate::memory::MemoryManager;
use crate::types::{EventSender, FunctionSchema, ToolCall, ToolDefinition, ToolExecutionMode};
use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Try to acquire the MemoryManager lock with a timeout.
/// Returns `Ok(guard)` on success, or a JSON busy-message string to return to the LLM.
pub fn try_lock_memory(
    memory: &Arc<Mutex<MemoryManager>>,
) -> std::result::Result<parking_lot::MutexGuard<'_, MemoryManager>, String> {
    memory
        .try_lock_for(Duration::from_secs(3))
        .ok_or_else(|| {
            serde_json::json!({
                "error": "memory_store_busy",
                "message": "Memory store is temporarily busy (may be consolidating). Retry the same call in a moment — do NOT change the query or parameters."
            })
            .to_string()
        })
}

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
        registry.register(Box::new(bash::BashTool::new()));
        registry.register(Box::new(webfetch::WebFetchTool::new()));
        if let Some(tool) = tavily_search::TavilySearchTool::from_env() {
            registry.register(Box::new(tool));
        }
        registry
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Returns a fingerprint of the current registry contents.
    /// Compares tool names and their parameter schemas so callers can skip
    /// catalog rendering when nothing changed.
    pub fn registry_fingerprint(&self) -> String {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        // Simple fingerprint: concat all tool names — sufficient because tool
        // names form a namespace and parameter schemas are stable per name.
        names.join("|")
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<_> = self
            .tools
            .values()
            .map(|t| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionSchema {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: canonicalize_json_object(&t.parameters_schema()),
                },
            })
            .collect();
        defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
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

    /// Remove tools by name. Unknown names are silently ignored.
    /// Used to filter tools based on [`AgentMode`](crate::mode::AgentMode).
    pub fn remove_all(&mut self, names: &[&str]) {
        for name in names {
            self.tools.remove(*name);
        }
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
        "bash" => Some(Box::new(bash::BashTool::new())),
        "webfetch" => Some(Box::new(webfetch::WebFetchTool::new())),
        "tavily_search" => {
            tavily_search::TavilySearchTool::from_env().map(|t| Box::new(t) as Box<dyn Tool>)
        }
        _ => None,
    }
}

pub fn register_memory_tools(registry: &mut ToolRegistry, memory: Arc<Mutex<MemoryManager>>) {
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

/// Recursively canonicalize a JSON object by sorting keys alphabetically.
/// This ensures tool parameter schemas produce identical byte representations
/// across calls, which is critical for prompt cache hit rate.
fn canonicalize_json_object(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut canonical = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json_object(&map[key]));
            }
            serde_json::Value::Object(canonical)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_json_object).collect())
        }
        _ => value.clone(),
    }
}
