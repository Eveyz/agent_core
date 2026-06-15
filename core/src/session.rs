//! Session Manager — persist and restore conversation sessions.
//!
//! Sessions are full conversation snapshots saved to SQLite.
//! Unlike Memory (which extracts knowledge), Sessions preserve the raw
//! message history so users can `/resume` exactly where they left off.
//!
//! ```text
//! Session = "what I was doing" (full message history, resumable)
//! Memory  = "what I know"     (extracted facts, searchable, cross-session)
//! ```

use crate::types::Message;
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────

/// Metadata for a saved session (no messages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub message_count: u32,
    pub cwd: String,
    pub model_used: String,
    pub tags: Vec<String>,
    pub archived: bool,
    pub parent_session_id: Option<String>,
    pub session_type: String,
    pub created_at: String,
    pub updated_at: String,
}

impl SessionMeta {
    /// Format as a single-line summary for `/sessions` listing.
    pub fn display_line(&self) -> String {
        let archived = if self.archived { "[archived] " } else { "" };
        let sub = if self.session_type == "subagent" {
            "[sub] "
        } else {
            ""
        };
        let time = &self.start_time[..self.start_time.len().min(16)];
        format!(
            "{}{}{} | {} msgs | {} | {} | {}",
            archived,
            sub,
            &self.id[..8],
            self.message_count,
            time,
            self.title,
            self.cwd
        )
    }
}

/// A full session with all messages loaded.
#[derive(Debug, Clone)]
pub struct Session {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
}

// ── Session Manager ──────────────────────────────────────────────────

/// Manages session persistence in SQLite.
/// Uses the same Storage backend as the memory system.
pub struct SessionManager {
    storage: super::memory::storage::Storage,
}

impl SessionManager {
    /// Create a new SessionManager backed by the shared storage.
    pub fn new(storage: super::memory::storage::Storage) -> Self {
        Self { storage }
    }

    // ── Save ────────────────────────────────────────────────────────

    /// Save the current conversation as a session.
    /// If `session_id` is provided, updates the existing session.
    /// Otherwise creates a new one. Returns the session ID.
    pub fn save(
        &self,
        session_id: Option<&str>,
        messages: &[Message],
        cwd: &str,
        model_used: &str,
    ) -> Result<String> {
        self.save_with_project(session_id, messages, cwd, model_used, None)
    }

    /// Save with an associated project.
    pub fn save_with_project(
        &self,
        session_id: Option<&str>,
        messages: &[Message],
        cwd: &str,
        model_used: &str,
        project_id: Option<&str>,
    ) -> Result<String> {
        self.save_full(session_id, messages, cwd, model_used, None, "main", project_id)
    }

    /// Save a subagent session, linked to a parent session.
    pub fn save_subagent(&self, subagent_id: &str, result: &impl SubagentResultLike) -> Result<String> {
        // Build minimal message log from the result
        let messages = vec![
            Message::user(&format!("Subagent task: {}", subagent_id)),
            Message::assistant(&result.summary_for_session()),
        ];
        // Create as new session — subagent_id is used as the session ID
        self.save_full(
            None,  // always create new
            &messages,
            "",
            "subagent",
            None,
            "subagent",
            None,
        )
    }

    /// Save with full control over parent, type, and project.
    fn save_full(
        &self,
        session_id: Option<&str>,
        messages: &[Message],
        cwd: &str,
        model_used: &str,
        parent_session_id: Option<&str>,
        session_type: &str,
        project_id: Option<&str>,
    ) -> Result<String> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let msg_count = messages.len() as u32;

