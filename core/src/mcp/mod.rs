//! MCP (Model Context Protocol) client — connect to MCP servers over stdio,
//! discover their tools, and expose them as native agent tools.
//!
//! ## Architecture
//!
//! ```text
//! Config.toml → McpClientManager
//!   ├── Server "filesystem" → StdioTransport (process)
//!   │   ├── initialize     ← JSON-RPC handshake
//!   │   ├── tools/list     ← discover tools
//!   │   └── tools/call     ← invoke on demand
//!   └── Server "github"  → StdioTransport (process)
//!       └── ...
//!
//! ToolRegistry ← McpTool (implements Tool trait)
//!   ├── mcp__filesystem__read_file
//!   ├── mcp__filesystem__write_file
//!   └── mcp__github__search_repos
//! ```
//!
//! Tool names are prefixed: `mcp__<server>__<tool>` to avoid conflicts.

pub mod channel;
pub mod protocol;
pub mod sse;
pub mod tool;
pub mod transport;

pub use channel::McpChannel;
pub use tool::McpTool;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use self::protocol::{
    ClientCapabilities, ClientInfo, ContentItem, InitializeParams, InitializeResult,
    JsonRpcResponse, ToolCallParams, ToolCallResult, ToolsCapability, ToolsListResult,
};
use self::sse::SseTransport;
use self::transport::StdioTransport;

/// Backward-compatible transport enum.
/// New code uses `McpServerConfig` directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    Stdio,
    Sse,
}

// ── Config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// List of MCP servers to connect to.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    /// Transport: "stdio" or "sse". Defaults to "stdio".
    #[serde(default = "default_transport")]
    pub transport: String,
    /// For stdio: the command to spawn.
    #[serde(default)]
    pub command: String,
    /// For stdio: command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// For sse: the base URL of the MCP server.
    #[serde(default)]
    pub url: String,
    /// Environment variables to pass to the server process (stdio only).
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_transport() -> String {
    "stdio".to_string()
}

fn default_true() -> bool {
    true
}

// ── Tool definition ──────────────────────────────────────────────────

/// A tool discovered from an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub server: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl McpToolDef {
    /// Full qualified name: `mcp__<server>__<tool>`
    pub fn qualified_name(&self) -> String {
        format!("mcp__{}__{}", self.server, self.name)
    }
}

// ── Server connection ────────────────────────────────────────────────

/// A connected MCP server — holds the transport + discovered tools.
enum Transport {
    Stdio(StdioTransport),
    Sse(SseTransport),
}

struct McpConnection {
    transport: Transport,
    tools: Vec<McpToolDef>,
}

// ── Client Manager ───────────────────────────────────────────────────

/// Manages connections to multiple MCP servers.
///
/// Usage:
/// ```ignore
/// let mut manager = McpClientManager::new();
/// manager.add_server(McpServerConfig { ... });
/// manager.connect_all().await?;
/// let tools = manager.all_tools();
/// // Register tools into ToolRegistry
/// ```
pub struct McpClientManager {
    servers: Vec<McpServerConfig>,
    connections: HashMap<String, McpConnection>,
}

