use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Replace the entire list with new items. IDs are auto-assigned (1, 2, 3, ...).
    /// **Destructive** — wipes all statuses. Prefer [`Self::merge_replace`] for replans.
    pub fn replace_all(&mut self, descriptions: Vec<String>) {
        self.items = descriptions
            .into_iter()
            .enumerate()
            .map(|(i, desc)| TodoItem::new(&(i + 1).to_string(), &desc))
            .collect();
        self.persist();
    }

    /// Merge a new plan description list with existing progress.
    ///
    /// Matching is by normalized description (case-insensitive trim). Completed /
    /// InProgress / Blocked statuses are preserved for matches. New items start
    /// Pending. Unmatched old items are dropped.
    ///
    /// After merge, if nothing is InProgress and there is a ready/pending item,
    /// the first ready item is marked InProgress.
    pub fn merge_replace(&mut self, descriptions: Vec<String>) {
        let old = std::mem::take(&mut self.items);
        let mut used_old: Vec<bool> = vec![false; old.len()];

        for (i, desc) in descriptions.into_iter().enumerate() {
            let id = (i + 1).to_string();
            let norm = normalize_desc(&desc);
            let mut item = TodoItem::new(&id, &desc);

            if let Some((idx, prev)) = old.iter().enumerate().find(|(j, o)| {
                !used_old[*j] && normalize_desc(&o.description) == norm
            }) {
                used_old[idx] = true;
                item.status = prev.status.clone();
                item.completed_at = prev.completed_at;
                item.depends_on = prev.depends_on.clone();
            }
            self.items.push(item);
        }

        // Ensure exactly one InProgress when work remains.
        let has_ip = self.items.iter().any(|i| i.status == TodoStatus::InProgress);
        if !has_ip {
            if let Some(ready_id) = self
                .ready_items()
                .first()
                .map(|i| i.id.clone())
                .or_else(|| {
                    self.items
                        .iter()
                        .find(|i| i.status == TodoStatus::Pending)
                        .map(|i| i.id.clone())
                })
            {
                let _ = self.update_status(&ready_id, TodoStatus::InProgress);
            }
        }

        self.persist();
    }

    /// Ensure there is an active (InProgress) step; return its id.
    pub fn ensure_active_step(&mut self) -> Option<String> {
        if let Some(ip) = self.in_progress().first() {
            return Some(ip.id.clone());
        }
        let next = self
            .ready_items()
            .first()
            .map(|i| i.id.clone())
            .or_else(|| {
                self.items
                    .iter()
                    .find(|i| i.status == TodoStatus::Pending)
                    .map(|i| i.id.clone())
            })?;
        let _ = self.update_status(&next, TodoStatus::InProgress);
        Some(next)
    }

    /// Mark `id` completed and promote the next ready item to InProgress.
    pub fn complete_and_advance(&mut self, id: &str) -> Result<(), String> {
        self.update_status(id, TodoStatus::Completed)?;
        let _ = self.ensure_active_step();
        Ok(())
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
            return "No todo items.".to_string();
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

fn normalize_desc(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Per-session todo lists shared across Runs of the same session.
///
/// Brain holds one store; each chat/project session gets its own
/// [`TodoList`] so plans from session A never inject into session B.
pub struct SessionTodoStore {
    by_session: Mutex<HashMap<String, Arc<Mutex<TodoList>>>>,
}

impl Default for SessionTodoStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionTodoStore {
    pub fn new() -> Self {
        Self {
            by_session: Mutex::new(HashMap::new()),
        }
    }

    fn scope_key(session_id: Option<&str>) -> String {
        session_id.unwrap_or("").to_string()
    }

    /// Return (creating if needed) the todo list for a session.
    /// `None` / empty session id uses the anonymous default scope (CLI / evals).
    pub fn for_session(&self, session_id: Option<&str>) -> Arc<Mutex<TodoList>> {
        let key = Self::scope_key(session_id);
        let mut map = self.by_session.lock();
        map.entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(TodoList::new())))
            .clone()
    }

    /// Wipe todos for a session (e.g. `/goal clear` or session delete).
    pub fn clear_session(&self, session_id: &str) {
        if let Some(list) = self.by_session.lock().get(session_id) {
            list.lock().replace_all(Vec::new());
        }
    }

    /// Drop the session entry entirely (frees the Arc if unused).
    pub fn remove_session(&self, session_id: &str) {
        self.by_session.lock().remove(session_id);
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
    fn session_todo_store_isolates_sessions() {
        let store = SessionTodoStore::new();
        {
            let a = store.for_session(Some("s1"));
            a.lock().replace_all(vec!["from s1".into()]);
        }
        {
            let b = store.for_session(Some("s2"));
            assert!(b.lock().items.is_empty());
            b.lock().replace_all(vec!["from s2".into()]);
        }
        let a_again = store.for_session(Some("s1"));
        assert_eq!(a_again.lock().items.len(), 1);
        assert_eq!(a_again.lock().items[0].description, "from s1");
        store.clear_session("s1");
        assert!(store.for_session(Some("s1")).lock().items.is_empty());
        assert_eq!(
            store.for_session(Some("s2")).lock().items[0].description,
            "from s2"
        );
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

    #[test]
    fn merge_replace_preserves_completed() {
        let mut list = TodoList::new();
        list.replace_all(vec!["Write models".into(), "Write tests".into()]);
        list.update_status("1", TodoStatus::Completed).unwrap();
        list.update_status("2", TodoStatus::InProgress).unwrap();

        list.merge_replace(vec![
            "Write models".into(),
            "Write tests".into(),
            "Write README".into(),
        ]);

        assert_eq!(list.items.len(), 3);
        assert_eq!(list.get("1").unwrap().status, TodoStatus::Completed);
        assert_eq!(list.get("2").unwrap().status, TodoStatus::InProgress);
        assert_eq!(list.get("3").unwrap().status, TodoStatus::Pending);
    }

    #[test]
    fn merge_replace_promotes_next_when_none_in_progress() {
        let mut list = TodoList::new();
        list.replace_all(vec!["A".into(), "B".into()]);
        list.update_status("1", TodoStatus::Completed).unwrap();
        // leave 2 pending
        list.merge_replace(vec!["A".into(), "B".into()]);
        assert_eq!(list.get("1").unwrap().status, TodoStatus::Completed);
        assert_eq!(list.get("2").unwrap().status, TodoStatus::InProgress);
    }
}
