use crate::todo::{TodoList, TodoStatus};
use crate::tools::{Tool, ToolRegistry};
use anyhow::Result;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;

pub fn register_todo_tools(registry: &mut ToolRegistry, todo_list: Arc<Mutex<TodoList>>) {
    registry.register(Box::new(TodoWriteTool::new(todo_list.clone())));
    registry.register(Box::new(TodoReadTool::new(todo_list.clone())));
    registry.register(Box::new(TodoUpdateTool::new(todo_list)));
}

struct TodoWriteTool {
    todo_list: Arc<Mutex<TodoList>>,
}

impl TodoWriteTool {
    fn new(todo_list: Arc<Mutex<TodoList>>) -> Self {
        Self { todo_list }
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
         same description are preserved). Pass force=true only when you must fully replace \
         the plan (wipes statuses). Prefer todo_update to advance steps; do not replan every turn."
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
                    "description": "If true, wipe and replace the entire plan. Default false (merge)."
                }
            },
            "required": ["items"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let items = parse_todo_items(&args)?;
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut list = self.todo_list.lock();
        let had_progress = list.items.iter().any(|i| {
            matches!(
                i.status,
                TodoStatus::Completed | TodoStatus::InProgress
            )
        });

        if force {
            list.replace_all(items);
            let _ = list.ensure_active_step();
            Ok(format!(
                "[plan replaced force=true]\n{}",
                list.to_context_string()
            ))
        } else {
            if had_progress {
                list.merge_replace(items);
                Ok(format!(
                    "[plan merged — prior progress preserved]\n{}",
                    list.to_context_string()
                ))
            } else {
                list.replace_all(items);
                let _ = list.ensure_active_step();
                Ok(format!(
                    "[plan created]\n{}",
                    list.to_context_string()
                ))
            }
        }
    }
}

struct TodoReadTool {
    todo_list: Arc<Mutex<TodoList>>,
}

impl TodoReadTool {
    fn new(todo_list: Arc<Mutex<TodoList>>) -> Self {
        Self { todo_list }
    }
}

#[async_trait::async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todo_read"
    }

    fn description(&self) -> &str {
        "Read the current todo list with status of all items"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let list = self.todo_list.lock();
        Ok(list.to_context_string())
    }
}

struct TodoUpdateTool {
    todo_list: Arc<Mutex<TodoList>>,
}

impl TodoUpdateTool {
    fn new(todo_list: Arc<Mutex<TodoList>>) -> Self {
        Self { todo_list }
    }
}

#[async_trait::async_trait]
impl Tool for TodoUpdateTool {
    fn name(&self) -> &str {
        "todo_update"
    }

    fn description(&self) -> &str {
        "Update a todo item status. Args: id (string), status (pending/in_progress/completed/blocked). \
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

        let mut list = self.todo_list.lock();
        if status == TodoStatus::Completed {
            list.complete_and_advance(id)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
        } else {
            list.update_status(id, status)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if status == TodoStatus::InProgress {
                // Demote other in_progress items so there is a single cursor.
                let others: Vec<String> = list
                    .items
                    .iter()
                    .filter(|i| i.id != id && i.status == TodoStatus::InProgress)
                    .map(|i| i.id.clone())
                    .collect();
                for oid in others {
                    let _ = list.update_status(&oid, TodoStatus::Pending);
                }
            }
        }

        let desc = list.get(id).map(|i| i.description.as_str()).unwrap_or("");
        let full_list = list.to_context_string();

        Ok(format!(
            "Todo '{}': \"{}\" updated to {}\n\n{}",
            id,
            desc,
            args["status"].as_str().unwrap_or(""),
            full_list
        ))
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
