//! McpChannel — a lightweight handle to a single MCP server connection.
//! Kept for backward compatibility and as a convenience wrapper.

use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{McpClientManager, McpToolDef};

/// A channel to a specific MCP server.
/// Wraps the shared McpClientManager and forwards calls to the named server.
pub struct McpChannel {
    server: String,
    manager: Arc<Mutex<McpClientManager>>,
}

impl McpChannel {
    pub fn new(server: &str, manager: Arc<Mutex<McpClientManager>>) -> Self {
        Self {
            server: server.to_string(),
            manager,
        }
    }

    /// Get tool definitions for this server.
    pub async fn tool_definitions(&self) -> Vec<McpToolDef> {
        let mgr = self.manager.lock().await;
        mgr.all_tools()
            .into_iter()
            .filter(|t| t.server == self.server)
            .collect()
    }

    /// Call a tool on this server.
    pub async fn invoke(&self, tool_name: &str, args: &Value) -> Result<String> {
        let mut mgr = self.manager.lock().await;
        mgr.call_tool(&self.server, tool_name, args.clone()).await
    }

    /// Convert tool definitions to the format expected by LLM tool schema.
    pub async fn tool_schemas(&self) -> Vec<Value> {
        let defs = self.tool_definitions().await;
        defs.iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.qualified_name(),
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }
}