impl McpClientManager {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            connections: HashMap::new(),
        }
    }

    /// Add a server configuration.
    pub fn add_server(&mut self, config: McpServerConfig) {
        self.servers.push(config);
    }

    /// Load servers from config.
    pub fn from_config(config: &McpConfig) -> Self {
        let mut manager = Self::new();
        for server in &config.servers {
            if server.enabled {
                manager.add_server(server.clone());
            }
        }
        manager
    }

    /// Number of configured servers.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Connect to all configured servers, discover their tools.
    /// Returns (server_name → Vec<error_messages>) for partial failures.
    pub async fn connect_all(&mut self) -> HashMap<String, Vec<String>> {
        let mut errors = HashMap::new();
        let servers: Vec<_> = self.servers.drain(..).collect();

        for config in servers {
            let name = config.name.clone();
            match Self::connect_one(&config).await {
                Ok(conn) => {
                    let tool_count = conn.tools.len();
                    tracing::info!(server = %name, tool_count, "MCP server connected");
                    self.connections.insert(name.clone(), conn);
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    tracing::warn!(server = %name, error = %msg, "MCP server connection failed");
                    errors.entry(name).or_insert_with(Vec::new).push(msg);
                }
            }
        }

        self.servers.clear();
        errors
    }

    /// Connect to a single MCP server.
    async fn connect_one(config: &McpServerConfig) -> Result<McpConnection> {
        let transport = match config.transport.as_str() {
            "sse" => {
                let sse = SseTransport::new(&config.url);
                sse.connect().await?;
                Transport::Sse(sse)
            }
            _ => {
                let stdio = StdioTransport::spawn(&config.command, &config.args).await?;
                Transport::Stdio(stdio)
            }
        };

        // Initialize handshake
        let init_params = serde_json::to_value(InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: false,
                }),
                resources: None,
            },
            client_info: ClientInfo {
                name: "agent_core".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })?;

        let response = transport_request(&transport, "initialize", init_params).await?;
        let _init_result: InitializeResult = serde_json::from_value(response.into_result()?)?;

        transport_notify(
            &transport,
            "notifications/initialized",
            serde_json::json!({}),
        )
        .await?;

        // Discover tools
        let tools_response =
            transport_request(&transport, "tools/list", serde_json::json!({})).await?;
        let tools_list: ToolsListResult = serde_json::from_value(tools_response.into_result()?)?;

        let tools: Vec<McpToolDef> = tools_list
            .tools
            .into_iter()
            .map(|t| McpToolDef {
                server: config.name.clone(),
                name: t.name,
                description: t.description,
                parameters: t.input_schema,
            })
            .collect();

        Ok(McpConnection { transport, tools })
    }

    /// Get all discovered tools across all connected servers.
    pub fn all_tools(&self) -> Vec<McpToolDef> {
        self.connections
            .values()
            .flat_map(|conn| conn.tools.clone())
            .collect()
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(&self, server: &str, tool_name: &str, args: Value) -> Result<String> {
        let conn = self
            .connections
            .get(server)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not connected", server))?;

        if !transport_is_alive(&conn.transport).await {
            anyhow::bail!("MCP server '{}' has died", server);
        }

        let params = serde_json::to_value(ToolCallParams {
            name: tool_name.to_string(),
            arguments: args,
        })?;

        let response = transport_request(&conn.transport, "tools/call", params).await?;
        let call_result: ToolCallResult = serde_json::from_value(response.into_result()?)?;

        if call_result.is_error {
            let text = call_result
                .content
                .iter()
                .filter_map(|c| match c {
                    ContentItem::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!("MCP tool error: {}", text);
        }

        // Extract text content
        let text = call_result
            .content
            .iter()
            .filter_map(|c| match c {
                ContentItem::Text { text } => Some(text.as_str()),
                ContentItem::Image { mime_type, .. } => Some(mime_type.as_str()),
                ContentItem::Resource { .. } => Some("[resource]"),
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }

    /// List connected server names.
    pub fn connected_servers(&self) -> Vec<&str> {
        self.connections.keys().map(|s| s.as_str()).collect()
    }

    /// Shut down all connections.
    pub async fn shutdown_all(&mut self) -> Result<()> {
        for (name, conn) in self.connections.drain() {
            tracing::info!(server = %name, "shutting down MCP server");
            transport_shutdown(conn.transport).await?;
        }
        Ok(())
    }

    /// Get total tool count.
    pub fn tool_count(&self) -> usize {
        self.connections.values().map(|c| c.tools.len()).sum()
    }
}

impl Drop for McpClientManager {
    fn drop(&mut self) {
        // Connections will be killed on drop via kill_on_drop
    }
}

impl Default for McpClientManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Transport dispatch helpers ────────────────────────────────────────

async fn transport_request(
    transport: &Transport,
    method: &str,
    params: Value,
) -> Result<JsonRpcResponse> {
    match transport {
        Transport::Stdio(t) => t.request(method, params).await,
        Transport::Sse(t) => t.request(method, params).await,
    }
}

async fn transport_notify(transport: &Transport, method: &str, params: Value) -> Result<()> {
    match transport {
        Transport::Stdio(t) => t.notify(method, params).await,
        Transport::Sse(t) => t.notify(method, params).await,
    }
}

async fn transport_is_alive(transport: &Transport) -> bool {
    match transport {
        Transport::Stdio(t) => t.is_alive().await,
        Transport::Sse(t) => t.is_connected().await,
    }
}

async fn transport_shutdown(transport: Transport) -> Result<()> {
    match transport {
        Transport::Stdio(t) => t.shutdown().await,
        Transport::Sse(t) => t.shutdown().await,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_def_qualified_name() {
        let def = McpToolDef {
            server: "filesystem".to_string(),
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({}),
        };
        assert_eq!(def.qualified_name(), "mcp__filesystem__read_file");
    }

    #[test]
    fn test_config_default_empty() {
        let config = McpConfig::default();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_manager_new_is_empty() {
        let mgr = McpClientManager::new();
        assert_eq!(mgr.server_count(), 0);
        assert_eq!(mgr.tool_count(), 0);
        assert!(mgr.connected_servers().is_empty());
    }

    #[test]
    fn test_add_server() {
        let mut mgr = McpClientManager::new();
        mgr.add_server(McpServerConfig {
            name: "test".to_string(),
            transport: "stdio".to_string(),
            command: "echo".to_string(),
            args: vec!["hello".to_string()],
            url: String::new(),
            env: HashMap::new(),
            enabled: true,
        });
        assert_eq!(mgr.server_count(), 1);
    }
}
