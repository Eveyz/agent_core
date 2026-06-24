//! Project Manager — organize sessions under local directories.
//!
//! A Project represents a local directory (like a VS Code workspace).
//! Sessions are created under the active project, allowing conversations
//! to be grouped by codebase.

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Project {
    /// Create a new project from a directory path.
    pub fn from_path(path: &str) -> Self {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let now = Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            path: path.to_string(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

// ── Project Manager ──────────────────────────────────────────────────

pub struct ProjectManager {
    storage: super::memory::storage::Storage,
}

impl ProjectManager {
    pub fn new(storage: super::memory::storage::Storage) -> Self {
        Self { storage }
    }

    // ── CRUD ────────────────────────────────────────────────────────

    /// Create a new project from a directory path.
    pub fn create(&self, path: &str) -> Result<Project> {
        let db = self.storage.conn();
        
        // Check if a project with the same path already exists
        if let Ok(mut stmt) = db.prepare("SELECT id, name, path, created_at, updated_at FROM projects WHERE path = ?1") {
            if let Ok(existing) = stmt.query_row([path], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            }) {
                return Ok(existing);
            }
        }

        let project = Project::from_path(path);
        db.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![&project.id, &project.name, &project.path, &project.created_at],
        )?;
        Ok(project)
    }

    /// List all projects, newest first.
    pub fn list(&self) -> Result<Vec<Project>> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT id, name, path, created_at, updated_at FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    /// Get a single project by ID.
    pub fn get(&self, project_id: &str) -> Result<Option<Project>> {
        let db = self.storage.conn();
        let mut stmt = db
            .prepare("SELECT id, name, path, created_at, updated_at FROM projects WHERE id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(Ok(p)) => Ok(Some(p)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Update project name.
    pub fn rename(&self, project_id: &str, new_name: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![new_name, now, project_id],
        )?;
        Ok(changed > 0)
    }

    /// Delete a project and optionally its sessions.
    pub fn delete(&self, project_id: &str) -> Result<bool> {
        let db = self.storage.conn();
        // Delete associated sessions first
        db.execute(
            "DELETE FROM session_messages WHERE session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
            rusqlite::params![project_id],
        )?;
        db.execute(
            "DELETE FROM sessions WHERE project_id = ?1",
            rusqlite::params![project_id],
        )?;
        let changed = db.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        )?;
        Ok(changed > 0)
    }

    // ── Sessions under project ──────────────────────────────────────

    /// List sessions belonging to a project.
    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<crate::session::SessionMeta>> {
        let db = self.storage.conn();
        let sql = format!(
            "{} WHERE project_id = ?1 AND archived = 0 ORDER BY updated_at DESC LIMIT 100",
            crate::session::META_SELECT
        );
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            crate::session::row_to_meta(row)
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_manager() -> (ProjectManager, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        (ProjectManager::new(storage), dir)
    }

    #[test]
    fn test_create_and_list() {
        let (mgr, _dir) = make_manager();
        let p = mgr.create("/home/user/my-project").unwrap();
        assert_eq!(p.name, "my-project");
        assert_eq!(p.path, "/home/user/my-project");

        let list = mgr.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "my-project");
    }

    #[test]
    fn test_get() {
        let (mgr, _dir) = make_manager();
        let p = mgr.create("/tmp/test").unwrap();
        let fetched = mgr.get(&p.id).unwrap().unwrap();
        assert_eq!(fetched.name, "test");

        assert!(mgr.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_rename() {
        let (mgr, _dir) = make_manager();
        let p = mgr.create("/tmp/old").unwrap();
        assert!(mgr.rename(&p.id, "New Name").unwrap());
        let fetched = mgr.get(&p.id).unwrap().unwrap();
        assert_eq!(fetched.name, "New Name");
    }

    #[test]
    fn test_delete() {
        let (mgr, _dir) = make_manager();
        let p = mgr.create("/tmp/delete-me").unwrap();
        assert!(mgr.delete(&p.id).unwrap());
        assert!(mgr.get(&p.id).unwrap().is_none());
        assert_eq!(mgr.list().unwrap().len(), 0);
    }

    #[test]
    fn test_list_sessions_under_project() {
        let (proj_mgr, _dir) = make_manager();
        let p = proj_mgr.create("/tmp/proj").unwrap();

        // Create a session under this project via SessionManager
        let session_mgr = crate::session::SessionManager::new(proj_mgr.storage.clone());
        let msgs = vec![crate::types::Message::user("hello")];
        let sid = session_mgr.save(None, &msgs, "/tmp/proj", "gpt").unwrap();

        // Link session to project manually (SessionManager doesn't yet expose project_id)
        {
            let db = proj_mgr.storage.conn();
            db.execute(
                "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                rusqlite::params![&p.id, &sid],
            )
            .unwrap();
        }

        let sessions = proj_mgr.list_sessions(&p.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "hello");
    }
}
