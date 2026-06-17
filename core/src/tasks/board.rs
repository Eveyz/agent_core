use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Ready,
    InProgress,
    Blocked,
    Completed,
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Ready => write!(f, "ready"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Blocked => write!(f, "blocked"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    id: String,
    title: String,
    goal: String,
    status: TaskStatus,
    blocked_by: Vec<String>,
    assigned_to: Option<String>,
    result: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TaskRecord {
    pub fn new(title: &str, goal: &str, blocked_by: Vec<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            goal: goal.to_string(),
            status: if blocked_by.is_empty() {
                TaskStatus::Ready
            } else {
                TaskStatus::Pending
            },
            blocked_by,
            assigned_to: None,
            result: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn status(&self) -> &TaskStatus {
        &self.status
    }

    pub fn blocked_by(&self) -> &[String] {
        &self.blocked_by
    }

    pub fn assigned_to(&self) -> Option<&str> {
        self.assigned_to.as_deref()
    }

    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn mark_in_progress(&mut self, agent_id: &str) {
        self.status = TaskStatus::InProgress;
        self.assigned_to = Some(agent_id.to_string());
        self.updated_at = Utc::now();
    }

    pub fn mark_completed(&mut self, result: Option<String>) {
        self.status = TaskStatus::Completed;
        if let Some(r) = result {
            self.result = Some(r);
        }
        self.updated_at = Utc::now();
    }

    pub fn mark_failed(&mut self, result: Option<String>) {
        self.status = TaskStatus::Failed;
        if let Some(r) = result {
            self.result = Some(r);
        }
        self.updated_at = Utc::now();
    }

    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }
}

pub struct TaskBoard {
    tasks: HashMap<String, TaskRecord>,
    storage_path: Option<PathBuf>,
}

impl Default for TaskBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBoard {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            storage_path: None,
        }
    }

    pub fn with_storage(path: PathBuf) -> Self {
        let tasks = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| {
                    let mut map = HashMap::new();
                    for line in content.lines() {
                        if let Ok(task) = serde_json::from_str::<TaskRecord>(line) {
                            map.insert(task.id.clone(), task);
                        }
                    }
                    Some(map)
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        Self {
            tasks,
            storage_path: Some(path),
        }
    }

    pub fn create(&mut self, title: &str, goal: &str, blocked_by: Vec<String>) -> String {
        let task = TaskRecord::new(title, goal, blocked_by);
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        self.persist();
        id
    }

    pub fn update(&mut self, id: &str, status: TaskStatus, result: Option<String>) -> Result<()> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("task '{}' not found", id))?;

        task.status = status.clone();
        task.updated_at = Utc::now();
        if let Some(r) = result {
            task.result = Some(r);
        }
        if status == TaskStatus::InProgress {
            task.assigned_to = Some("agent".to_string());
        }

        if status == TaskStatus::Completed || status == TaskStatus::Failed {
            self.update_dependents();
        } else {
            self.persist();
        }

        Ok(())
    }

    fn update_dependents(&mut self) {
        let completed_ids: Vec<String> = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id.clone())
            .collect();

        let failed_ids: Vec<String> = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Failed)
            .map(|t| t.id.clone())
            .collect();

        for task in self.tasks.values_mut() {
            if task.status == TaskStatus::Pending {
                let has_failed_dep = task.blocked_by.iter().any(|dep| failed_ids.contains(dep));

                if has_failed_dep {
                    task.status = TaskStatus::Blocked;
                    continue;
                }

                let all_deps_met = task
                    .blocked_by
                    .iter()
                    .all(|dep| completed_ids.contains(dep));
                if all_deps_met {
                    task.status = TaskStatus::Ready;
                }
            }
        }
        self.persist();
    }

    pub fn get(&self, id: &str) -> Option<&TaskRecord> {
        self.tasks.get(id)
    }

    pub fn ready_tasks(&self) -> Vec<&TaskRecord> {
        let mut ready: Vec<_> = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Ready)
            .collect();
        ready.sort_by_key(|t| t.created_at);
        ready
    }

    pub fn all_tasks(&self) -> Vec<&TaskRecord> {
        let mut all: Vec<_> = self.tasks.values().collect();
        all.sort_by_key(|t| t.created_at);
        all
    }

    pub fn summary(&self) -> String {
        if self.tasks.is_empty() {
            return "Task board is empty".to_string();
        }

        let mut out = String::from("== Task Board ==\n");
        let all = self.all_tasks();
        for task in &all {
            let icon = match task.status {
                TaskStatus::Pending => "[ ]",
                TaskStatus::Ready => "[>]",
                TaskStatus::InProgress => "[~]",
                TaskStatus::Blocked => "[!]",
                TaskStatus::Completed => "[x]",
                TaskStatus::Failed => "[-]",
            };
            let deps = if task.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" (blocked by: {})", task.blocked_by.join(", "))
            };
            let result = task
                .result
                .as_ref()
                .map(|r| format!(" -> {}", truncate(r, 80)))
                .unwrap_or_default();
            out.push_str(&format!(
                "{} {} [{}] ({}){}{}\n",
                icon, task.title, task.id, task.goal, deps, result
            ));
        }
        out.push_str(&format!(
            "== {} tasks: {} ready, {} in progress, {} completed ==\n",
            self.tasks.len(),
            self.ready_tasks().len(),
            self.tasks
                .values()
                .filter(|t| t.status == TaskStatus::InProgress)
                .count(),
            self.tasks
                .values()
                .filter(|t| t.status == TaskStatus::Completed)
                .count()
        ));
        out
    }

    fn persist(&self) {
        if let Some(ref path) = self.storage_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Sort by created_at to make file deterministic
            let mut all_tasks: Vec<_> = self.tasks.values().collect();
            all_tasks.sort_by_key(|t| t.created_at);
            let content: String = all_tasks
                .iter()
                .filter_map(|t| serde_json::to_string(t).ok())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = std::fs::write(path, content);
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_list() {
        let mut board = TaskBoard::new();
        let id1 = board.create("First task", "goal 1", vec![]);
        let id2 = board.create("Second task", "goal 2", vec![id1.clone()]);

        assert_eq!(board.all_tasks().len(), 2);
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, id1);
    }

    #[test]
    fn test_dependency_resolution() {
        let mut board = TaskBoard::new();
        let id1 = board.create("First", "goal 1", vec![]);
        let id2 = board.create("Second", "goal 2", vec![id1.clone()]);

        board.update(&id1, TaskStatus::Completed, None).unwrap();

        let ready = board.ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, id2);
    }

    #[test]
    fn test_assign_and_result() {
        let mut board = TaskBoard::new();
        let id1 = board.create("Task", "goal", vec![]);
        board
            .update(&id1, TaskStatus::Completed, Some("done".to_string()))
            .unwrap();

        let task = board.get(&id1).unwrap();
        assert_eq!(task.result.as_deref(), Some("done"));
    }

    #[test]
    fn test_failed_dep_blocks_dependents() {
        let mut board = TaskBoard::new();
        let id1 = board.create("First", "g1", vec![]);
        let id2 = board.create("Second", "g2", vec![id1.clone()]);

        board
            .update(&id1, TaskStatus::Failed, Some("error".to_string()))
            .unwrap();

        let task = board.get(&id2).unwrap();
        assert_eq!(task.status, TaskStatus::Blocked);
    }

    #[test]
    fn test_complex_dag() {
        let mut board = TaskBoard::new();
        let a = board.create("Step A", "a", vec![]);
        let b = board.create("Step B", "b", vec![]);
        let c = board.create("Step C", "c", vec![a.clone(), b.clone()]);
        let d = board.create("Step D", "d", vec![c.clone()]);

        // A and B are ready
        assert_eq!(board.ready_tasks().len(), 2);

        board.update(&a, TaskStatus::Completed, None).unwrap();
        // C still blocked on B
        assert_eq!(board.ready_tasks().len(), 1);

        board.update(&b, TaskStatus::Completed, None).unwrap();
        // Now C is ready
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, c);

        board.update(&c, TaskStatus::Completed, None).unwrap();
        // D is ready
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, d);
    }
}