        let id = match session_id {
            Some(id) => {
                // Update existing session
                db.execute(
                    "UPDATE sessions SET message_count = ?1, updated_at = ?2, end_time = ?3, cwd = ?4, model_used = ?5 WHERE id = ?6",
                    rusqlite::params![msg_count, now, now, cwd, model_used, id],
                )?;
                // Delete old messages and re-insert
                db.execute(
                    "DELETE FROM session_messages WHERE session_id = ?1",
                    rusqlite::params![id],
                )?;
                id.to_string()
            }
            None => {
                let new_id = uuid::Uuid::new_v4().to_string();
                // Auto-generate title from first user message
                let title = Self::auto_title(messages);
                let parent = parent_session_id.unwrap_or("");
                let proj = project_id.unwrap_or("");
                db.execute(
                    "INSERT INTO sessions (id, title, start_time, message_count, cwd, model_used, parent_session_id, session_type, project_id, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    rusqlite::params![new_id, title, now, msg_count, cwd, model_used, parent, session_type, proj, now],
                )?;
                new_id
            }
        };

        // Insert messages
        for (i, msg) in messages.iter().enumerate() {
            let role = msg.role.to_string();
            let content = msg.content.as_deref().unwrap_or("");
            let tool_calls = serde_json::to_string(&msg.tool_calls).unwrap_or_else(|_| "[]".to_string());
            let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
            let name = msg.name.as_deref().unwrap_or("");

            db.execute(
                "INSERT OR REPLACE INTO session_messages (session_id, msg_index, role, content, tool_calls, tool_call_id, name, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![id, i as i64, role, content, tool_calls, tool_call_id, name, now],
            )?;
        }

