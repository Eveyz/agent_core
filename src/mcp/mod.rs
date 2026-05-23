pub mod channel;

pub use channel::McpChannel;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub server: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub struct McpClient {
    servers: Vec<McpServerConfig>,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            tools: Vec::new(),
        }
    }

    pub fn add_server(&mut self, config: McpServerConfig) {
        self.servers.push(config);
    }

    pub fn list_servers(&self) -> &[McpServerConfig] {
        &self.servers
    }

    pub fn list_tools(&self) -> &[McpToolDef] {
        &self.tools
    }

    pub fn register_tool(&mut self, tool: McpToolDef) {
        self.tools.push(tool);
    }

    pub fn find_tool(&self, name: &str) -> Option<&McpToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }

    pub async fn call_tool(&self, name: &str, _args: Value) -> Result<String> {
        let tool = self
            .find_tool(name)
            .ok_or_else(|| anyhow::anyhow!("MCP tool '{}' not found", name))?;

        Ok(format!(
            "[MCP] Called tool '{}' on server '{}': (stub implementation)",
            tool.name, tool.server
        ))
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}
