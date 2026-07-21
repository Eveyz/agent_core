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

use crate::types::{ImageAttachment, Message, ReasoningState};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

const REASONING_METADATA_KEY: &str = "_reasoning";
const IMAGES_METADATA_KEY: &str = "_images";

/// Embed structured reasoning into the session `metadata` JSON blob so it
/// survives SQLite round-trips without a schema migration.
fn merge_reasoning_into_metadata(
    metadata: Option<&serde_json::Value>,
    reasoning: Option<&ReasoningState>,
) -> Option<serde_json::Value> {
    match (metadata, reasoning) {
        (None, None) => None,
        (Some(meta), None) => Some(meta.clone()),
        (None, Some(r)) if r.is_empty() => None,
        (None, Some(r)) => Some(serde_json::json!({ REASONING_METADATA_KEY: r })),
        (Some(meta), Some(r)) if r.is_empty() => Some(meta.clone()),
        (Some(meta), Some(r)) => {
            let mut obj = match meta {
                serde_json::Value::Object(map) => map.clone(),
                other => {
                    let mut map = serde_json::Map::new();
                    map.insert("_value".into(), other.clone());
                    map
                }
            };
            if let Ok(v) = serde_json::to_value(r) {
                obj.insert(REASONING_METADATA_KEY.into(), v);
            }
            Some(serde_json::Value::Object(obj))
        }
    }
}

/// Embed image attachment refs into metadata for SQLite persistence.
fn merge_images_into_metadata(
    metadata: Option<&serde_json::Value>,
    images: Option<&Vec<ImageAttachment>>,
) -> Option<serde_json::Value> {
    match (metadata, images) {
        (None, None) => None,
        (Some(meta), None) => Some(meta.clone()),
        (None, Some(imgs)) if imgs.is_empty() => None,
        (None, Some(imgs)) => Some(serde_json::json!({ IMAGES_METADATA_KEY: imgs })),
        (Some(meta), Some(imgs)) if imgs.is_empty() => Some(meta.clone()),
        (Some(meta), Some(imgs)) => {
            let mut obj = match meta {
                serde_json::Value::Object(map) => map.clone(),
                other => {
                    let mut map = serde_json::Map::new();
                    map.insert("_value".into(), other.clone());
                    map
                }
            };
            if let Ok(v) = serde_json::to_value(imgs) {
                obj.insert(IMAGES_METADATA_KEY.into(), v);
            }
            Some(serde_json::Value::Object(obj))
        }
    }
}

/// Pull `_reasoning` out of metadata on load; returns (cleaned_metadata, reasoning).
fn split_reasoning_from_metadata(
    metadata: Option<serde_json::Value>,
) -> (Option<serde_json::Value>, Option<ReasoningState>) {
    let Some(mut meta) = metadata else {
        return (None, None);
    };
    let reasoning = meta
        .as_object_mut()
        .and_then(|obj| obj.remove(REASONING_METADATA_KEY))
        .and_then(|v| serde_json::from_value::<ReasoningState>(v).ok())
        .filter(|r| !r.is_empty());
    let metadata = match &meta {
        serde_json::Value::Object(map) if map.is_empty() => None,
        serde_json::Value::Object(map) if map.len() == 1 && map.contains_key("_value") => {
            map.get("_value").cloned()
        }
        other => Some(other.clone()),
    };
    (metadata, reasoning)
}

/// Pull `_images` out of metadata on load; returns (cleaned_metadata, images).
fn split_images_from_metadata(
    metadata: Option<serde_json::Value>,
) -> (Option<serde_json::Value>, Option<Vec<ImageAttachment>>) {
    let Some(mut meta) = metadata else {
        return (None, None);
    };
    let images = meta
        .as_object_mut()
        .and_then(|obj| obj.remove(IMAGES_METADATA_KEY))
        .and_then(|v| serde_json::from_value::<Vec<ImageAttachment>>(v).ok())
        .filter(|imgs| !imgs.is_empty());
    let metadata = match &meta {
        serde_json::Value::Object(map) if map.is_empty() => None,
        serde_json::Value::Object(map) if map.len() == 1 && map.contains_key("_value") => {
            map.get("_value").cloned()
        }
        other => Some(other.clone()),
    };
    (metadata, images)
}

fn merge_ephemeral_into_metadata(message: &Message) -> Option<serde_json::Value> {
    let with_reasoning =
        merge_reasoning_into_metadata(message.metadata.as_ref(), message.reasoning.as_ref());
    merge_images_into_metadata(with_reasoning.as_ref(), message.images.as_ref())
}

fn split_ephemeral_from_metadata(
    metadata: Option<serde_json::Value>,
) -> (
    Option<serde_json::Value>,
    Option<ReasoningState>,
    Option<Vec<ImageAttachment>>,
) {
    let (metadata, reasoning) = split_reasoning_from_metadata(metadata);
    let (metadata, images) = split_images_from_metadata(metadata);
    (metadata, reasoning, images)
}

/// Prepare messages for the crash-safe JSON snapshot. `Message::reasoning` is
/// omitted from normal provider serialization, so use the SQLite metadata
/// encoding here to preserve opaque blobs and signatures.
pub(crate) fn messages_for_snapshot(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            message.metadata = merge_ephemeral_into_metadata(&message);
            message.reasoning = None;
            message.images = None;
            message
        })
        .collect()
}