        Ok(id)
    }

    // ── List ────────────────────────────────────────────────────────

    /// List all sessions, newest first.
    /// `include_archived`: if false, skips archived sessions.
    pub fn list(&self, include_archived: bool) -> Result<Vec<SessionMeta>> {
        let db = self.storage.conn();
        let sql = if include_archived {
            "SELECT id, title, summary, start_time, end_time, message_count, cwd, model_used, tags, archived, \
             COALESCE(parent_session_id, ''), COALESCE(session_type, 'main'), created_at, updated_at \
             FROM sessions ORDER BY updated_at DESC LIMIT 100"
        } else {
            "SELECT id, title, summary, start_time, end_time, message_count, cwd, model_used, tags, archived, \
             COALESCE(parent_session_id, ''), COALESCE(session_type, 'main'), created_at, updated_at \
             FROM sessions WHERE archived = 0 ORDER BY updated_at DESC LIMIT 100"
        };

        let mut stmt = db.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            let tags_str: String = row.get(8)?;
            let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
            let parent: String = row.get(10)?;
            let stype: String = row.get(11)?;
            Ok(SessionMeta {
                id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                message_count: row.get(5)?,
                cwd: row.get(6)?,
                model_used: row.get(7)?,
                tags,
                archived: row.get::<_, i32>(9)? != 0,
                parent_session_id: if parent.is_empty() { None } else { Some(parent) },
                session_type: stype,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Search sessions by title or summary keyword.
    pub fn search(&self, keyword: &str, limit: usize) -> Result<Vec<SessionMeta>> {
        let db = self.storage.conn();
        let pattern = format!("%{}%", keyword);
        let mut stmt = db.prepare(
            "SELECT id, title, summary, start_time, end_time, message_count, cwd, model_used, tags, archived, \
             COALESCE(parent_session_id, ''), COALESCE(session_type, 'main'), created_at, updated_at \
             FROM sessions WHERE (title LIKE ?1 OR summary LIKE ?1) ORDER BY updated_at DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |row| {
            let tags_str: String = row.get(8)?;
            let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
            let parent: String = row.get(10)?;
            let stype: String = row.get(11)?;
            Ok(SessionMeta {
                id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                message_count: row.get(5)?,
                cwd: row.get(6)?,
                model_used: row.get(7)?,
                tags,
                archived: row.get::<_, i32>(9)? != 0,
                parent_session_id: if parent.is_empty() { None } else { Some(parent) },
                session_type: stype,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Get a single session's metadata (without messages).
    pub fn get_meta(&self, session_id: &str) -> Result<Option<SessionMeta>> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT id, title, summary, start_time, end_time, message_count, cwd, model_used, tags, archived, \
             COALESCE(parent_session_id, ''), COALESCE(session_type, 'main'), created_at, updated_at \
             FROM sessions WHERE id = ?1",
        )?;

        let mut rows = stmt.query_map(rusqlite::params![session_id], |row| {
            let tags_str: String = row.get(8)?;
            let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
            let parent: String = row.get(10)?;
            let stype: String = row.get(11)?;
            Ok(SessionMeta {
                id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                message_count: row.get(5)?,
                cwd: row.get(6)?,
                model_used: row.get(7)?,
                tags,
                archived: row.get::<_, i32>(9)? != 0,
                parent_session_id: if parent.is_empty() { None } else { Some(parent) },
                session_type: stype,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?;

        match rows.next() {
            Some(Ok(meta)) => Ok(Some(meta)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    // ── Resume ──────────────────────────────────────────────────────

    /// Load a session's messages for resume.
    pub fn resume(&self, session_id: &str) -> Result<Option<Session>> {
        let meta = match self.get_meta(session_id)? {
            Some(m) => m,
            None => return Ok(None),
        };

        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT role, content, tool_calls, tool_call_id, name, msg_index \
             FROM session_messages WHERE session_id = ?1 ORDER BY msg_index ASC",
        )?;

        let mut messages: Vec<(i64, Message)> = Vec::new();
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            let role_str: String = row.get(0)?;
            let content: String = row.get(1)?;
            let tool_calls_json: String = row.get(2)?;
            let tool_call_id: String = row.get(3)?;
            let name: String = row.get(4)?;
            let idx: i64 = row.get(5)?;

            let tool_calls: Option<Vec<crate::types::ToolCall>> =
                serde_json::from_str(&tool_calls_json).ok().and_then(|v: serde_json::Value| {
                    if v.is_array() && !v.as_array().unwrap().is_empty() {
                        serde_json::from_value(v).ok()
                    } else {
                        None
                    }
                });

            Ok((idx, Message {
                role: crate::types::Role::from_str(&role_str),
                content: if content.is_empty() { None } else { Some(content) },
                tool_calls,
                tool_call_id: if tool_call_id.is_empty() { None } else { Some(tool_call_id) },
                name: if name.is_empty() { None } else { Some(name) },
            }))
        })?;

        for row in rows {
            messages.push(row?);
        }
        messages.sort_by_key(|(idx, _)| *idx);
        let messages: Vec<Message> = messages.into_iter().map(|(_, m)| m).collect();

        Ok(Some(Session { meta, messages }))
    }

    // ── Update metadata ─────────────────────────────────────────────

    /// Rename a session.
    pub fn rename(&self, session_id: &str, new_title: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![new_title, now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Set the summary for a session (typically LLM-generated).
    pub fn set_summary(&self, session_id: &str, summary: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET summary = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![summary, now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Add tags to a session.
    pub fn add_tags(&self, session_id: &str, new_tags: &[&str]) -> Result<bool> {
        let db = self.storage.conn();
        let mut stmt = db.prepare("SELECT tags FROM sessions WHERE id = ?1")?;
        let tags_str: String = stmt.query_row(rusqlite::params![session_id], |row| row.get(0))?;
        let mut tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
        for tag in new_tags {
            let t = tag.to_string();
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        let updated = serde_json::to_string(&tags)?;
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET tags = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![updated, now, session_id],
        )?;
        Ok(changed > 0)
    }

    // ── Delete / Archive ────────────────────────────────────────────

    /// Archive a session (soft delete).
    pub fn archive(&self, session_id: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET archived = 1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Unarchive a session.
    pub fn unarchive(&self, session_id: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET archived = 0, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Permanently delete a session and its messages.
    pub fn delete(&self, session_id: &str) -> Result<bool> {
        let db = self.storage.conn();
        // Delete messages first (foreign key cascade should handle this, but do it explicitly)
        db.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        let changed = db.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(changed > 0)
    }

    /// Delete all archived sessions.
    pub fn purge_archived(&self) -> Result<usize> {
        let db = self.storage.conn();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE archived = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        db.execute("DELETE FROM sessions WHERE archived = 1", [])?;
        Ok(count as usize)
    }

    // ── Stats ───────────────────────────────────────────────────────

    /// Total counts.
    pub fn count(&self) -> Result<SessionCounts> {
        let db = self.storage.conn();
        let total: i64 = db
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap_or(0);
        let archived: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE archived = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(SessionCounts {
            total: total as usize,
            active: (total - archived) as usize,
            archived: archived as usize,
        })
    }

    // ── Helpers ─────────────────────────────────────────────────────

    /// Auto-generate a title from the first user message.
    fn auto_title(messages: &[Message]) -> String {
        for msg in messages {
            if msg.role == crate::types::Role::User
                && let Some(ref content) = msg.content
            {
                let trimmed = content.trim();
                // Take first line, max 80 chars
                let first_line = trimmed.lines().next().unwrap_or(trimmed);
                let title: String = first_line.chars().take(80).collect();
                if !title.is_empty() {
                    return title;
                }
            }
        }
        "Untitled".to_string()
    }
}

// ── Helper for Role deserialization ──────────────────────────────────

/// Extension trait for Role deserialization from strings.
trait RoleExt {
    fn from_str(s: &str) -> Self;
}

impl RoleExt for crate::types::Role {
    fn from_str(s: &str) -> Self {
        match s {
            "system" => crate::types::Role::System,
            "assistant" => crate::types::Role::Assistant,
            "tool" => crate::types::Role::Tool,
            _ => crate::types::Role::User,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionCounts {
    pub total: usize,
    pub active: usize,
    pub archived: usize,
}

// ── Trait for subagent result compatibility ──────────────────────────

/// Trait for subagent result types that can be saved as sessions.
pub trait SubagentResultLike {
    fn summary_for_session(&self) -> String;
}

impl SubagentResultLike for crate::subagent::SubagentResult {
    fn summary_for_session(&self) -> String {
        format!(
            "[Subagent '{}'] iterations={} success={}\n{}",
            self.subagent_id, self.iterations_used, self.success, self.output
        )
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;
    use tempfile::TempDir;

    fn make_manager() -> (SessionManager, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        (SessionManager::new(storage), dir)
    }

    fn make_messages() -> Vec<Message> {
        vec![
            Message {
                role: Role::User,
                content: Some("帮我修复权限系统的bug".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            Message {
                role: Role::Assistant,
                content: Some("让我先看看代码".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![crate::types::ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: crate::types::FunctionCall {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"src/permission/mod.rs"}"#.to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            Message {
                role: Role::Tool,
                content: Some("pub struct PermissionPolicy {...}".to_string()),
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: Some("read_file".to_string()),
            },
        ]
    }

    #[test]
    fn test_save_and_list() {
        let (mgr, _dir) = make_manager();
        let msgs = make_messages();
        let id = mgr.save(None, &msgs, "/home/project", "gpt-4o").unwrap();
        assert!(!id.is_empty());

        let sessions = mgr.list(false).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].message_count, 4);
        assert_eq!(sessions[0].cwd, "/home/project");
        assert_eq!(sessions[0].model_used, "gpt-4o");
    }

    #[test]
    fn test_auto_title_from_first_user_message() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![
            Message::user("帮我修复权限系统的bug"),
            Message::assistant("好的"),
        ];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.title, "帮我修复权限系统的bug");
    }

    #[test]
    fn test_auto_title_untitled_when_no_user_msg() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![
            Message::system("system prompt"),
            Message::assistant("hello"),
        ];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();
        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.title, "Untitled");
    }

    #[test]
    fn test_save_updates_existing() {
        let (mgr, _dir) = make_manager();

        // First save
        let msgs1 = vec![Message::user("hello")];
        let id = mgr.save(None, &msgs1, "/tmp", "gpt").unwrap();

        // Update with more messages
        let msgs2 = vec![Message::user("hello"), Message::assistant("hi there")];
        mgr.save(Some(&id), &msgs2, "/tmp", "gpt-4").unwrap();

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.model_used, "gpt-4");
    }

    #[test]
    fn test_resume_loads_all_messages() {
        let (mgr, _dir) = make_manager();
        let msgs = make_messages();
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        let session = mgr.resume(&id).unwrap().unwrap();
        assert_eq!(session.messages.len(), 4);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content.as_deref().unwrap(), "帮我修复权限系统的bug");
        assert_eq!(session.messages[1].role, Role::Assistant);

        // Tool call message
        let tc = session.messages[2].tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].function.name, "read_file");

        // Tool result message
        assert_eq!(session.messages[3].role, Role::Tool);
        assert_eq!(session.messages[3].tool_call_id.as_deref().unwrap(), "call_1");
        assert!(session.messages[3].content.as_ref().unwrap().contains("PermissionPolicy"));
    }

    #[test]
    fn test_resume_nonexistent() {
        let (mgr, _dir) = make_manager();
        let result = mgr.resume("does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_rename() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![Message::user("hello")];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        assert!(mgr.rename(&id, "My Important Session").unwrap());
        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.title, "My Important Session");
    }

    #[test]
    fn test_rename_nonexistent() {
        let (mgr, _dir) = make_manager();
        assert!(!mgr.rename("nope", "whatever").unwrap());
    }

    #[test]
    fn test_summary() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![Message::user("hello")];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        mgr.set_summary(&id, "User said hello and started a new project.").unwrap();
        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert!(meta.summary.contains("hello"));
    }

    #[test]
    fn test_tags() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![Message::user("hello")];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        mgr.add_tags(&id, &["rust", "permission"]).unwrap();
        mgr.add_tags(&id, &["rust", "refactor"]).unwrap(); // "rust" should not duplicate

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert!(meta.tags.contains(&"rust".to_string()));
        assert!(meta.tags.contains(&"permission".to_string()));
        assert!(meta.tags.contains(&"refactor".to_string()));
        assert_eq!(meta.tags.len(), 3);
    }

    #[test]
    fn test_archive_and_list() {
        let (mgr, _dir) = make_manager();

        let id1 = mgr.save(None, &[Message::user("a")], "/tmp", "gpt").unwrap();
        let id2 = mgr.save(None, &[Message::user("b")], "/tmp", "gpt").unwrap();

        mgr.archive(&id1).unwrap();

        // Default list skips archived
        let active = mgr.list(false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);

        // Include archived
        let all = mgr.list(true).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_unarchive() {
        let (mgr, _dir) = make_manager();
        let id = mgr.save(None, &[Message::user("a")], "/tmp", "gpt").unwrap();
        mgr.archive(&id).unwrap();
        assert_eq!(mgr.list(false).unwrap().len(), 0);

        mgr.unarchive(&id).unwrap();
        assert_eq!(mgr.list(false).unwrap().len(), 1);
    }

    #[test]
    fn test_delete_removes_messages() {
        let (mgr, _dir) = make_manager();
        let msgs = make_messages();
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        mgr.delete(&id).unwrap();

        assert!(mgr.get_meta(&id).unwrap().is_none());
        assert!(mgr.resume(&id).unwrap().is_none());
    }

    #[test]
    fn test_search_by_keyword() {
        let (mgr, _dir) = make_manager();

        mgr.save(None, &[Message::user("Rust内存管理问题")], "/tmp", "gpt").unwrap();
        mgr.save(None, &[Message::user("Python脚本优化")], "/tmp", "gpt").unwrap();
        mgr.save(None, &[Message::user("Docker部署Rust服务")], "/tmp", "gpt").unwrap();

        mgr.set_summary(
            &mgr.list(false).unwrap()[0].id,
            "讨论了Rust的所有权和生命周期",
        )
        .unwrap();

        let results = mgr.search("Rust", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_purge_archived() {
        let (mgr, _dir) = make_manager();

        let id1 = mgr.save(None, &[Message::user("a")], "/tmp", "gpt").unwrap();
        mgr.save(None, &[Message::user("b")], "/tmp", "gpt").unwrap();
        mgr.archive(&id1).unwrap();

        let deleted = mgr.purge_archived().unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(mgr.list(true).unwrap().len(), 1); // only "b" remains
    }

    #[test]
    fn test_count() {
        let (mgr, _dir) = make_manager();

        mgr.save(None, &[Message::user("a")], "/tmp", "gpt").unwrap();
        let id2 = mgr.save(None, &[Message::user("b")], "/tmp", "gpt").unwrap();
        mgr.archive(&id2).unwrap();

        let counts = mgr.count().unwrap();
        assert_eq!(counts.total, 2);
        assert_eq!(counts.active, 1);
        assert_eq!(counts.archived, 1);
    }

    #[test]
    fn test_save_empty_messages() {
        let (mgr, _dir) = make_manager();
        let id = mgr.save(None, &[], "/tmp", "gpt").unwrap();
        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.message_count, 0);
        assert_eq!(meta.title, "Untitled");
    }

    #[test]
    fn test_auto_title_truncates_long() {
        let (mgr, _dir) = make_manager();
        let long_msg = "a".repeat(200);
        let msgs = vec![Message::user(&long_msg)];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert!(meta.title.len() <= 80);
        assert_eq!(meta.title, "a".repeat(80));
    }

    #[test]
    fn test_auto_title_handles_multiline() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![Message::user("第一行\n第二行\n第三行")];
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.title, "第一行");
    }

    #[test]
    fn test_display_line() {
        let meta = SessionMeta {
            id: "a1b2c3d4e5f6".to_string(),
            title: "Fix permission bug".to_string(),
            summary: "".to_string(),
            start_time: "2026-06-07T12:00:00Z".to_string(),
            end_time: None,
            message_count: 42,
            cwd: "/home/project".to_string(),
            model_used: "gpt-4o".to_string(),
            tags: vec![],
            archived: false,
            parent_session_id: None,
            session_type: "main".to_string(),
            created_at: "2026-06-07T12:00:00Z".to_string(),
            updated_at: "2026-06-07T12:30:00Z".to_string(),
        };
        let line = meta.display_line();
        assert!(line.contains("a1b2c3d4"));
        assert!(line.contains("42 msgs"));
        assert!(line.contains("Fix permission bug"));
        assert!(line.contains("/home/project"));
    }

    #[test]
    fn test_display_line_archived() {
        let meta = SessionMeta {
            id: "a1b2c3d4e5f6".to_string(),
            title: "test".to_string(),
            summary: "".to_string(),
            start_time: "2026-06-07T12:00:00Z".to_string(),
            end_time: None,
            message_count: 1,
            cwd: "/tmp".to_string(),
            model_used: "gpt".to_string(),
            tags: vec![],
            archived: true,
            parent_session_id: None,
            session_type: "main".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
        };
        let line = meta.display_line();
        assert!(line.starts_with("[archived]"));
    }

    #[test]
    fn test_display_line_subagent() {
        let meta = SessionMeta {
            id: "s1s2s3s4s5s6".to_string(),
            title: "research sub-task".to_string(),
            summary: "".to_string(),
            start_time: "2026-06-07T12:00:00Z".to_string(),
            end_time: None,
            message_count: 5,
            cwd: "/tmp".to_string(),
            model_used: "subagent".to_string(),
            tags: vec![],
            archived: false,
            parent_session_id: Some("parent123".to_string()),
            session_type: "subagent".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
        };
        let line = meta.display_line();
        assert!(line.contains("[sub]"), "subagent sessions should show [sub] tag, got: {}", line);
    }

    #[test]
    fn test_save_subagent_creates_session() {
        let (mgr, _dir) = make_manager();
        let result = crate::subagent::SubagentResult {
            subagent_id: "sub-1".to_string(),
            output: "Found 3 files matching pattern".to_string(),
            iterations_used: 2,
            success: true,
        };
        let id = mgr.save_subagent("sub-1", &result).unwrap();
        assert!(!id.is_empty());

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.session_type, "subagent");
        assert_eq!(meta.message_count, 2);
    }
}
