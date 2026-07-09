use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::memory::MemoryManager;
use crate::tools::{Tool, try_lock_memory};

pub struct ConversationSearchTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl ConversationSearchTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ConversationSearchTool {
    fn name(&self) -> &str {
        "conversation_search"
    }

    fn description(&self) -> &str {
        "Search past conversation history (keyword + salience ranking, hybrid when embeddings available). \
         USE when: user asks about prior discussions, preferences, or decisions; you need context from \
         earlier sessions; continuing work from before. Do NOT use for current codebase — use grep/read_file."
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
                    "description": "Number of results to return (default 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"].as_str().context("missing 'query'")?;
        let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

        // Compute embedding outside the memory lock when available.
        let embedding = {
            let memory = match try_lock_memory(&self.memory) {
                Ok(m) => m,
                Err(busy_msg) => return Ok(busy_msg),
            };
            memory
                .embedding_model()
                .and_then(|model| model.embed_single(query).ok())
        };

        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };

        let results = if let Some(ref emb) = embedding {
            memory
                .search_conversation_precomputed(emb, query, top_k)
                .unwrap_or_else(|_| {
                    memory
                        .search_conversation_bm25_with_salience(query, top_k)
                        .unwrap_or_default()
                })
        } else {
            memory.search_conversation_bm25_with_salience(query, top_k)?
        };

        let items: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "role": r.role,
                    "content": r.content,
                    "created_at": r.created_at,
                    "importance": r.importance
                })
            })
            .collect();

        Ok(json!({ "results": items }).to_string())
    }
}

pub struct ConversationSearchDateTool {
    memory: Arc<Mutex<MemoryManager>>,
}

impl ConversationSearchDateTool {
    pub fn new(memory: Arc<Mutex<MemoryManager>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ConversationSearchDateTool {
    fn name(&self) -> &str {
        "conversation_search_date"
    }

    fn description(&self) -> &str {
        "Search conversation history by date range."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "start_date": {
                    "type": "string",
                    "description": "Start date in ISO 8601 format (e.g. 2024-01-01T00:00:00Z)"
                },
                "end_date": {
                    "type": "string",
                    "description": "End date in ISO 8601 format"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Number of results (default 10)",
                    "default": 10
                }
            },
            "required": ["start_date", "end_date"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let start = args["start_date"]
            .as_str()
            .context("missing 'start_date'")?;
        let end = args["end_date"].as_str().context("missing 'end_date'")?;
        let top_k = args["top_k"].as_u64().unwrap_or(10) as usize;

        let memory = match try_lock_memory(&self.memory) {
            Ok(m) => m,
            Err(busy_msg) => return Ok(busy_msg),
        };
        let results = memory.recall().search_by_date(start, end, top_k)?;

        let items: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "role": r.role,
                    "content": r.content,
                    "created_at": r.created_at
                })
            })
            .collect();

        Ok(json!({ "results": items }).to_string())
    }
}
