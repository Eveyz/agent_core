use crate::todo::{SessionPlanStore, TodoStatus};
use crate::tools::{Tool, ToolRegistry};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub fn register_todo_tools(
    registry: &mut ToolRegistry,
    store: Arc<SessionPlanStore>,
    session_id: Option<String>,
    prompt_id: Option<String>,
) {
    registry.register(Box::new(TodoWriteTool::new(
        store.clone(),
        session_id.clone(),
        prompt_id.clone(),
    )));
    registry.register(Box::new(TodoReadTool::new(
        store.clone(),
        session_id.clone(),
        prompt_id.clone(),
    )));
    registry.register(Box::new(TodoUpdateTool::new(store, session_id, prompt_id)));
}

struct TodoWriteTool {
    store: Arc<SessionPlanStore>,
    session_id: Option<String>,
    prompt_id: Option<String>,
}

impl TodoWriteTool {
    fn new(
        store: Arc<SessionPlanStore>,
        session_id: Option<String>,
        prompt_id: Option<String>,
    ) -> Self {
        Self {
            store,
            session_id,
            prompt_id,
        }
    }
}

/// Parse `items` from either `["a","b"]` or `[{"content":"a"},{"description":"b"}]`.
fn parse_todo_items(args: &Value) -> Result<Vec<String>> {
    let arr = args
        .get("items")
        .or_else(|| args.get("todos"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "missing 'items' array. Example: {{\"items\":[\"step 1\",\"step 2\"]}}. \
                 Got keys: {:?}",
                args.as_object()
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default()
            )
        })?;

    let mut items = Vec::new();
    for v in arr {
        if let Some(s) = v.as_str() {
            let s = s.trim();
            if !s.is_empty() {
                items.push(s.to_string());
            }
            continue;
        }
        if let Some(obj) = v.as_object() {
            let text = obj
                .get("content")
                .or_else(|| obj.get("description"))
                .or_else(|| obj.get("text"))
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if let Some(t) = text {
                items.push(t);
            }
        }
    }

    if items.is_empty() {
        anyhow::bail!(
            "'items' must not be empty. Pass string descriptions, e.g. \
             {{\"items\":[\"Create models\",\"Write tests\"]}}"
        );
    }
    Ok(items)
}

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "Create or update a todo plan for complex multi-step work only. \
         Skip this tool for simple 1–2 step tasks — just execute with other tools. \
         By default MERGES with existing progress (completed/in_progress items with the \
         same description are preserved). If the new items look like a different job, \
         the current plan is parked and a new active plan is created. \
         Pass force=true only when you must fully replace the active plan (wipes statuses). \
         Prefer todo_update to advance steps; do not replan every turn. \
         Mid-run steers stay on the same plan — continue, rewrite, or park; do not invent a new prompt."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {"type": "string"},
                            {
                                "type": "object",
                                "properties": {
                                    "content": {"type": "string"},
                                    "description": {"type": "string"}
                                }
                            }
                        ]
                    },
                    "description": "Task descriptions (strings or {content|description} objects)."
                },
                "force": {
                    "type": "boolean",
                    "description": "If true, wipe and replace the entire active plan. Default false (merge or park+create)."
                }
            },
            "required": ["items"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let items = parse_todo_items(&args)?;
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        self.store
            .write_plan(
                self.session_id.as_deref(),
                items,
                force,
                self.prompt_id.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

struct TodoReadTool {
    store: Arc<SessionPlanStore>,
    session_id: Option<String>,
    prompt_id: Option<String>,
}

impl TodoReadTool {
    fn new(
        store: Arc<SessionPlanStore>,
        session_id: Option<String>,
        prompt_id: Option<String>,
    ) -> Self {
        Self {
            store,
            session_id,
            prompt_id,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todo_read"
    }

    fn description(&self) -> &str {
        "Read the current active todo plan with status of all items. \
         Also lists parked plans if any."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let _ = self.prompt_id; // tools are bound to a prompt at registry build time
        let mut out = self
            .store
            .with_active(self.session_id.as_deref(), |list| list.to_context_string());
        if let Some(line) = self.store.parked_injection_line(self.session_id.as_deref()) {
            out.push('\n');
            out.push_str(&line);
        }
        Ok(out)
    }
}

struct TodoUpdateTool {
    store: Arc<SessionPlanStore>,
    session_id: Option<String>,
    prompt_id: Option<String>,
}

impl TodoUpdateTool {
    fn new(
        store: Arc<SessionPlanStore>,
        session_id: Option<String>,
        prompt_id: Option<String>,
    ) -> Self {
        Self {
            store,
            session_id,
            prompt_id,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TodoUpdateTool {
    fn name(&self) -> &str {
        "todo_update"
    }

    fn description(&self) -> &str {
        "Update a todo item status on the active plan. Args: id (string), \
         status (pending/in_progress/completed/blocked). \
         Completing a step auto-promotes the next ready item to in_progress."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Todo item ID"},
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "blocked"],
                    "description": "New status"
                }
            },
            "required": ["id", "status"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let _ = self.prompt_id;
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let status = match args["status"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'status'"))?
        {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            "blocked" => TodoStatus::Blocked,
            s => anyhow::bail!("invalid status: {}", s),
        };

        self.store
            .update_item(self.session_id.as_deref(), id, status)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_string_items() {
        let args = serde_json::json!({"items": ["a", "b"]});
        assert_eq!(parse_todo_items(&args).unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn parse_object_items() {
        let args = serde_json::json!({
            "items": [
                {"content": "Write models"},
                {"description": "Write tests"}
            ]
        });
        assert_eq!(
            parse_todo_items(&args).unwrap(),
            vec!["Write models", "Write tests"]
        );
    }

    #[test]
    fn parse_todos_alias() {
        let args = serde_json::json!({"todos": ["x"]});
        assert_eq!(parse_todo_items(&args).unwrap(), vec!["x"]);
    }
}
