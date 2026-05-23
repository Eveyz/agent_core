use super::McpToolDef;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

pub struct McpChannel {
    tools: HashMap<String, McpToolDef>,
}

impl Default for McpChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl McpChannel {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register_tool(&mut self, tool: McpToolDef) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }

    pub async fn invoke(&self, name: &str, args: &Value) -> Result<String> {
        let _tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("MCP tool '{}' not found in channel", name))?;

        Ok(format!(
            "[MCP Channel] Invoked '{}' with args {}: (stub)",
            name, args
        ))
    }
}
