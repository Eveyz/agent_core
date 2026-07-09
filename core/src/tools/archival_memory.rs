use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::memory::MemoryManager;
use crate::tools::{Tool, try_lock_memory};

pub struct ArchivalMemoryInsertTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl ArchivalMemoryInsertTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ArchivalMemoryInsertTool {
    fn name(&self) -> &str {
        "archival_memory_insert"
    }

    fn description(&self) -> &str {
        "Store long-term knowledge in archival memory. Use for important facts too verbose \
         for core blocks or agverse.md. Prefer agverse.md for active project conventions."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The knowledge content to store"
                },
                "metadata": {
                    "type": "string",
                    "description": "Optional metadata (JSON string)"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let content = args["content"].as_str().context("missing 'content'")?;
        let metadata = args["metadata"].as_str();

        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        let id = memory.archival().insert(content, metadata)?;

        Ok(json!({
            "success": true,
            "id": id,
            "message": "Content stored in archival memory"
        })
        .to_string())
    }
}

pub struct ArchivalMemorySearchTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl ArchivalMemorySearchTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ArchivalMemorySearchTool {
    fn name(&self) -> &str {
        "archival_memory_search"
    }

    fn description(&self) -> &str {
        "Search archival memory for long-term stored knowledge. \
         USE when: looking up distilled facts, old decisions, or knowledge promoted from recall. \
         For recent conversations use conversation_search instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Number of results (default 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"].as_str().context("missing 'query'")?;
        let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

        // Pure keyword search via SQLite FTS5 — no embedding model needed.
        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        let results = memory.archival().search_by_keyword(query, top_k)?;

        let items: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "content": r.content,
                    "metadata": r.metadata,
                    "created_at": r.created_at
                })
            })
            .collect();

        Ok(json!({ "results": items }).to_string())
    }
}

pub struct ArchivalMemoryDeleteTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl ArchivalMemoryDeleteTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ArchivalMemoryDeleteTool {
    fn name(&self) -> &str {
        "archival_memory_delete"
    }

    fn description(&self) -> &str {
        "Delete a record from archival memory by ID."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The record ID to delete"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let id = args["id"].as_str().context("missing 'id'")?;

        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        let deleted = memory.archival().delete(id)?;

        Ok(json!({
            "success": deleted,
            "message": if deleted { "Record deleted" } else { "Record not found" }
        })
        .to_string())
    }
}
