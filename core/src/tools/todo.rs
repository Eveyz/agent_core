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

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "Create a plan by overwriting the entire todo list. Pass an array of item descriptions. Previous list is replaced. Use this for initial planning and when the plan changes."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of task descriptions. IDs are auto-assigned (1, 2, 3, ...)."
                }
            },
            "required": ["items"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let items: Vec<String> = args["items"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'items' array"))?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        if items.is_empty() {
            anyhow::bail!("'items' must not be empty");
        }

        let mut list = self.todo_list.lock();
        list.replace_all(items);
        Ok(list.to_context_string())
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
        "Update a todo item status. Args: id (string), status (pending/in_progress/completed/blocked)"
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
        list.update_status(id, status)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
            
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
