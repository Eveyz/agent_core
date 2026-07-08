//! Bridge: register MCP tools into the agent's ToolRegistry.
//!
//! Each MCP tool becomes a native `Tool` that forwards calls to the MCP server.

use crate::mcp::McpClientManager;
use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// An MCP tool wrapped as a native agent tool.
///
/// Tool names are prefixed: `mcp__<server>__<tool>` to avoid naming conflicts
/// and to clearly indicate the tool's origin.
pub struct McpTool {
    qualified_name: String,
    description: String,
    parameters: Value,
    server: String,
    tool_name: String,
    manager: Arc<Mutex<McpClientManager>>,
}

impl McpTool {
    pub fn new(
        server: String,
        tool_name: String,
        description: String,
        parameters: Value,
        manager: Arc<Mutex<McpClientManager>>,
    ) -> Self {
        let qualified_name = format!("mcp__{}__{}", server, tool_name);
        Self {
            qualified_name,
            description,
            parameters,
            server,
            tool_name,
            manager,
        }
    }

    /// Register all tools from a connected McpClientManager into a ToolRegistry.
    pub fn register_all(
        registry: &mut crate::tools::ToolRegistry,
        manager: Arc<Mutex<McpClientManager>>,
    ) {
        let mgr = match manager.try_lock() {
            Ok(m) => m,
            Err(_) => return,
        };

        for tool_def in mgr.all_tools() {
            let mcp_tool = McpTool::new(
                tool_def.server,
                tool_def.name,
                tool_def.description,
                tool_def.parameters,
                manager.clone(),
            );
            registry.register(Box::new(mcp_tool));
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let mut mgr = self.manager.lock().await;
        mgr.call_tool(&self.server, &self.tool_name, args).await
    }
}
