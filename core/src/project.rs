//! Project Manager — organize sessions under local directories.
//!
//! A Project represents a local directory (like a VS Code workspace).
//! Sessions are created under the active project, allowing conversations
//! to be grouped by codebase.

use anyhow::{Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pinned_at: String,
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
        Self::with_name(name, path)
    }

    /// Create a new project with an explicit display name.
    pub fn with_name(name: impl Into<String>, path: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            path: path.to_string(),
            pinned: false,
            pinned_at: String::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Default Documents directory for new empty projects.
pub fn documents_dir() -> Result<PathBuf> {
    dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Documents")))
        .ok_or_else(|| anyhow::anyhow!("could not resolve Documents directory"))
}

/// Sanitize a project name into a safe path segment.
pub fn sanitize_project_folder_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "untitled".to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || ch.is_control() {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let cleaned = out.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() {
        "untitled".to_string()
    } else {
        cleaned
    }
}

/// Default path for a new project: Documents/<sanitized_name>.
pub fn default_project_path(name: &str) -> Result<String> {
    let folder = sanitize_project_folder_name(name);
    Ok(documents_dir()?.join(folder).to_string_lossy().to_string())
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        pinned: row.get::<_, i32>(3)? != 0,
        pinned_at: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

const PROJECT_SELECT: &str = "SELECT id, name, path, COALESCE(pinned, 0), COALESCE(pinned_at, ''), created_at, updated_at FROM projects";

// ── Project Manager ──────────────────────────────────────────────────

pub struct ProjectManager {
    storage: super::memory::storage::Storage,
}

impl ProjectManager {
    pub fn new(storage: super::memory::storage::Storage) -> Self {
        Self { storage }
    }

    // ── CRUD ────────────────────────────────────────────────────────

    fn find_by_path(&self, path: &str) -> Result<Option<Project>> {
        let db = self.storage.conn();
        let sql = format!("{PROJECT_SELECT} WHERE path = ?1 AND id != '__adhoc_chat__'");
        let mut stmt = db.prepare(&sql)?;
        match stmt.query_row([path], row_to_project) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn insert_project(&self, project: &Project) -> Result<()> {
        let db = self.storage.conn();
        db.execute(
            "INSERT INTO projects (id, name, path, pinned, pinned_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &project.id,
                &project.name,
                &project.path,
                if project.pinned { 1 } else { 0 },
                &project.pinned_at,
                &project.created_at,
                &project.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Create a new project from a directory path (import existing folder).
    pub fn create(&self, path: &str) -> Result<Project> {
        if let Some(existing) = self.find_by_path(path)? {
            return Ok(existing);
        }

        let project = Project::from_path(path);
        self.insert_project(&project)?;
        Ok(project)
    }

    /// Create an empty project folder (if needed) and register it with an explicit name.
    pub fn create_new(&self, name: &str, path: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            bail!("project name cannot be empty");
        }
        let path = path.trim();
        if path.is_empty() {
            bail!("project path cannot be empty");
        }

        if let Some(existing) = self.find_by_path(path)? {
            return Ok(existing);
        }

        let p = Path::new(path);
        if p.is_file() {
            bail!("path exists as a file: {path}");
        }
        if !p.exists() {
            std::fs::create_dir_all(p)?;
        }

        let project = Project::with_name(name, path);
        self.insert_project(&project)?;
        Ok(project)
    }

    /// List all projects, newest first.
    pub fn list(&self) -> Result<Vec<Project>> {
        let db = self.storage.conn();
        let sql = format!("{PROJECT_SELECT} ORDER BY updated_at DESC");
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_project)?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    /// Get a single project by ID.
    pub fn get(&self, project_id: &str) -> Result<Option<Project>> {
        let db = self.storage.conn();
        let sql = format!("{PROJECT_SELECT} WHERE id = ?1");
        let mut stmt = db.prepare(&sql)?;
        match stmt.query_row(rusqlite::params![project_id], row_to_project) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update project name.
    pub fn rename(&self, project_id: &str, new_name: &str) -> Result<bool> {
        if project_id == "__adhoc_chat__" {
            return Ok(false);
        }
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![new_name, now, project_id],
        )?;
        Ok(changed > 0)
    }

    /// Pin or unpin a project in the sidebar.
    pub fn set_pinned(&self, project_id: &str, pinned: bool) -> Result<bool> {
        if project_id == "__adhoc_chat__" {
            return Ok(false);
        }
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let pinned_at = if pinned { now.as_str() } else { "" };
        let changed = db.execute(
            "UPDATE projects SET pinned = ?1, pinned_at = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![if pinned { 1 } else { 0 }, pinned_at, now, project_id],
        )?;
        Ok(changed > 0)
    }

    /// Delete a project and optionally its sessions.
    pub fn delete(&self, project_id: &str) -> Result<bool> {
        if project_id == "__adhoc_chat__" {
            return Ok(false);
        }
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
    /// Only lists ordinary main chats. Child subagent transcripts and durable
    /// agent-contact conversations have dedicated navigation surfaces.
    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<crate::session::SessionMeta>> {
        let db = self.storage.conn();
        let sql = format!(
            "{} WHERE project_id = ?1 AND archived = 0 \
             AND COALESCE(session_type, 'main') = 'main' \
             ORDER BY updated_at DESC LIMIT 100",
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
        assert!(!p.pinned);

        let list = mgr.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "my-project");
    }

    #[test]
    fn test_create_new_with_name() {
        let (mgr, dir) = make_manager();
        let path = dir.path().join("fresh-proj");
        let path_str = path.to_string_lossy().to_string();
        assert!(!path.exists());
        let p = mgr.create_new("My Fresh Project", &path_str).unwrap();
        assert_eq!(p.name, "My Fresh Project");
        assert!(path.is_dir());
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
    fn test_set_pinned() {
        let (mgr, _dir) = make_manager();
        let p = mgr.create("/tmp/pin-me").unwrap();
        assert!(mgr.set_pinned(&p.id, true).unwrap());
        let fetched = mgr.get(&p.id).unwrap().unwrap();
        assert!(fetched.pinned);
        assert!(!fetched.pinned_at.is_empty());
        assert!(mgr.set_pinned(&p.id, false).unwrap());
        let fetched = mgr.get(&p.id).unwrap().unwrap();
        assert!(!fetched.pinned);
        assert!(fetched.pinned_at.is_empty());
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
    fn test_sanitize_folder_name() {
        assert_eq!(sanitize_project_folder_name("My App"), "My App");
        assert_eq!(sanitize_project_folder_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_project_folder_name("   "), "untitled");
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

    #[test]
    fn test_list_sessions_excludes_subagent_children() {
        let (proj_mgr, _dir) = make_manager();
        let p = proj_mgr.create("/tmp/proj-sa").unwrap();
        let session_mgr = crate::session::SessionManager::new(proj_mgr.storage.clone());

        let main_id = session_mgr
            .save(None, &[crate::types::Message::user("main")], "/tmp", "gpt")
            .unwrap();
        {
            let db = proj_mgr.storage.conn();
            db.execute(
                "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                rusqlite::params![&p.id, &main_id],
            )
            .unwrap();
        }
        let (child_id, _) = session_mgr
            .pre_allocate_subagent_session("sz-weather", Some(&main_id), None, None)
            .unwrap();

        let child_project = session_mgr.get_project_id(&child_id).unwrap().unwrap();
        assert_eq!(child_project, p.id);

        let sessions = proj_mgr.list_sessions(&p.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, main_id);
        assert!(!sessions.iter().any(|s| s.session_type == "subagent"));
    }
}
