use crate::tools::{Tool, ToolRegistry};
use crate::todo::{TodoItem, TodoList, TodoStatus};
use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};

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
        "Create a todo item for planning. Args: id (string), description (string), depends_on (optional array of IDs)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Unique ID"},
                "description": {"type": "string", "description": "Task description"},
                "depends_on": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "IDs this task depends on"
                }
            },
            "required": ["id", "description"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let id = args["id"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let description = args["description"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'description'"))?;
        let depends_on: Vec<String> = args["depends_on"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let mut list = self.todo_list.lock().unwrap();
        list.add(TodoItem::new(id, description).with_depends_on(depends_on));
        Ok(format!("Todo '{}' created: {}", id, description))
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
        let list = self.todo_list.lock().unwrap();
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
        let id = args["id"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let status = match args["status"].as_str().ok_or_else(|| anyhow::anyhow!("missing 'status'"))? {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            "blocked" => TodoStatus::Blocked,
            s => anyhow::bail!("invalid status: {}", s),
        };

        let mut list = self.todo_list.lock().unwrap();
        list.update_status(id, status)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(format!("Todo '{}' updated to {}", id, args["status"].as_str().unwrap_or("")))
    }
}
