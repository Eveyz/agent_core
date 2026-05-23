use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeRecord {
    pub id: String,
    pub task_id: String,
    pub path: PathBuf,
    pub branch: String,
    pub created_at: DateTime<Utc>,
    pub status: WorktreeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorktreeStatus {
    Active,
    Merged,
    Removed,
}

pub struct WorktreeManager {
    repo_root: PathBuf,
    worktrees: Vec<WorktreeRecord>,
}

impl WorktreeManager {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            worktrees: Vec::new(),
        }
    }

    pub fn create(&mut self, task_id: &str, branch_name: &str) -> Result<WorktreeRecord> {
        let worktree_id = format!("wt-{}", &task_id.replace('/', "-"));
        let worktree_path = self.repo_root.join(".worktrees").join(&worktree_id);

        std::fs::create_dir_all(&worktree_path)?;

        let output = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch_name,
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already exists") {
                std::process::Command::new("git")
                    .args([
                        "worktree",
                        "add",
                        worktree_path.to_str().unwrap(),
                        branch_name,
                    ])
                    .current_dir(&self.repo_root)
                    .output()?;
            } else {
                anyhow::bail!("git worktree add failed: {}", stderr);
            }
        }

        let record = WorktreeRecord {
            id: worktree_id,
            task_id: task_id.to_string(),
            path: worktree_path,
            branch: branch_name.to_string(),
            created_at: Utc::now(),
            status: WorktreeStatus::Active,
        };

        self.worktrees.push(record.clone());
        Ok(record)
    }

    pub fn remove(&mut self, worktree_id: &str) -> Result<()> {
        let record = self
            .worktrees
            .iter_mut()
            .find(|w| w.id == worktree_id)
            .ok_or_else(|| anyhow::anyhow!("worktree '{}' not found", worktree_id))?;

        let output = std::process::Command::new("git")
            .args(["worktree", "remove", record.path.to_str().unwrap()])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            let _ = std::fs::remove_dir_all(&record.path);
        }

        record.status = WorktreeStatus::Removed;
        Ok(())
    }

    pub fn get_by_task(&self, task_id: &str) -> Option<&WorktreeRecord> {
        self.worktrees
            .iter()
            .find(|w| w.task_id == task_id && w.status == WorktreeStatus::Active)
    }

    pub fn list_active(&self) -> Vec<&WorktreeRecord> {
        self.worktrees
            .iter()
            .filter(|w| w.status == WorktreeStatus::Active)
            .collect()
    }

    pub fn list_all(&self) -> &[WorktreeRecord] {
        &self.worktrees
    }

    pub fn task_path(&self, task_id: &str) -> Option<PathBuf> {
        self.get_by_task(task_id).map(|w| w.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_manager_new() {
        let manager = WorktreeManager::new(PathBuf::from("/tmp/test_repo"));
        assert!(manager.list_active().is_empty());
    }

    #[test]
    fn test_get_by_task_not_found() {
        let manager = WorktreeManager::new(PathBuf::from("/tmp/test_repo"));
        assert!(manager.get_by_task("nonexistent").is_none());
    }
}
