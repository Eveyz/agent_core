//! Bridge: register MCP tools into the agent's ToolRegistry.
//!
//! Each MCP tool becomes a native `Tool` that forwards calls to the MCP server.

use crate::mcp::McpClientManager;
use crate::tools::Tool;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Tells the model the desktop UI already renders a map from this tool result.
const MAP_UI_HINT: &str = " The Agverse app shows an interactive map and place cards from this result. In your final reply do not paste coordinates, Place IDs, or a Google Maps link; a place name or address is enough. Add only what the map cannot: what it is, how to get there, which exit, whether it is open.";

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
        let description = annotate_map_tool_description(&server, &tool_name, description);
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

/// Places / routes tools whose JSON the desktop UI turns into a map widget.
pub(crate) fn is_map_display_tool(server: &str, tool_name: &str) -> bool {
    let tool = tool_name.to_ascii_lowercase();
    if tool.contains("weather") || tool.contains("ip_location") {
        return false;
    }
    if tool.contains("search_places")
        || tool.contains("compute_routes")
        || tool.contains("resolve_names")
        || tool.contains("resolve_maps_urls")
        || tool.contains("maps_text_search")
        || tool.contains("maps_around_search")
        || tool.contains("maps_search_detail")
        || tool.contains("maps_geo")
        || tool.contains("maps_regeocode")
        || tool.contains("maps_direction")
        || tool.contains("maps_bicycling")
        || tool.contains("maps_distance")
    {
        return true;
    }
    let server = server.to_ascii_lowercase();
    (server.contains("amap") || server.contains("gaode") || server.contains("map"))
        && (tool.contains("place") || tool.contains("route") || tool.contains("direction"))
}

fn annotate_map_tool_description(server: &str, tool_name: &str, description: String) -> String {
    if !is_map_display_tool(server, tool_name) {
        return description;
    }
    if description.contains("interactive map and place cards") {
        return description;
    }
    let mut out = description;
    if !out.ends_with('.') && !out.ends_with(' ') {
        out.push('.');
    }
    out.push_str(MAP_UI_HINT);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_tools_get_ui_hint() {
        let d = annotate_map_tool_description(
            "google-map",
            "search_places",
            "Find places on Google Maps".into(),
        );
        assert!(d.contains("Find places on Google Maps"));
        assert!(d.contains("do not paste coordinates"));
        assert!(d.contains("interactive map and place cards"));
    }

    #[test]
    fn weather_and_unrelated_tools_are_unchanged() {
        let weather = annotate_map_tool_description(
            "google-map",
            "lookup_weather",
            "Look up the weather".into(),
        );
        assert_eq!(weather, "Look up the weather");

        let fetch = annotate_map_tool_description("fetch", "fetch", "Fetch a URL".into());
        assert_eq!(fetch, "Fetch a URL");
    }

    #[test]
    fn hint_is_not_duplicated() {
        let once =
            annotate_map_tool_description("amap-maps", "maps_text_search", "Search POIs".into());
        let twice = annotate_map_tool_description("amap-maps", "maps_text_search", once.clone());
        assert_eq!(once.matches("interactive map and place cards").count(), 1);
        assert_eq!(twice, once);
    }

    #[test]
    fn resolve_names_and_routes_count_as_map_display() {
        assert!(is_map_display_tool("google-map", "resolve_names"));
        assert!(is_map_display_tool("maps-grounding-lite", "compute_routes"));
        assert!(is_map_display_tool("amap-maps", "maps_direction_driving"));
        assert!(!is_map_display_tool("parallel-search", "web_search"));
    }
}
