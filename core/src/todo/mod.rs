use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
            TodoStatus::Blocked => write!(f, "blocked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub description: String,
    pub status: TodoStatus,
    pub depends_on: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl TodoItem {
    pub fn new(id: &str, description: &str) -> Self {
        Self {
            id: id.to_string(),
            description: description.to_string(),
            status: TodoStatus::Pending,
            depends_on: Vec::new(),
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    pub fn with_depends_on(mut self, deps: Vec<String>) -> Self {
        self.depends_on = deps;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoList {
    pub items: Vec<TodoItem>,
    #[serde(skip)]
    storage_path: Option<PathBuf>,
}

impl Default for TodoList {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoList {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            storage_path: None,
        }
    }

    pub fn with_storage(path: PathBuf) -> Self {
        let items = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            items,
            storage_path: Some(path),
        }
    }

    pub fn add(&mut self, item: TodoItem) {
        self.items.push(item);
        self.persist();
    }

    pub fn update_status(&mut self, id: &str, status: TodoStatus) -> Result<(), String> {
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| format!("Todo item '{}' not found", id))?;

        if status == TodoStatus::Completed {
            item.completed_at = Some(Utc::now());
        }
        item.status = status;
        self.persist();
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&TodoItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn pending(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| i.status == TodoStatus::Pending)
            .collect()
    }

    pub fn in_progress(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .collect()
    }

    pub fn ready_items(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| {
                i.status == TodoStatus::Pending
                    && i.depends_on.iter().all(|dep| {
                        self.items
                            .iter()
                            .any(|d| d.id == *dep && d.status == TodoStatus::Completed)
                    })
            })
            .collect()
    }

    pub fn completion_rate(&self) -> f64 {
        if self.items.is_empty() {
            return 1.0;
        }
        let completed = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        completed as f64 / self.items.len() as f64
    }

    pub fn summary(&self) -> String {
        let total = self.items.len();
        let completed = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        let in_progress = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .count();
        let pending = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Pending)
            .count();
        let blocked = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Blocked)
            .count();

        format!(
            "Todo: {total} total | {completed} done | {in_progress} in progress | {pending} pending | {blocked} blocked"
        )
    }

    pub fn to_context_string(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }

        let mut out = String::from("== Current Plan ==\n");
        for item in &self.items {
            let status_icon = match item.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
                TodoStatus::Blocked => "[!]",
            };
            out.push_str(&format!(
                "{} {} {}: {}\n",
                status_icon, item.id, item.status, item.description
            ));
        }
        out.push_str(&format!("== {} ==\n", self.summary()));
        out
    }

    fn persist(&self) {
        if let Some(ref path) = self.storage_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&self.items) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "First task"));
        list.add(TodoItem::new("2", "Second task"));

        assert_eq!(list.items.len(), 2);
        assert!(list.get("1").is_some());
        assert!(list.get("3").is_none());
    }

    #[test]
    fn test_update_status() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "Task"));

        list.update_status("1", TodoStatus::InProgress).unwrap();
        assert_eq!(list.get("1").unwrap().status, TodoStatus::InProgress);

        list.update_status("1", TodoStatus::Completed).unwrap();
        assert!(list.get("1").unwrap().completed_at.is_some());
    }

    #[test]
    fn test_ready_items_with_dependencies() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "First"));
        list.add(TodoItem::new("2", "Second").with_depends_on(vec!["1".to_string()]));
        list.add(TodoItem::new("3", "Third").with_depends_on(vec!["2".to_string()]));

        let ready = list.ready_items();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "1");

        list.update_status("1", TodoStatus::Completed).unwrap();
        let ready = list.ready_items();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "2");
    }

    #[test]
    fn test_completion_rate() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "A"));
        list.add(TodoItem::new("2", "B"));

        assert_eq!(list.completion_rate(), 0.0);

        list.update_status("1", TodoStatus::Completed).unwrap();
        assert!((list.completion_rate() - 0.5).abs() < f64::EPSILON);
    }
}