/// Remove only incomplete tool edges from an interrupted prompt. Complete
/// call/result pairs remain valuable execution context on resume.
fn sanitize_interrupted_history(messages: &mut Vec<Message>) {
    let result_ids: std::collections::HashSet<String> = messages
        .iter()
        .filter(|msg| msg.role == crate::types::Role::Tool)
        .filter_map(|msg| msg.tool_call_id.clone())
        .collect();

    let mut complete_call_ids = std::collections::HashSet::new();
    for msg in messages
        .iter_mut()
        .filter(|msg| msg.role == crate::types::Role::Assistant)
    {
        let Some(calls) = msg.tool_calls.as_mut() else {
            continue;
        };
        calls.retain(|call| result_ids.contains(&call.id));
        complete_call_ids.extend(calls.iter().map(|call| call.id.clone()));
        if calls.is_empty() {
            msg.tool_calls = None;
        }
    }

    messages.retain(|msg| {
        msg.role != crate::types::Role::Tool
            || msg
                .tool_call_id
                .as_ref()
                .is_some_and(|id| complete_call_ids.contains(id))
    });
}

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
    pub prompt_count: u32,
    pub cwd: String,
    pub model_used: String,
    pub tags: Vec<String>,
    pub archived: bool,
    pub parent_session_id: Option<String>,
    pub session_type: String,
    pub process_time_ms: u64,
    pub thought_time_ms: u64,
    pub mode: String,
    /// Session-scoped pinned goal (empty string in DB → None here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_goal: Option<String>,
    #[serde(default)]
    pub goal_completed: bool,
    /// Sidebar pin (distinct from pinned_goal).
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pinned_at: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single prompt (user question + assistant's complete response cycle)
/// within a session. Maps directly to a "turn".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub session_id: String,
    pub turn_index: u32,
    pub model: String,
    pub status: String,
    pub token_usage: serde_json::Value,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
    pub messages: Vec<Message>,
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

/// A full session with all messages and prompts loaded.
#[derive(Debug, Clone)]
pub struct Session {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    pub prompts: Vec<Prompt>,
}

// ── Session Manager ──────────────────────────────────────────────────

pub const META_SELECT: &str = "SELECT id, title, summary, start_time, end_time, message_count, \
    COALESCE(prompt_count, 0), cwd, model_used, tags, \
    COALESCE(archived, 0), \
    COALESCE(parent_session_id, ''), COALESCE(session_type, 'main'), \
    COALESCE(process_time_ms, 0), COALESCE(thought_time_ms, 0), \
    COALESCE(mode, 'build'), \
    COALESCE(pinned_goal, ''), COALESCE(goal_completed, 0), \
    COALESCE(pinned, 0), COALESCE(pinned_at, ''), \
    created_at, updated_at FROM sessions";

pub fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<SessionMeta> {
    let tags_str: String = row.get(9)?;
    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
    let parent: String = row.get(11)?;
    let stype: String = row.get(12)?;
    let pinned_goal: String = row.get(16)?;
    Ok(SessionMeta {
        id: row.get(0)?,
        title: row.get(1)?,
        summary: row.get(2)?,
        start_time: row.get(3)?,
        end_time: row.get(4)?,
        message_count: row.get(5)?,
        prompt_count: row.get(6)?,
        cwd: row.get(7)?,
        model_used: row.get(8)?,
        tags,
        archived: row.get::<_, i32>(10)? != 0,
        parent_session_id: if parent.is_empty() {
            None
        } else {
            Some(parent)
        },
        session_type: stype,
        process_time_ms: row.get::<_, i64>(13)? as u64,
        thought_time_ms: row.get::<_, i64>(14)? as u64,
        mode: row.get(15)?,
        pinned_goal: if pinned_goal.is_empty() {
            None
        } else {
            Some(pinned_goal)
        },
        goal_completed: row.get::<_, i32>(17)? != 0,
        pinned: row.get::<_, i32>(18)? != 0,
        pinned_at: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
    })
}

/// Manages session persistence in SQLite.
/// Uses the same Storage backend as the memory system.
pub struct SessionManager {
    storage: super::memory::storage::Storage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentLineage {
    pub session_id: String,
    pub parent_session_id: String,
    pub parent_run_id: String,
    pub parent_call_id: String,
    pub child_run_id: String,
}

impl SessionManager {
    /// Create a new SessionManager backed by the shared storage.
    pub fn new(storage: super::memory::storage::Storage) -> Self {
        Self { storage }
    }

    fn snapshot_path(session_id: &str) -> std::path::PathBuf {
        crate::paths::session_messages_snapshot_path(session_id)
    }

    /// Atomically promote the runtime's raw transcript snapshot into SQLite.
    /// Returns false when no snapshot exists.
    pub fn commit_snapshot(&self, session_id: &str) -> Result<bool> {
        let path = Self::snapshot_path(session_id);
        if !path.exists() {
            return Ok(false);
        }
        if let (Some(meta), Ok(modified)) =
            (self.get_meta(session_id)?, path.metadata()?.modified())
        {
            let snapshot_time: chrono::DateTime<Utc> = modified.into();
            if let Ok(sqlite_time) = chrono::DateTime::parse_from_rfc3339(&meta.updated_at) {
                if snapshot_time <= sqlite_time.with_timezone(&Utc) {
                    std::fs::remove_file(&path)?;
                    return Ok(false);
                }
            }
        }
        let json = std::fs::read_to_string(&path)?;
        let messages: Vec<Message> = serde_json::from_str(&json)?;
        validate_transcript(&messages)?;
        self.save_canonical_transcript(session_id, &messages)?;
        Ok(true)
    }

    /// Replace a session with the exact full (uncompressed) transcript.
    /// UI projections must never call this path. Compaction must never feed
    /// the model window into this API — only the dual-track full transcript.
    pub fn save_canonical_transcript(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        self.save_canonical_transcript_inner(session_id, messages, None)
    }

    /// Like [`save_canonical_transcript`], but forces the latest prompt group to
    /// use the pre-created lifecycle `prompt_id` (source of truth for rewind).
    pub fn save_canonical_transcript_for_prompt(
        &self,
        session_id: &str,
        messages: &[Message],
        prompt_id: &str,
    ) -> Result<()> {
        self.save_canonical_transcript_inner(session_id, messages, Some(prompt_id))
    }

    fn save_canonical_transcript_inner(
        &self,
        session_id: &str,
        messages: &[Message],
        binding_prompt_id: Option<&str>,
    ) -> Result<()> {
        validate_transcript(messages)?;
        let meta = self
            .get_meta(session_id)?
            .ok_or_else(|| anyhow::anyhow!("session '{session_id}' does not exist"))?;
        self.save_full(
            Some(session_id),
            messages,
            &meta.cwd,
            &meta.model_used,
            None,
            "main",
            None,
            binding_prompt_id,
        )?;
        let snapshot = Self::snapshot_path(session_id);
        if snapshot.exists() {
            std::fs::remove_file(snapshot)?;
        }
        Ok(())
    }

    /// Rewind the canonical transcript to immediately before a prompt. The
    /// retried user message is appended by the next Run.
    pub fn truncate_before_prompt(&self, session_id: &str, prompt_id: &str) -> Result<()> {
        let resumed = self
            .resume(session_id)?
            .ok_or_else(|| anyhow::anyhow!("session '{session_id}' does not exist"))?;
        let target = resumed
            .prompts
            .iter()
            .find(|prompt| prompt.id == prompt_id)
            .ok_or_else(|| anyhow::anyhow!("prompt '{prompt_id}' does not exist in session"))?;
        let messages: Vec<Message> = resumed
            .prompts
            .iter()
            .filter(|prompt| prompt.turn_index < target.turn_index)
            .flat_map(|prompt| prompt.messages.clone())
            .collect();
        self.save_canonical_transcript(session_id, &messages)
    }

    // ── Prompt lifecycle ────────────────────────────────────────────

    /// Create a prompt record with status='running' when a Run starts.
    /// Returns (prompt_id, turn_index).
    ///
    /// The prompt is later updated via [`finish_prompt`] when the Run
    /// completes / is cancelled / fails. On startup, any prompt still
    /// in 'running' state is repaired to 'interrupted' (zombie recovery).
    pub fn create_prompt(&self, session_id: &str, model: &str) -> Result<(String, u32)> {
        let db = self.storage.conn();
        let now = chrono::Utc::now().to_rfc3339();
        let prompt_id = uuid::Uuid::new_v4().to_string();

        // Compute the next turn_index atomically.
        let turn_index: u32 = db
            .query_row(
                "SELECT COALESCE(MAX(turn_index), -1) + 1 FROM prompts WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| row.get::<_, i64>(0).map(|v| v as u32),
            )
            .unwrap_or(0);

        db.execute(
            "INSERT INTO prompts (id, session_id, turn_index, model, status, started_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
            rusqlite::params![prompt_id, session_id, turn_index, model, now],
        )?;

        // Bump session activity so sidebar ordering reflects a new user turn immediately.
        db.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;

        Ok((prompt_id, turn_index))
    }

    /// Update a prompt's status and ended_at when a Run finishes.
    pub fn finish_prompt(
        &self,
        prompt_id: &str,
        status: &str,
        token_usage: &serde_json::Value,
    ) -> Result<()> {
        let db = self.storage.conn();
        let now = chrono::Utc::now().to_rfc3339();
        let token_str = serde_json::to_string(token_usage).unwrap_or_else(|_| "{}".to_string());
        db.execute(
            "UPDATE prompts SET status = ?1, ended_at = ?2, token_usage = ?3 WHERE id = ?4",
            rusqlite::params![status, now, token_str, prompt_id],
        )?;
        Ok(())
    }

    /// Repair zombie prompts: any prompt still in 'running' state on
    /// startup was interrupted by a crash / restart / power loss.
    ///
    /// The `ended_at` timestamp is derived from the last session message
    /// (if any were saved before the crash), falling back to the prompt's
    /// `started_at`, then to the current time.
    pub fn repair_zombie_prompts(&self) -> Result<usize> {
        let db = self.storage.conn();
        let now = chrono::Utc::now().to_rfc3339();
        let count = db.execute(
            "UPDATE prompts SET status = 'interrupted', ended_at = COALESCE( \
             (SELECT sm.created_at FROM session_messages sm \
              WHERE sm.session_id = prompts.session_id \
              ORDER BY sm.msg_index DESC LIMIT 1), \
             prompts.started_at, ?1) \
             WHERE status = 'running'",
            rusqlite::params![now],
        )?;
        Ok(count)
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
        self.save_full(
            session_id, messages, cwd, model_used, None, "main", project_id, None,
        )
    }

    /// Save a subagent session, linked to a parent session.
    /// Kept for backward-compat; prefer `save_subagent_with_messages`.
    pub fn save_subagent(
        &self,
        subagent_id: &str,
        result: &impl SubagentResultLike,
    ) -> Result<String> {
        let messages = vec![
            Message::user(&format!("Subagent task: {}", subagent_id)),
            Message::assistant(&result.summary_for_session()),
        ];
        self.save_full(
            None, // always create new
            &messages, "", "subagent", None, "subagent", None, None,
        )
    }

    /// Save a subagent session with the full message history.
    ///
    /// Unlike `save_subagent` which only stores a 2-message summary, this
    /// preserves the complete subagent conversation (tool calls, results,
    /// reasoning) so it can be inspected later.
    pub fn save_subagent_with_messages(
        &self,
        subagent_id: &str,
        messages: &[Message],
        parent_session_id: Option<&str>,
        parent_run_id: Option<&str>,
        parent_call_id: Option<&str>,
    ) -> Result<String> {
        let (session_id, prompt_id) = self.pre_allocate_subagent_session(
            subagent_id,
            parent_session_id,
            parent_run_id,
            parent_call_id,
        )?;
        self.finalize_subagent_session(&session_id, &prompt_id, subagent_id, messages)?;
        Ok(session_id)
    }

    /// Create a child session + prompt before the subagent runs so todo tools
    /// can bind to a durable `(session_id, prompt_id)` during execution.
    pub fn pre_allocate_subagent_session(
        &self,
        subagent_id: &str,
        parent_session_id: Option<&str>,
        parent_run_id: Option<&str>,
        parent_call_id: Option<&str>,
    ) -> Result<(String, String)> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let session_id = uuid::Uuid::new_v4().to_string();
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let title = format!("Subagent: {subagent_id}");
        let parent = parent_session_id.unwrap_or("");

        db.execute(
            "INSERT INTO sessions (id, title, start_time, message_count, prompt_count, cwd, model_used, parent_session_id, session_type, project_id, mode, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 0, 1, '', 'subagent', ?4, 'subagent', '', 'build', ?3, ?3)",
            rusqlite::params![session_id, title, now, parent],
        )?;
        db.execute(
            "INSERT INTO prompts (id, session_id, turn_index, model, status, started_at, created_at) \
             VALUES (?1, ?2, 0, 'subagent', 'running', ?3, ?3)",
            rusqlite::params![prompt_id, session_id, now],
        )?;
        let child_run_id = uuid::Uuid::new_v4().to_string();
        db.execute(
            "INSERT OR REPLACE INTO subagent_lineage \
             (session_id, parent_session_id, parent_run_id, parent_call_id, child_run_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                session_id,
                parent,
                parent_run_id.unwrap_or(""),
                parent_call_id.unwrap_or(""),
                child_run_id,
                now,
            ],
        )?;
        Ok((session_id, prompt_id))
    }

    /// Persist subagent messages into a pre-allocated session/prompt.
    pub fn finalize_subagent_session(
        &self,
        session_id: &str,
        prompt_id: &str,
        subagent_id: &str,
        messages: &[Message],
    ) -> Result<()> {
        let mut full_messages = vec![Message::user(&format!("Subagent task: {}", subagent_id))];
        full_messages.extend_from_slice(messages);
        self.save_full(
            Some(session_id),
            &full_messages,
            "",
            "subagent",
            None,
            "subagent",
            None,
            Some(prompt_id),
        )?;
        let _ = self.finish_prompt(prompt_id, "completed", &serde_json::json!({}));
        Ok(())
    }

    pub fn subagent_lineage(&self, session_id: &str) -> Result<Option<SubagentLineage>> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT session_id, parent_session_id, parent_run_id, parent_call_id, child_run_id \
             FROM subagent_lineage WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![session_id])?;
        Ok(rows.next()?.map(|row| SubagentLineage {
            session_id: row.get(0).unwrap_or_default(),
            parent_session_id: row.get(1).unwrap_or_default(),
            parent_run_id: row.get(2).unwrap_or_default(),
            parent_call_id: row.get(3).unwrap_or_default(),
            child_run_id: row.get(4).unwrap_or_default(),
        }))
    }

    /// Save with full control over parent, type, and project.
    ///
    /// When `binding_prompt_id` is set, the last prompt group is forced to use
    /// that id so the pre-created lifecycle row remains the rewind key.
    fn save_full(
        &self,
        session_id: Option<&str>,
        messages: &[Message],
        cwd: &str,
        model_used: &str,
        parent_session_id: Option<&str>,
        session_type: &str,
        project_id: Option<&str>,
        binding_prompt_id: Option<&str>,
    ) -> Result<String> {
        let mut db = self.storage.conn();

        // Wrap everything in a transaction so a crash between DELETE and
        // INSERT doesn't lose data. If the guard is dropped without commit,
        // SQLite auto-rolls back.
        let tx = db.transaction()?;
        let now = Utc::now().to_rfc3339();
        let msg_count = messages.len() as u32;

        let exists = match session_id {
            Some(id) => {
                let mut stmt = tx.prepare("SELECT 1 FROM sessions WHERE id = ?1")?;
                stmt.exists(rusqlite::params![id])?
            }
            None => false,
        };

        let id = if exists {
            let id = session_id.unwrap();
            // Update existing session
            tx.execute(
                "UPDATE sessions SET message_count = ?1, prompt_count = ?2, updated_at = ?3, end_time = ?4, cwd = ?5, model_used = ?6 WHERE id = ?7",
                rusqlite::params![msg_count, 0, now, now, cwd, model_used, id],
            )?;
            // Delete old messages (prompts are kept — they track lifecycle independently)
            tx.execute(
                "DELETE FROM session_messages WHERE session_id = ?1",
                rusqlite::params![id],
            )?;
            id.to_string()
        } else {
            let new_id = session_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            // Auto-generate title from first user message
            let title = Self::auto_title(messages);
            let parent = parent_session_id.unwrap_or("");
            let proj = project_id.unwrap_or("");
            tx.execute(
                "INSERT INTO sessions (id, title, start_time, message_count, prompt_count, cwd, model_used, parent_session_id, session_type, project_id, mode, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                rusqlite::params![new_id, title, now, msg_count, 0, cwd, model_used, parent, session_type, proj, "build", now],
            )?;
            new_id
        };

        // Split messages into prompts (each User message starts a new prompt)
        let prompt_groups = Self::split_into_prompts(messages);
        let prompt_count = prompt_groups.len() as u32;

        // Update prompt count
        tx.execute(
            "UPDATE sessions SET prompt_count = ?1 WHERE id = ?2",
            rusqlite::params![prompt_count, id],
        )?;

        tx.execute(
            "DELETE FROM prompts WHERE session_id = ?1 AND turn_index >= ?2",
            rusqlite::params![id, prompt_count as i64],
        )?;

        // Upsert prompts (preserve lifecycle status for existing ones)
        // and insert their messages.
        let mut global_msg_idx = 0;
        let last_prompt_idx = prompt_groups.len().saturating_sub(1);
        for (prompt_idx, group) in prompt_groups.iter().enumerate() {
            let turn_idx = prompt_idx as i64;
            let prompt_model = group
                .iter()
                .find_map(|m| m.model.as_deref())
                .unwrap_or(model_used);

            // Check if a prompt already exists at this (session_id, turn_index).
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM prompts WHERE session_id = ?1 AND turn_index = ?2",
                    rusqlite::params![id, turn_idx],
                    |row| row.get(0),
                )
                .ok();

            let prompt_id = if prompt_idx == last_prompt_idx {
                if let Some(bound) = binding_prompt_id {
                    // Canonical lifecycle id wins over turn_index guessing.
                    if let Some(ref existing) = existing_id {
                        if existing != bound {
                            tx.execute(
                                "DELETE FROM prompts WHERE id = ?1",
                                rusqlite::params![existing],
                            )?;
                        }
                    }
                    let bound_exists: bool = tx
                        .query_row(
                            "SELECT 1 FROM prompts WHERE id = ?1",
                            rusqlite::params![bound],
                            |_| Ok(true),
                        )
                        .unwrap_or(false);
                    if bound_exists {
                        tx.execute(
                            "UPDATE prompts SET session_id = ?1, turn_index = ?2, model = ?3 WHERE id = ?4",
                            rusqlite::params![id, turn_idx, prompt_model, bound],
                        )?;
                    } else {
                        tx.execute(
                            "INSERT INTO prompts (id, session_id, turn_index, model, status, started_at, created_at) \
                             VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?5)",
                            rusqlite::params![bound, id, turn_idx, prompt_model, now],
                        )?;
                    }
                    bound.to_string()
                } else if let Some(pid) = existing_id {
                    tx.execute(
                        "UPDATE prompts SET model = ?1 WHERE id = ?2",
                        rusqlite::params![prompt_model, pid],
                    )?;
                    pid
                } else {
                    let pid = uuid::Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO prompts (id, session_id, turn_index, model, status, started_at, created_at) \
                         VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?5)",
                        rusqlite::params![pid, id, turn_idx, prompt_model, now],
                    )?;
                    pid
                }
            } else if let Some(pid) = existing_id {
                // Update existing prompt (keep its lifecycle status + timestamps).
                tx.execute(
                    "UPDATE prompts SET model = ?1 WHERE id = ?2",
                    rusqlite::params![prompt_model, pid],
                )?;
                pid
            } else {
                // New prompt (legacy or fresh session without pre-created prompts).
                let pid = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO prompts (id, session_id, turn_index, model, status, started_at, created_at) \
                     VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?5)",
                    rusqlite::params![pid, id, turn_idx, prompt_model, now],
                )?;
                pid
            };

            for msg in group {
                let role = msg.role.to_string();
                let content = msg.content.as_deref().unwrap_or("");
                let tool_calls =
                    serde_json::to_string(&msg.tool_calls).unwrap_or_else(|_| "[]".to_string());
                let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
                let name = msg.name.as_deref().unwrap_or("");
                let model = msg.model.as_deref().unwrap_or("");
                let metadata = serde_json::to_string(&merge_ephemeral_into_metadata(msg))
                .unwrap_or_else(|_| "{}".to_string());

                tx.execute(
                    "INSERT OR REPLACE INTO session_messages (session_id, prompt_id, msg_index, role, content, tool_calls, tool_call_id, name, model, metadata, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![id, prompt_id, global_msg_idx, role, content, tool_calls, tool_call_id, name, model, metadata, now],
                )?;
                global_msg_idx += 1;
            }
        }

        // Commit the transaction. If we crash before this point, everything
        // is rolled back automatically by SQLite (WAL mode).
        tx.commit()?;

        Ok(id)
    }

    /// Split a flat message list into prompt groups.
    /// Each group starts with a User message and includes all subsequent
    /// messages until the next User (or end of list).
    fn split_into_prompts(messages: &[Message]) -> Vec<Vec<&Message>> {
        let mut groups: Vec<Vec<&Message>> = Vec::new();
        let mut current: Vec<&Message> = Vec::new();

        for msg in messages {
            if msg.role == crate::types::Role::User && !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            current.push(msg);
        }
        if !current.is_empty() {
            groups.push(current);
        }

        groups
    }

    // ── List ────────────────────────────────────────────────────────

    /// List all sessions, newest first.
    /// `include_archived`: if false, skips archived sessions.
    pub fn list(&self, include_archived: bool) -> Result<Vec<SessionMeta>> {
        let db = self.storage.conn();
        let sql = if include_archived {
            format!("{META_SELECT} ORDER BY updated_at DESC LIMIT 100")
        } else {
            format!("{META_SELECT} WHERE archived = 0 ORDER BY updated_at DESC LIMIT 100")
        };

        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map([], |row| row_to_meta(row))?;

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
        let sql = format!(
            "{META_SELECT} WHERE (title LIKE ?1 OR summary LIKE ?1) ORDER BY updated_at DESC LIMIT ?2"
        );
        let mut stmt = db.prepare(&sql)?;

        let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |row| {
            row_to_meta(row)
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
        let sql = format!("{META_SELECT} WHERE id = ?1");
        let mut stmt = db.prepare(&sql)?;

        let mut rows = stmt.query_map(rusqlite::params![session_id], |row| row_to_meta(row))?;

        match rows.next() {
            Some(Ok(meta)) => Ok(Some(meta)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Get the project ID associated with a session.
    pub fn get_project_id(&self, session_id: &str) -> Result<Option<String>> {
        let db = self.storage.conn();
        let mut stmt = db.prepare("SELECT project_id FROM sessions WHERE id = ?1")?;
        let mut rows =
            stmt.query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))?;
        match rows.next() {
            Some(Ok(proj_id)) => Ok(Some(proj_id)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Load a session's messages and prompts for resume.
    pub fn resume(&self, session_id: &str) -> Result<Option<Session>> {
        // A runtime snapshot is newer and more authoritative than any UI
        // projection in SQLite. Promote it before reading the session.
        self.commit_snapshot(session_id)?;
        let meta = match self.get_meta(session_id)? {
            Some(m) => m,
            None => return Ok(None),
        };

        let (messages, prompts) = {
            let db = self.storage.conn();

            // Load messages with prompt_id
            let mut stmt = db.prepare(
                "SELECT role, content, tool_calls, tool_call_id, name, model, msg_index, prompt_id, metadata \
                 FROM session_messages WHERE session_id = ?1 ORDER BY msg_index ASC",
            )?;

            let rows = stmt.query_map(rusqlite::params![session_id], |row| {
                let role_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                let tool_calls_json: String = row.get(2)?;
                let tool_call_id: String = row.get(3)?;
                let name: String = row.get(4)?;
                let model: String = row.get(5)?;
                let idx: i64 = row.get(6)?;
                let prompt_id: String = row.get(7)?;
                let metadata_json: String = row.get(8)?;

                let tool_calls: Option<Vec<crate::types::ToolCall>> =
                    serde_json::from_str(&tool_calls_json)
                        .ok()
                        .and_then(|v: serde_json::Value| {
                            if v.is_array() && !v.as_array().unwrap().is_empty() {
                                serde_json::from_value(v).ok()
                            } else {
                                None
                            }
                        });

                let metadata: Option<serde_json::Value> =
                    serde_json::from_str::<serde_json::Value>(&metadata_json)
                        .ok()
                        .filter(|value| !value.is_null());

                let (metadata, reasoning, images) = split_ephemeral_from_metadata(metadata);

                Ok((
                    idx,
                    Message {
                        role: crate::types::Role::from_str(&role_str),
                        content: if content.is_empty() {
                            None
                        } else {
                            Some(content)
                        },
                        tool_calls,
                        tool_call_id: if tool_call_id.is_empty() {
                            None
                        } else {
                            Some(tool_call_id)
                        },
                        name: if name.is_empty() { None } else { Some(name) },
                        model: if model.is_empty() { None } else { Some(model) },
                        metadata,
                        reasoning,
                        images,
                    },
                    prompt_id,
                ))
            })?;

            let mut messages_by_prompt: std::collections::HashMap<String, Vec<(i64, Message)>> =
                std::collections::HashMap::new();
            let mut legacy_messages = Vec::new();
            for row in rows {
                let (idx, msg, pid) = row?;
                if !pid.is_empty() {
                    messages_by_prompt.entry(pid).or_default().push((idx, msg));
                } else {
                    legacy_messages.push((idx, msg));
                }
            }

            // Load prompts
            let mut prompt_stmt = db.prepare(
                "SELECT id, session_id, turn_index, model, status, token_usage, \
                 started_at, ended_at, created_at FROM prompts WHERE session_id = ?1 ORDER BY turn_index ASC",
            )?;
            let prompt_rows = prompt_stmt.query_map(rusqlite::params![session_id], |row| {
                let token_str: String = row.get(5)?;
                Ok(Prompt {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    turn_index: row.get::<_, i64>(2)? as u32,
                    model: row.get(3)?,
                    status: row.get(4)?,
                    token_usage: serde_json::from_str(&token_str).unwrap_or(serde_json::json!({})),
                    started_at: row.get(6)?,
                    ended_at: row.get(7)?,
                    created_at: row.get(8)?,
                    messages: Vec::new(), // populated below
                })
            })?;
            let mut prompts: Vec<Prompt> = Vec::new();
            for row in prompt_rows {
                let mut prompt = row?;
                if let Some(mut msgs) = messages_by_prompt.remove(&prompt.id) {
                    msgs.sort_by_key(|(idx, _)| *idx);
                    let cleaned_msgs: Vec<Message> = msgs.into_iter().map(|(_, m)| m).collect();

                    prompt.messages = cleaned_msgs;
                }
                prompts.push(prompt);
            }

            // Reconstruct clean flat history list from cleaned prompts
            let mut flat_messages = Vec::new();
            for p in &prompts {
                let mut history_messages = p.messages.clone();
                // Preserve the canonical prompt projection for inspection, but
                // sanitize an interrupted tail before it is sent to a provider.
                if p.status != "completed" {
                    sanitize_interrupted_history(&mut history_messages);
                }
                flat_messages.extend(history_messages);
            }
            if !legacy_messages.is_empty() {
                legacy_messages.sort_by_key(|(idx, _)| *idx);
                flat_messages.extend(legacy_messages.into_iter().map(|(_, m)| m));
            }

            // If no prompts exist (legacy data), build them by scanning User boundaries
            if prompts.is_empty() && !flat_messages.is_empty() {
                let groups = Self::split_into_prompts(&flat_messages);
                for (idx, group) in groups.iter().enumerate() {
                    let model = group.iter().find_map(|m| m.model.as_deref()).unwrap_or("");
                    prompts.push(Prompt {
                        id: format!("legacy-prompt-{}", idx),
                        session_id: session_id.to_string(),
                        turn_index: idx as u32,
                        model: model.to_string(),
                        status: "completed".to_string(),
                        token_usage: serde_json::json!({}),
                        started_at: None,
                        ended_at: None,
                        created_at: String::new(),
                        messages: group.iter().map(|m| (*m).clone()).collect(),
                    });
                }
            }

            (flat_messages, prompts)
        }; // db lock released here

        Ok(Some(Session {
            meta,
            messages,
            prompts,
        }))
    }

    // ── Update metadata ─────────────────────────────────────────────

    /// Save timing data for a session.
    pub fn save_timing(
        &self,
        session_id: &str,
        process_time_ms: u64,
        thought_time_ms: u64,
    ) -> Result<()> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        db.execute(
            "UPDATE sessions SET process_time_ms = ?1, thought_time_ms = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![process_time_ms as i64, thought_time_ms as i64, now, session_id],
        )?;
        Ok(())
    }

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

    /// Pin or unpin a session in the sidebar.
    pub fn set_pinned(&self, session_id: &str, pinned: bool) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let pinned_at = if pinned { now.as_str() } else { "" };
        let changed = db.execute(
            "UPDATE sessions SET pinned = ?1, pinned_at = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![if pinned { 1 } else { 0 }, pinned_at, now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Pin a session-level goal (replaces any previous goal; resets completed).
    pub fn set_pinned_goal(&self, session_id: &str, goal: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET pinned_goal = ?1, goal_completed = 0, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![goal, now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Clear the session-level pinned goal.
    pub fn clear_pinned_goal(&self, session_id: &str) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET pinned_goal = '', goal_completed = 0, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;
        Ok(changed > 0)
    }

    /// Mark the session-level goal as completed (or not).
    pub fn set_goal_completed(&self, session_id: &str, completed: bool) -> Result<bool> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let changed = db.execute(
            "UPDATE sessions SET goal_completed = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![if completed { 1 } else { 0 }, now, session_id],
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

    /// Permanently delete a session and all its associated data (messages, recall memory, summaries, runs, agent history, subagent sessions, and files).
    pub fn delete(&self, session_id: &str) -> Result<bool> {
        // 1. Find child sessions. Drop the lock before recursing —
        //    parking_lot::Mutex is NOT reentrant, so holding it while
        //    calling self.delete() would deadlock.
        let child_ids: Vec<String> = {
            let db = self.storage.conn();
            let mut stmt = db.prepare("SELECT id FROM sessions WHERE parent_session_id = ?1")?;
            stmt.query_map(rusqlite::params![session_id], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect()
        }; // lock released here

        // Recurse into children (each gets its own transaction)
        for child_id in &child_ids {
            let _ = self.delete(child_id);
        }

        // 2. Delete all DB data for this session in a single transaction
        let _reflection_guard = crate::memory::reflection::reflection_persistence_guard();
        let reflected_facts: Vec<String> = {
            let db = self.storage.conn();
            let mut stmt = db.prepare(
                "SELECT f.content FROM reflection_facts f \
                 WHERE f.agverse_owned = 1 \
                   AND EXISTS (SELECT 1 FROM reflection_fact_sources s WHERE s.fact_key = f.fact_key AND s.session_id = ?1) \
                   AND NOT EXISTS (SELECT 1 FROM reflection_fact_sources s WHERE s.fact_key = f.fact_key AND s.session_id != ?1)",
            )?;
            stmt.query_map(rusqlite::params![session_id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut agverse_edit = crate::memory::reflection::remove_facts_from_agverse(
            self.storage.clone(),
            &reflected_facts,
        )?;
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO deleted_reflection_sessions(session_id, deleted_at) VALUES (?1, ?2) \
             ON CONFLICT(session_id) DO UPDATE SET deleted_at = excluded.deleted_at",
            rusqlite::params![session_id, Utc::now().to_rfc3339()],
        )?;
        tx.execute(
            "DELETE FROM reflection_fact_sources WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM archival_memory WHERE id IN \
             (SELECT f.archival_id FROM reflection_facts f \
              WHERE NOT EXISTS (SELECT 1 FROM reflection_fact_sources s WHERE s.fact_key = f.fact_key))",
            [],
        )?;
        tx.execute(
            "DELETE FROM reflection_facts WHERE NOT EXISTS \
             (SELECT 1 FROM reflection_fact_sources s WHERE s.fact_key = reflection_facts.fact_key)",
            [],
        )?;
        tx.execute(
            "DELETE FROM reflection_state WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;

        tx.execute(
            "DELETE FROM recall_memory WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM conversation_summaries WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM agent_history WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM workflow_runs WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM cronjob_runs WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM prompts WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;

        let changed = tx.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
        )?;

        if let Some(operation_id) = agverse_edit.id() {
            tx.execute(
                "UPDATE reflection_file_operations SET state = 'committed' WHERE id = ?1",
                [operation_id],
            )?;
        }

        tx.commit()?;
        agverse_edit.finish();

        // 3. Clean up associated files from the filesystem (best-effort)
        let session_dir = crate::paths::session_dir(session_id);
        if session_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&session_dir) {
                tracing::warn!(path = %session_dir.display(), error = %e, "Failed to delete session directory");
            }
        }

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

fn validate_transcript(messages: &[Message]) -> Result<()> {
    let mut declared = std::collections::HashSet::new();
    let mut completed = std::collections::HashSet::new();
    for message in messages {
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                if !declared.insert(call.id.clone()) {
                    anyhow::bail!("duplicate tool call id '{}' in transcript", call.id);
                }
            }
        }
        if message.role == crate::types::Role::Tool {
            let id = message
                .tool_call_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("tool result is missing tool_call_id"))?;
            if !declared.contains(id) {
                anyhow::bail!("tool result references unknown call id '{id}'");
            }
            if !completed.insert(id.to_string()) {
                anyhow::bail!("duplicate tool result for call id '{id}'");
            }
        }
    }
    Ok(())
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
                model: None,
                metadata: None,
                reasoning: None,
                images: None,
            },
            Message {
                role: Role::Assistant,
                content: Some("let me check".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                model: None,
                metadata: None,
                reasoning: None,
                images: None,
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
                model: None,
                metadata: None,
                reasoning: None,
                images: None,
            },
            Message {
                role: Role::Tool,
                content: Some("pub struct PermissionPolicy {...}".to_string()),
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: Some("read_file".to_string()),
                model: None,
                metadata: None,
                reasoning: None,
                images: None,
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
    fn canonical_transcript_round_trip_preserves_provider_order() {
        let (mgr, _dir) = make_manager();
        let mut messages = make_messages();
        messages.push(Message::assistant_with_tools(
            "second tool",
            vec![crate::types::ToolCall {
                id: "call_2".into(),
                call_type: "function".into(),
                function: crate::types::FunctionCall {
                    name: "grep".into(),
                    arguments: r#"{"pattern":"PermissionDecision"}"#.into(),
                },
            }],
        ));
        messages.push(Message::tool(
            "call_2".into(),
            "matches".into(),
            Some("grep".into()),
        ));
        messages.push(Message::assistant("final answer"));

        let session_id = mgr.save(None, &messages, "/tmp", "test-model").unwrap();
        mgr.save_canonical_transcript(&session_id, &messages)
            .unwrap();
        let resumed = mgr.resume(&session_id).unwrap().unwrap();

        assert_eq!(
            serde_json::to_value(&resumed.messages).unwrap(),
            serde_json::to_value(&messages).unwrap(),
        );
    }

    #[test]
    fn subagent_session_persists_full_lineage() {
        let (mgr, _dir) = make_manager();
        let parent_id = mgr
            .save(None, &[Message::user("parent")], "/tmp", "m")
            .unwrap();
        let child_id = mgr
            .save_subagent_with_messages(
                "researcher",
                &[Message::user("child task"), Message::assistant("done")],
                Some(&parent_id),
                Some("parent-run"),
                Some("call-42"),
            )
            .unwrap();
        let lineage = mgr.subagent_lineage(&child_id).unwrap().unwrap();
        assert_eq!(lineage.parent_session_id, parent_id);
        assert_eq!(lineage.parent_run_id, "parent-run");
        assert_eq!(lineage.parent_call_id, "call-42");
        assert!(!lineage.child_run_id.is_empty());
    }

    #[test]
    fn retry_rewind_truncates_canonical_messages_at_prompt_boundary() {
        let (mgr, _dir) = make_manager();
        let messages = vec![
            Message::user("one"),
            Message::assistant("answer one"),
            Message::user("two"),
            Message::assistant("answer two"),
        ];
        let session_id = mgr.save(None, &messages, "/tmp", "m").unwrap();
        let resumed = mgr.resume(&session_id).unwrap().unwrap();
        let second_prompt_id = resumed.prompts[1].id.clone();

        mgr.truncate_before_prompt(&session_id, &second_prompt_id)
            .unwrap();
        let rewound = mgr.resume(&session_id).unwrap().unwrap();
        assert_eq!(rewound.messages.len(), 2);
        assert_eq!(rewound.messages[0].content.as_deref(), Some("one"));
        assert_eq!(rewound.messages[1].content.as_deref(), Some("answer one"));
    }

    #[test]
    fn save_for_prompt_keeps_precreated_prompt_id_as_rewind_key() {
        let (mgr, _dir) = make_manager();
        let session_id = mgr
            .save(
                None,
                &[Message::user("seed"), Message::assistant("ok")],
                "/tmp",
                "m",
            )
            .unwrap();
        let (prompt_id, turn) = mgr.create_prompt(&session_id, "m").unwrap();
        assert_eq!(turn, 1);

        let messages = vec![
            Message::user("seed"),
            Message::assistant("ok"),
            Message::user("weather?"),
        ];
        mgr.save_canonical_transcript_for_prompt(&session_id, &messages, &prompt_id)
            .unwrap();

        let resumed = mgr.resume(&session_id).unwrap().unwrap();
        assert_eq!(resumed.prompts.len(), 2);
        assert_eq!(resumed.prompts[1].id, prompt_id);
        assert_eq!(
            resumed.prompts[1].messages[0].content.as_deref(),
            Some("weather?")
        );

        // Rewind by the same id the frontend would hold after runIdSet.
        mgr.truncate_before_prompt(&session_id, &prompt_id).unwrap();
        let rewound = mgr.resume(&session_id).unwrap().unwrap();
        assert_eq!(rewound.messages.len(), 2);
        assert_eq!(rewound.messages[0].content.as_deref(), Some("seed"));
        assert!(!rewound.prompts.iter().any(|p| p.id == prompt_id));
    }

    #[test]
    fn test_create_prompt_bumps_session_updated_at() {
        let (mgr, _dir) = make_manager();
        let msgs = vec![Message::user("hello")];
        let session_id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();
        let before = mgr.get_meta(&session_id).unwrap().unwrap().updated_at;

        std::thread::sleep(std::time::Duration::from_millis(5));
        let (_prompt_id, _turn) = mgr.create_prompt(&session_id, "gpt-4o").unwrap();
        let after = mgr.get_meta(&session_id).unwrap().unwrap().updated_at;

        assert_ne!(before, after);
        assert!(after >= before);
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
        assert_eq!(
            session.messages[0].content.as_deref().unwrap(),
            "帮我修复权限系统的bug"
        );
        assert_eq!(session.messages[1].role, Role::Assistant);

        // Tool call message
        let tc = session.messages[2].tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].function.name, "read_file");

        // Tool result message
        assert_eq!(session.messages[3].role, Role::Tool);
        assert_eq!(
            session.messages[3].tool_call_id.as_deref().unwrap(),
            "call_1"
        );
        assert!(
            session.messages[3]
                .content
                .as_ref()
                .unwrap()
                .contains("PermissionPolicy")
        );
    }

    /// Dual-track: persisting the full transcript (not the compacted model
    /// window) keeps older turns available on resume. A synthetic summary
    /// that only exists in the model window must not land in SQLite.
    #[test]
    fn dual_track_persist_full_not_compacted_window() {
        let (mgr, _dir) = make_manager();

        let mut full = Vec::new();
        for i in 0..6 {
            full.push(Message::user(&format!("old user {i}")));
            full.push(Message::assistant(&format!("old assistant {i}")));
        }

        let id = mgr.save(None, &full, "/tmp", "gpt").unwrap();

        // Simulate what a compacted model window looks like (must never be
        // the canonical write after dual-track).
        let compacted_window = vec![
            Message::assistant(
                "[Compressed turns 1-4]\nDecisions made:\n  • keep going\nKey facts:\n  • fact",
            ),
            Message::user("old user 5"),
            Message::assistant("old assistant 5"),
        ];

        // Correct path: rewrite from full (as Run.refresh_context_snapshot now does).
        mgr.save_canonical_transcript(&id, &full).unwrap();
        let resumed = mgr.resume(&id).unwrap().unwrap();
        assert_eq!(resumed.messages.len(), full.len());
        assert_eq!(
            resumed.messages[0].content.as_deref(),
            Some("old user 0"),
            "oldest turns must survive resume"
        );
        assert!(
            !resumed.messages.iter().any(|m| {
                m.content
                    .as_deref()
                    .is_some_and(|c| c.contains("[Compressed turns"))
            }),
            "LLM summary must not appear in the persisted full transcript"
        );

        // Contrast: if we wrongly saved the model window, old turns vanish
        // and the summary leaks into UI resume.
        mgr.save_canonical_transcript(&id, &compacted_window)
            .unwrap();
        let wrong = mgr.resume(&id).unwrap().unwrap();
        assert_eq!(wrong.messages.len(), compacted_window.len());
        assert!(
            wrong.messages[0]
                .content
                .as_deref()
                .is_some_and(|c| c.contains("[Compressed turns")),
            "sanity: compacted window really would poison resume if persisted"
        );

        // Restore full again — resume recovers all turns (the dual-track contract).
        mgr.save_canonical_transcript(&id, &full).unwrap();
        let recovered = mgr.resume(&id).unwrap().unwrap();
        assert_eq!(recovered.messages.len(), full.len());
        assert_eq!(
            recovered.messages[0].content.as_deref(),
            Some("old user 0")
        );
    }

    #[test]
    fn snapshot_messages_preserve_opaque_reasoning_via_metadata() {
        let messages = vec![
            Message::assistant("working").with_reasoning(ReasoningState {
                text: Some("plain reasoning".into()),
                encrypted_content: Some("opaque-blob".into()),
                signature: Some("provider-signature".into()),
                summary: None,
            }),
        ];

        let snapshot = messages_for_snapshot(&messages);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains(REASONING_METADATA_KEY));
        assert!(json.contains("opaque-blob"));

        let decoded: Vec<Message> = serde_json::from_str(&json).unwrap();
        let (metadata, reasoning) = split_reasoning_from_metadata(decoded[0].metadata.clone());
        assert!(metadata.is_none());
        let reasoning = reasoning.expect("reasoning must survive snapshot JSON");
        assert_eq!(reasoning.encrypted_content.as_deref(), Some("opaque-blob"));
        assert_eq!(reasoning.signature.as_deref(), Some("provider-signature"));
    }

    #[test]
    fn interrupted_resume_keeps_complete_tool_pairs_and_drops_only_dangling_calls() {
        let (mgr, _dir) = make_manager();
        let mut msgs = make_messages();
        msgs.push(Message::assistant_with_tools(
            "unfinished",
            vec![crate::types::ToolCall {
                id: "call_dangling".to_string(),
                call_type: "function".to_string(),
                function: crate::types::FunctionCall {
                    name: "shell".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        ));
        let id = mgr.save(None, &msgs, "/tmp", "gpt").unwrap();
        let first = mgr.resume(&id).unwrap().unwrap();
        let prompt_id = first.prompts[0].id.clone();
        mgr.finish_prompt(&prompt_id, "interrupted", &serde_json::json!({}))
            .unwrap();

        let resumed = mgr.resume(&id).unwrap().unwrap();
        assert!(
            resumed
                .messages
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("call_1"))
        );
        assert!(resumed.messages.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "call_1"))
        }));
        assert!(!resumed.messages.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "call_dangling"))
        }));
        assert!(!resumed.messages.iter().any(|m| {
            m.content
                .as_deref()
                .is_some_and(|content| content.contains("Execution Interrupted"))
        }));
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

        mgr.set_summary(&id, "User said hello and started a new project.")
            .unwrap();
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

        let id1 = mgr
            .save(None, &[Message::user("a")], "/tmp", "gpt")
            .unwrap();
        let id2 = mgr
            .save(None, &[Message::user("b")], "/tmp", "gpt")
            .unwrap();

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
        let id = mgr
            .save(None, &[Message::user("a")], "/tmp", "gpt")
            .unwrap();
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

        mgr.save(None, &[Message::user("Rust内存管理问题")], "/tmp", "gpt")
            .unwrap();
        mgr.save(None, &[Message::user("Python脚本优化")], "/tmp", "gpt")
            .unwrap();
        mgr.save(None, &[Message::user("Docker部署Rust服务")], "/tmp", "gpt")
            .unwrap();

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

        let id1 = mgr
            .save(None, &[Message::user("a")], "/tmp", "gpt")
            .unwrap();
        mgr.save(None, &[Message::user("b")], "/tmp", "gpt")
            .unwrap();
        mgr.archive(&id1).unwrap();

        let deleted = mgr.purge_archived().unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(mgr.list(true).unwrap().len(), 1); // only "b" remains
    }

    #[test]
    fn test_count() {
        let (mgr, _dir) = make_manager();

        mgr.save(None, &[Message::user("a")], "/tmp", "gpt")
            .unwrap();
        let id2 = mgr
            .save(None, &[Message::user("b")], "/tmp", "gpt")
            .unwrap();
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
            prompt_count: 3,
            cwd: "/home/project".to_string(),
            model_used: "gpt-4o".to_string(),
            tags: vec![],
            archived: false,
            parent_session_id: None,
            session_type: "main".to_string(),
            process_time_ms: 0,
            thought_time_ms: 0,
            mode: "build".to_string(),
            pinned_goal: None,
            goal_completed: false,
            pinned: false,
            pinned_at: String::new(),
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
            prompt_count: 0,
            cwd: "/tmp".to_string(),
            model_used: "gpt".to_string(),
            tags: vec![],
            archived: true,
            parent_session_id: None,
            session_type: "main".to_string(),
            process_time_ms: 0,
            thought_time_ms: 0,
            mode: "build".to_string(),
            pinned_goal: None,
            goal_completed: false,
            pinned: false,
            pinned_at: String::new(),
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
            prompt_count: 0,
            cwd: "/tmp".to_string(),
            model_used: "subagent".to_string(),
            tags: vec![],
            archived: false,
            parent_session_id: Some("parent123".to_string()),
            session_type: "subagent".to_string(),
            process_time_ms: 0,
            thought_time_ms: 0,
            mode: "build".to_string(),
            pinned_goal: None,
            goal_completed: false,
            pinned: false,
            pinned_at: String::new(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
        };
        let line = meta.display_line();
        assert!(
            line.contains("[sub]"),
            "subagent sessions should show [sub] tag, got: {}",
            line
        );
    }

    #[test]
    fn test_save_subagent_creates_session() {
        let (mgr, _dir) = make_manager();
        let result = crate::subagent::SubagentResult {
            role_name: "test_role".to_string(),
            subagent_id: "sub-1".to_string(),
            output: "Found 3 files matching pattern".to_string(),
            last_text: "Found 3 files matching pattern".to_string(),
            iterations_used: 2,
            success: true,
        };
        let id = mgr.save_subagent("sub-1", &result).unwrap();
        assert!(!id.is_empty());

        let meta = mgr.get_meta(&id).unwrap().unwrap();
        assert_eq!(meta.session_type, "subagent");
        assert_eq!(meta.message_count, 2);
    }

    #[test]
    fn test_cascading_delete() {
        let (mgr, _dir) = make_manager();

        // 1. Create a parent session
        let parent_id = mgr
            .save(None, &[Message::user("parent")], "/tmp", "gpt")
            .unwrap();

        // 2. Create a child session linked to parent
        let child_id = mgr
            .save_full(
                None,
                &[Message::user("child")],
                "/tmp",
                "gpt",
                Some(&parent_id),
                "subagent",
                None,
                None,
            )
            .unwrap();

        // Populate dependent database tables
        let db = mgr.storage.conn();

        // Insert parent records to satisfy foreign key constraints
        db.execute(
            "INSERT INTO agents (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "agent-1",
                "test agent",
                "2026-07-06T12:00:00Z",
                "2026-07-06T12:00:00Z"
            ],
        )
        .unwrap();

        db.execute(
            "INSERT INTO workflows (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "wf-1",
                "test workflow",
                "2026-07-06T12:00:00Z",
                "2026-07-06T12:00:00Z"
            ],
        )
        .unwrap();

        db.execute(
            "INSERT INTO cronjobs (id, name, cadence_type, cadence_value, prompt, permission_level, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["cron-1", "test cron", "daily", "12", "prompt", "standard", "2026-07-06T12:00:00Z"],
        ).unwrap();

        // recall_memory
        db.execute(
            "INSERT INTO recall_memory (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["rec-1", parent_id, "user", "some recall data", "2026-07-06T12:00:00Z"],
        ).unwrap();

        // conversation_summaries
        db.execute(
            "INSERT INTO conversation_summaries (id, session_id, summary, message_range, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["sum-1", parent_id, "some summary", "1-2", "2026-07-06T12:00:00Z"],
        ).unwrap();

        // agent_history
        db.execute(
            "INSERT INTO agent_history (id, agent_id, session_id, trigger, input, output, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["hist-1", "agent-1", parent_id, "manual", "input", "output", "2026-07-06T12:00:00Z"],
        ).unwrap();

        // workflow_runs
        db.execute(
            "INSERT INTO workflow_runs (id, workflow_id, session_id, started_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["run-1", "wf-1", parent_id, "now", "2026-07-06T12:00:00Z"],
        ).unwrap();

        // cronjob_runs
        db.execute(
            "INSERT INTO cronjob_runs (id, cronjob_id, session_id, started_at, status) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["cron-run-1", "cron-1", parent_id, "now", "success"],
        ).unwrap();

        // 3. Create dummy files on filesystem
        let session_fs = crate::paths::session_dir(&parent_id);
        std::fs::create_dir_all(&session_fs).unwrap();
        let snapshot_file = crate::paths::session_messages_snapshot_path(&parent_id);
        std::fs::write(&snapshot_file, "[]").unwrap();

        let prompt_fs = crate::paths::prompt_dir(&parent_id, "prompt-test");
        std::fs::create_dir_all(&prompt_fs).unwrap();
        let plan_file = prompt_fs.join("plan.md");
        std::fs::write(&plan_file, "plan content").unwrap();

        // Drop db lock before calling mgr methods (parking_lot::Mutex is NOT reentrant)
        drop(db);

        // Verify everything exists before deletion
        assert!(mgr.get_meta(&parent_id).unwrap().is_some());
        assert!(mgr.get_meta(&child_id).unwrap().is_some());

        let db = mgr.storage.conn();
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM recall_memory WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM conversation_summaries WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM agent_history WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM workflow_runs WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM cronjob_runs WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert!(snapshot_file.exists());
        assert!(session_fs.exists());

        // Drop db lock before calling mgr.delete (parking_lot::Mutex is NOT reentrant)
        drop(db);

        // 4. Perform Delete
        let deleted = mgr.delete(&parent_id).unwrap();
        assert!(deleted);

        // Verify everything is deleted
        assert!(mgr.get_meta(&parent_id).unwrap().is_none());
        assert!(mgr.get_meta(&child_id).unwrap().is_none()); // child deleted recursively!

        let db = mgr.storage.conn();
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM recall_memory WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM conversation_summaries WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM agent_history WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM workflow_runs WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM cronjob_runs WHERE session_id = ?1",
                rusqlite::params![parent_id],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        assert!(!snapshot_file.exists());
        assert!(!session_fs.exists());
    }

    #[test]
    fn pre_allocate_subagent_session_then_finalize() {
        let (mgr, _dir) = make_manager();
        let parent = mgr
            .save(None, &make_messages(), "/tmp", "gpt")
            .unwrap();
        let (child_sid, child_pid) = mgr
            .pre_allocate_subagent_session(
                "researcher",
                Some(&parent),
                Some("run-1"),
                Some("call-1"),
            )
            .unwrap();

        // Session + prompt exist before any messages are written.
        let db = mgr.storage.conn();
        let exists: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = ?1 AND session_type = 'subagent'",
                rusqlite::params![child_sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
        let prompt_status: String = db
            .query_row(
                "SELECT status FROM prompts WHERE id = ?1",
                rusqlite::params![child_pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(prompt_status, "running");
        drop(db);

        let store = crate::todo::SessionPlanStore::with_storage(Some(mgr.storage.clone()));
        store
            .write_plan(
                Some(&child_sid),
                vec!["Research step".into()],
                false,
                Some(&child_pid),
            )
            .unwrap();
        assert_eq!(
            store.active_source_prompt_id(Some(&child_sid)).as_deref(),
            Some(child_pid.as_str())
        );

        mgr.finalize_subagent_session(
            &child_sid,
            &child_pid,
            "researcher",
            &[Message {
                role: Role::Assistant,
                content: Some("done".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                model: None,
                metadata: None,
                reasoning: None,
                images: None,
            }],
        )
        .unwrap();

        let db = mgr.storage.conn();
        let status: String = db
            .query_row(
                "SELECT status FROM prompts WHERE id = ?1",
                rusqlite::params![child_pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "completed");
        let msg_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                rusqlite::params![child_sid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(msg_count >= 2);
    }
}
