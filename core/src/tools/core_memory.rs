use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::memory::MemoryManager;
use crate::tools::{Tool, try_lock_memory};

pub struct CoreMemoryAppendTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl CoreMemoryAppendTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for CoreMemoryAppendTool {
    fn name(&self) -> &str {
        "core_memory_append"
    }

    fn description(&self) -> &str {
        "Append to a core memory block. Use block `human` for cross-project user traits \
         (name, habits, language). Use block `persona` for agent personality. \
         Do NOT store project architecture here — use edit on agverse.md instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "block_id": {
                    "type": "string",
                    "description": "The memory block ID (e.g. 'human', 'persona', 'task')"
                },
                "content": {
                    "type": "string",
                    "description": "The content to append"
                }
            },
            "required": ["block_id", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let block_id = args["block_id"].as_str().context("missing 'block_id'")?;
        let content = args["content"].as_str().context("missing 'content'")?;

        let mut memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        memory.core_mut().append(block_id, content)?;

        let block = memory.core().get(block_id);
        Ok(json!({
            "success": true,
            "block_id": block_id,
            "content": block.map(|b| b.content.as_str()).unwrap_or("")
        })
        .to_string())
    }
}

pub struct CoreMemoryReplaceTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl CoreMemoryReplaceTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for CoreMemoryReplaceTool {
    fn name(&self) -> &str {
        "core_memory_replace"
    }

    fn description(&self) -> &str {
        "Replace text in a core memory block (`human` or `persona`). \
         For project-specific rules, edit agverse.md instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "block_id": {
                    "type": "string",
                    "description": "The memory block ID"
                },
                "old_content": {
                    "type": "string",
                    "description": "The text to find and replace"
                },
                "new_content": {
                    "type": "string",
                    "description": "The replacement text"
                }
            },
            "required": ["block_id", "old_content", "new_content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let block_id = args["block_id"].as_str().context("missing 'block_id'")?;
        let old_content = args["old_content"]
            .as_str()
            .context("missing 'old_content'")?;
        let new_content = args["new_content"]
            .as_str()
            .context("missing 'new_content'")?;

        let mut memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        memory
            .core_mut()
            .replace(block_id, old_content, new_content)?;

        let block = memory.core().get(block_id);
        Ok(json!({
            "success": true,
            "block_id": block_id,
            "content": block.map(|b| b.content.as_str()).unwrap_or("")
        })
        .to_string())
    }
}

pub struct CoreMemoryReadTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl CoreMemoryReadTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for CoreMemoryReadTool {
    fn name(&self) -> &str {
        "core_memory_read"
    }

    fn description(&self) -> &str {
        "Read a core memory block (`human`, `persona`, etc.). \
         Project rules live in agverse.md — use read_file for those."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "block_id": {
                    "type": "string",
                    "description": "The memory block ID to read"
                }
            },
            "required": ["block_id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let block_id = args["block_id"].as_str().context("missing 'block_id'")?;

        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        match memory.core().get(block_id) {
            Some(block) => Ok(json!({
                "block_id": block_id,
                "label": block.label,
                "content": block.content,
                "updated_at": block.updated_at
            })
            .to_string()),
            None => Ok(json!({
                "error": format!("memory block '{}' not found", block_id)
            })
            .to_string()),
        }
    }
}
