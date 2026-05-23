use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
    pub id: String,
    pub goal: String,
    pub status: TaskStatus,
    pub blocked_by: Vec<String>,
    pub assigned_to: Option<String>,
    pub result: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskRecord {
    pub fn new(id: &str, goal: &str, blocked_by: Vec<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.to_string(),
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
}

pub struct TaskBoard {
    tasks: Vec<TaskRecord>,
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
            tasks: Vec::new(),
            storage_path: None,
        }
    }

    pub fn with_storage(path: PathBuf) -> Self {
        let tasks = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| {
                    content
                        .lines()
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .collect::<Vec<TaskRecord>>()
                        .into()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            tasks,
            storage_path: Some(path),
        }
    }

    pub fn create(&mut self, id: &str, goal: &str, blocked_by: Vec<String>) {
        let task = TaskRecord::new(id, goal, blocked_by);
        self.tasks.push(task);
        self.persist();
    }

    pub fn update(
        &mut self,
        id: &str,
        status: TaskStatus,
        result: Option<String>,
    ) -> Result<()> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
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
        }

        self.persist();
        Ok(())
    }

    fn update_dependents(&mut self) {
        let completed_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id.clone())
            .collect();

        let failed_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Failed)
            .map(|t| t.id.clone())
            .collect();

        for task in &mut self.tasks {
            if task.status == TaskStatus::Pending {
                let has_failed_dep = task
                    .blocked_by
                    .iter()
                    .any(|dep| failed_ids.contains(dep));

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
    }

    pub fn get(&self, id: &str) -> Option<&TaskRecord> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn ready_tasks(&self) -> Vec<&TaskRecord> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Ready)
            .collect()
    }

    pub fn all_tasks(&self) -> &[TaskRecord] {
        &self.tasks
    }

    pub fn summary(&self) -> String {
        if self.tasks.is_empty() {
            return "Task board is empty".to_string();
        }

        let mut out = String::from("== Task Board ==\n");
        for task in &self.tasks {
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
                "{} {} {}: {}{}{}\n",
                icon, task.id, task.status, task.goal, deps, result
            ));
        }
        out.push_str(&format!(
            "== {} tasks: {} ready, {} in progress, {} completed ==\n",
            self.tasks.len(),
            self.ready_tasks().len(),
            self.tasks
                .iter()
                .filter(|t| t.status == TaskStatus::InProgress)
                .count(),
            self.tasks
                .iter()
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
            let content: String = self
                .tasks
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
        board.create("1", "First task", vec![]);
        board.create("2", "Second task", vec!["1".to_string()]);

        assert_eq!(board.all_tasks().len(), 2);
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, "1");
    }

    #[test]
    fn test_dependency_resolution() {
        let mut board = TaskBoard::new();
        board.create("1", "First", vec![]);
        board.create("2", "Second", vec!["1".to_string()]);

        board.update("1", TaskStatus::Completed, None).unwrap();

        let ready = board.ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "2");
    }

    #[test]
    fn test_assign_and_result() {
        let mut board = TaskBoard::new();
        board.create("1", "Task", vec![]);
        board.update(
            "1",
            TaskStatus::Completed,
            Some("done".to_string()),
        )
        .unwrap();

        let task = board.get("1").unwrap();
        assert_eq!(task.result.as_deref(), Some("done"));
    }

    #[test]
    fn test_failed_dep_blocks_dependents() {
        let mut board = TaskBoard::new();
        board.create("1", "First", vec![]);
        board.create("2", "Second", vec!["1".to_string()]);

        board.update("1", TaskStatus::Failed, Some("error".to_string())).unwrap();

        let task = board.get("2").unwrap();
        assert_eq!(task.status, TaskStatus::Blocked);
    }

    #[test]
    fn test_complex_dag() {
        let mut board = TaskBoard::new();
        board.create("a", "Step A", vec![]);
        board.create("b", "Step B", vec![]);
        board.create("c", "Step C", vec!["a".to_string(), "b".to_string()]);
        board.create("d", "Step D", vec!["c".to_string()]);

        // A and B are ready
        assert_eq!(board.ready_tasks().len(), 2);

        board.update("a", TaskStatus::Completed, None).unwrap();
        // C still blocked on B
        assert_eq!(board.ready_tasks().len(), 1);

        board.update("b", TaskStatus::Completed, None).unwrap();
        // Now C is ready
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, "c");

        board.update("c", TaskStatus::Completed, None).unwrap();
        // D is ready
        assert_eq!(board.ready_tasks().len(), 1);
        assert_eq!(board.ready_tasks()[0].id, "d");
    }
}
