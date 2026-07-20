//! Background reflection daemon — creates session-scoped conversation
//! summaries and separately extracts deduplicated durable facts.
//!
//! Only active in Deep mode when `reflection_model` is configured.
//! Progress is persisted in SQLite, so restarts never lose the cursor or
//! require conversation messages to be copied into an in-memory buffer.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::OptionalExtension;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::client::OpenAIClient;
use crate::memory::agverse_md::{
    self, classify_fact, normalize_section_name, scope_for_section, FactDisposition, FactScope,
};
use crate::memory::MemoryManager;
use crate::memory::storage::Storage;
use crate::types::Message;

static REFLECTION_PERSIST_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

pub(crate) fn reflection_persistence_guard() -> parking_lot::MutexGuard<'static, ()> {
    REFLECTION_PERSIST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
}

pub(crate) struct AgverseFileOperation {
    storage: Storage,
    id: Option<String>,
    path: std::path::PathBuf,
    original: Option<String>,
    changed: bool,
    finished: bool,
}

impl AgverseFileOperation {
    pub(crate) fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub(crate) fn finish(&mut self) {
        self.finished = true;
        if let Some(id) = &self.id {
            let _ = self
                .storage
                .conn()
                .execute("DELETE FROM reflection_file_operations WHERE id = ?1", [id]);
        }
    }
}

impl Drop for AgverseFileOperation {
    fn drop(&mut self) {
        if self.finished || !self.changed {
            return;
        }
        let result = match &self.original {
            Some(content) => write_atomic(&self.path, content),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        };
        if let Err(e) = result {
            tracing::error!(path = %self.path.display(), "failed to roll back agverse.md edit: {e}");
            return;
        }
        if let Some(id) = &self.id {
            let _ = self
                .storage
                .conn()
                .execute("DELETE FROM reflection_file_operations WHERE id = ?1", [id]);
        }
    }
}

pub(crate) fn remove_facts_from_agverse(
    storage: Storage,
    facts: &[String],
) -> Result<AgverseFileOperation> {
    let path = crate::paths::get_global_agverse_md_path();
    let original = match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let content = original.as_deref().unwrap_or("");
    if facts.is_empty() || original.is_none() {
        return Ok(AgverseFileOperation {
            storage,
            id: None,
            path,
            original,
            changed: false,
            finished: false,
        });
    }
    let keys: HashSet<_> = facts.iter().map(|fact| normalize_fact(fact)).collect();
    let filtered = content
        .lines()
        .filter(|line| {
            let candidate = line.trim().trim_start_matches(['-', '*']).trim();
            let candidate = candidate
                .split_once("] ")
                .map(|(_, text)| text)
                .unwrap_or(candidate);
            !keys.contains(&normalize_fact(candidate))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let updated = filtered + "\n";
    let changed = updated != content;
    if changed {
        let id = uuid::Uuid::new_v4().to_string();
        storage.conn().execute(
            "INSERT INTO reflection_file_operations \
             (id, path, original_content, updated_content, state, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'prepared', ?5)",
            rusqlite::params![
                id,
                path.to_string_lossy(),
                original,
                updated,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        if let Err(e) = write_atomic(&path, &updated) {
            let _ = storage.conn().execute(
                "DELETE FROM reflection_file_operations WHERE id = ?1",
                [&id],
            );
            return Err(e);
        }
        return Ok(AgverseFileOperation {
            storage,
            id: Some(id),
            path,
            original,
            changed: true,
            finished: false,
        });
    }
    Ok(AgverseFileOperation {
        storage,
        id: None,
        path,
        original,
        changed: false,
        finished: false,
    })
}

#[derive(Debug, Clone)]
struct PendingMessage {
    id: String,
    sequence: i64,
    role: String,
    content: String,
}

#[derive(Debug, Clone)]
struct ReflectionBatch {
    session_id: String,
    messages: Vec<PendingMessage>,
}

impl ReflectionBatch {
    fn last_sequence(&self) -> i64 {
        self.messages.last().map(|m| m.sequence).unwrap_or(0)
    }

    fn message_range(&self) -> String {
        match (self.messages.first(), self.messages.last()) {
            (Some(first), Some(last)) => format!("{}..{}", first.id, last.id),
            _ => String::new(),
        }
    }
}

#[derive(Clone)]
struct ReflectionRepository {
    storage: Storage,
}

impl ReflectionRepository {
    fn new(storage: Storage) -> Self {
        Self { storage }
    }

    fn enable(&self) -> Result<()> {
        self.storage.conn().execute(
            "UPDATE reflection_control SET enabled = 1, updated_at = ?1 WHERE singleton = 1",
            [chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn load_pending_batch(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Option<ReflectionBatch>> {
        let db = self.storage.conn();
        let cursor: i64 = db
            .query_row(
                "SELECT last_reflected_sequence FROM reflection_state WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let mut stmt = db.prepare(
            "SELECT id, reflection_sequence, role, content FROM recall_memory \
             WHERE session_id = ?1 AND role IN ('user', 'assistant') AND reflection_sequence > ?2 \
             ORDER BY reflection_sequence ASC LIMIT ?3",
        )?;
        let messages = stmt
            .query_map(rusqlite::params![session_id, cursor, limit as i64], |row| {
                Ok(PendingMessage {
                    id: row.get(0)?,
                    sequence: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(ReflectionBatch {
            session_id: session_id.to_string(),
            messages,
        }))
    }

    fn complete_batch(
        &self,
        batch: &ReflectionBatch,
        summary: &str,
        claim_token: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let range = batch.message_range();
        let last_reflected_sequence = batch.last_sequence();
        let mut db = self.storage.conn();
        let tx = db.transaction()?;
        let owns_claim: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM reflection_state \
             WHERE session_id = ?1 AND claim_token = ?2 AND status = 'running')",
            rusqlite::params![batch.session_id, claim_token],
            |row| row.get(0),
        )?;
        let deleted: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM deleted_reflection_sessions WHERE session_id = ?1)",
            [&batch.session_id],
            |row| row.get(0),
        )?;
        let existing_messages: i64 = tx.query_row(
            "SELECT COUNT(*) FROM recall_memory WHERE session_id = ?1 \
             AND reflection_sequence >= ?2 AND reflection_sequence <= ?3",
            rusqlite::params![
                batch.session_id,
                batch.messages.first().map(|m| m.sequence).unwrap_or(0),
                batch.last_sequence()
            ],
            |row| row.get(0),
        )?;
        if !owns_claim || deleted || existing_messages != batch.messages.len() as i64 {
            anyhow::bail!("reflection claim was cancelled before persistence");
        }
        tx.execute(
            "INSERT INTO conversation_summaries \
             (id, session_id, summary, message_range, created_at) \
             SELECT ?1, ?2, ?3, ?4, ?5 \
             WHERE NOT EXISTS (SELECT 1 FROM conversation_summaries WHERE session_id = ?2 AND message_range = ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), batch.session_id, summary, range, now],
        )?;
        tx.execute(
            "INSERT INTO reflection_state \
             (session_id, last_reflected_sequence, status, last_attempt_at, last_success_at, last_error, last_error_at, claim_token, updated_at) \
             VALUES (?1, ?2, 'idle', ?3, ?3, '', '', '', ?3) \
             ON CONFLICT(session_id) DO UPDATE SET \
               last_reflected_sequence = MAX(last_reflected_sequence, excluded.last_reflected_sequence), \
               status = 'idle', last_attempt_at = excluded.last_attempt_at, \
               last_success_at = excluded.last_success_at, claim_token = '', updated_at = excluded.updated_at",
            rusqlite::params![batch.session_id, last_reflected_sequence, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn pending_count(&self, session_id: &str) -> Result<usize> {
        let db = self.storage.conn();
        let count = db.query_row(
            "SELECT COUNT(*) FROM recall_memory r \
             WHERE r.session_id = ?1 AND r.role IN ('user', 'assistant') \
               AND r.reflection_sequence > COALESCE((SELECT last_reflected_sequence FROM reflection_state WHERE session_id = ?1), 0)",
            [session_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count as usize)
    }

    fn pending_sessions(&self) -> Result<Vec<String>> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT DISTINCT r.session_id FROM recall_memory r \
             LEFT JOIN reflection_state s ON s.session_id = r.session_id \
             WHERE r.role IN ('user', 'assistant') AND r.reflection_sequence > COALESCE(s.last_reflected_sequence, 0) \
             ORDER BY r.session_id",
        )?;
        Ok(stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn try_claim(&self, session_id: &str, lease_seconds: u64) -> Result<Option<String>> {
        let now = chrono::Utc::now();
        let stale_before = (now - chrono::Duration::seconds(lease_seconds as i64)).to_rfc3339();
        let now = now.to_rfc3339();
        let claim_token = uuid::Uuid::new_v4().to_string();
        let db = self.storage.conn();
        let changed = db.execute(
            "INSERT INTO reflection_state \
             (session_id, last_reflected_sequence, status, last_attempt_at, last_success_at, last_error, last_error_at, claim_token, updated_at) \
             SELECT ?1, 0, 'running', ?2, '', '', '', ?3, ?2 \
             WHERE (SELECT enabled FROM reflection_control WHERE singleton = 1) = 1 \
               AND NOT EXISTS (SELECT 1 FROM deleted_reflection_sessions WHERE session_id = ?1) \
             ON CONFLICT(session_id) DO UPDATE SET status = 'running', \
               last_attempt_at = excluded.last_attempt_at, claim_token = excluded.claim_token, updated_at = excluded.updated_at \
             WHERE reflection_state.status != 'disabled' \
               AND (reflection_state.status != 'running' OR reflection_state.last_attempt_at < ?4) \
               AND (SELECT enabled FROM reflection_control WHERE singleton = 1) = 1",
            rusqlite::params![session_id, now, claim_token, stale_before],
        )?;
        Ok((changed == 1).then_some(claim_token))
    }

    fn mark_error(&self, session_id: &str, claim_token: &str, error: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.storage.conn().execute(
            "UPDATE reflection_state SET status = 'error', last_attempt_at = ?1, \
             last_error = ?2, last_error_at = ?1, claim_token = '', updated_at = ?1 \
             WHERE session_id = ?3 AND claim_token = ?4",
            rusqlite::params![now, error, session_id, claim_token],
        )?;
        Ok(())
    }

    fn ensure_pending(&self, session_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let db = self.storage.conn();
        db.execute(
            "INSERT INTO reflection_state \
             (session_id, last_reflected_sequence, status, last_attempt_at, last_success_at, last_error, last_error_at, claim_token, updated_at) \
             SELECT ?1, 0, 'idle', '', '', '', '', '', ?2 \
             WHERE (SELECT enabled FROM reflection_control WHERE singleton = 1) = 1 \
               AND NOT EXISTS (SELECT 1 FROM deleted_reflection_sessions WHERE session_id = ?1) \
             ON CONFLICT(session_id) DO UPDATE SET \
               status = CASE WHEN status = 'disabled' THEN 'idle' ELSE status END, updated_at = excluded.updated_at \
             WHERE (SELECT enabled FROM reflection_control WHERE singleton = 1) = 1",
            rusqlite::params![session_id, now],
        )?;
        Ok(())
    }

    fn owns_claim(&self, session_id: &str, claim_token: &str) -> Result<bool> {
        Ok(self.storage.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM reflection_state s \
             WHERE s.session_id = ?1 AND s.claim_token = ?2 AND s.status = 'running' \
               AND NOT EXISTS (SELECT 1 FROM deleted_reflection_sessions d WHERE d.session_id = s.session_id))",
            rusqlite::params![session_id, claim_token],
            |row| row.get(0),
        )?)
    }

    fn session_cwd(&self, session_id: &str) -> Result<Option<String>> {
        let db = self.storage.conn();
        let cwd: Option<String> = db
            .query_row(
                "SELECT cwd FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(cwd.filter(|c| !c.trim().is_empty()))
    }

    fn store_fact(
        &self,
        fact: &ExtractedFact,
        session_id: &str,
        agverse_owned: bool,
        scope: &str,
        status: &str,
        metadata: &str,
        embedding: &[u8],
    ) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let archival_id = uuid::Uuid::new_v4().to_string();
        let fact_key = normalize_fact(&fact.text);
        let mut db = self.storage.conn();
        let tx = db.transaction()?;

        // Supersede older active facts in the same section that conflict.
        {
            let mut stmt = tx.prepare(
                "SELECT fact_key, content FROM reflection_facts \
                 WHERE section = ?1 AND status = 'active'",
            )?;
            let conflicts: Vec<(String, String)> = stmt
                .query_map([&fact.section], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter(|(_, content)| {
                    crate::memory::agverse_md::facts_conflict(content, &fact.text)
                })
                .collect();
            drop(stmt);
            for (old_key, _) in &conflicts {
                if old_key == &fact_key {
                    continue;
                }
                tx.execute(
                    "UPDATE reflection_facts SET status = 'superseded', updated_at = ?1 \
                     WHERE fact_key = ?2",
                    rusqlite::params![now, old_key],
                )?;
            }
        }

        let inserted = tx.execute(
            "INSERT OR IGNORE INTO reflection_facts \
             (fact_key, content, section, archival_id, agverse_owned, scope, status, source_session, updated_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            rusqlite::params![
                fact_key,
                fact.text,
                fact.section,
                archival_id,
                agverse_owned as i32,
                scope,
                status,
                session_id,
                now,
            ],
        )?;
        if inserted == 0 {
            // Refresh metadata on duplicate key (same normalized text).
            tx.execute(
                "UPDATE reflection_facts SET content = ?1, section = ?2, agverse_owned = ?3, \
                 scope = ?4, status = ?5, source_session = ?6, updated_at = ?7 \
                 WHERE fact_key = ?8",
                rusqlite::params![
                    fact.text,
                    fact.section,
                    agverse_owned as i32,
                    scope,
                    status,
                    session_id,
                    now,
                    fact_key,
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO archival_memory (id, content, embedding, metadata, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![archival_id, fact.text, embedding, metadata, now],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO reflection_fact_sources (fact_key, session_id, created_at) \
             VALUES (?1, ?2, ?3)",
            rusqlite::params![fact_key, session_id, now],
        )?;
        tx.commit()?;
        Ok(inserted == 1)
    }

    fn status(&self) -> Result<ReflectionStatus> {
        let db = self.storage.conn();
        let pending_messages = db.query_row(
            "SELECT COUNT(*) FROM recall_memory r \
             LEFT JOIN reflection_state s ON s.session_id = r.session_id \
             WHERE r.role IN ('user', 'assistant') AND r.reflection_sequence > COALESCE(s.last_reflected_sequence, 0)",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let summaries_created =
            db.query_row("SELECT COUNT(*) FROM conversation_summaries", [], |row| {
                row.get::<_, i64>(0)
            })? as usize;
        let durable_facts_created =
            db.query_row("SELECT COUNT(*) FROM reflection_facts", [], |row| {
                row.get::<_, i64>(0)
            })? as usize;
        let status = db.query_row(
            "SELECT CASE \
               WHEN EXISTS(SELECT 1 FROM reflection_state WHERE status = 'running') THEN 'running' \
               WHEN EXISTS(SELECT 1 FROM reflection_state WHERE status = 'error') THEN 'error' \
               WHEN EXISTS(SELECT 1 FROM reflection_state WHERE status = 'disabled') THEN 'disabled' \
               ELSE 'idle' END",
            [],
            |row| row.get::<_, String>(0),
        )?;
        let last_success_at = db.query_row(
            "SELECT COALESCE(MAX(last_success_at), '') FROM reflection_state",
            [],
            |row| row.get::<_, String>(0),
        )?;
        let last_error = db.query_row(
            "SELECT last_error FROM reflection_state WHERE last_error_at != '' ORDER BY last_error_at DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        ).unwrap_or_default();
        Ok(ReflectionStatus {
            enabled: false,
            status,
            pending_messages,
            summaries_created,
            durable_facts_created,
            last_success_at,
            last_error,
        })
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReflectionStatus {
    pub enabled: bool,
    pub status: String,
    pub pending_messages: usize,
    pub summaries_created: usize,
    pub durable_facts_created: usize,
    pub last_success_at: String,
    pub last_error: String,
}

pub fn reflection_status(storage: Storage) -> Result<ReflectionStatus> {
    ReflectionRepository::new(storage).status()
}

pub fn disable_reflection(storage: Storage) -> Result<()> {
    let _persist_guard = reflection_persistence_guard();
    let now = chrono::Utc::now().to_rfc3339();
    let mut db = storage.conn();
    let tx = db.transaction()?;
    tx.execute(
        "UPDATE reflection_control SET enabled = 0, updated_at = ?1 WHERE singleton = 1",
        [&now],
    )?;
    tx.execute(
        "UPDATE reflection_state SET status = 'disabled', claim_token = '', updated_at = ?1",
        [&now],
    )?;
    tx.commit()?;
    Ok(())
}

/// Lazy-initialized reflection daemon.
///
/// The Tokio task is NOT spawned in `spawn()` — it is deferred to `start()` or
/// the first `notify_session()` call from within an async runtime.
/// This avoids panicking when `Brain::from_config` is called outside a Tokio
/// runtime (e.g., Tauri's synchronous `setup` closure).
pub struct ReflectionDaemon {
    sender: Mutex<Option<mpsc::Sender<String>>>,
    init: Mutex<Option<DaemonInit>>,
    repository: ReflectionRepository,
}

struct DaemonInit {
    client: OpenAIClient,
    memory: Arc<Mutex<MemoryManager>>,
    trigger_count: usize,
    claim_lease_seconds: u64,
}

impl ReflectionDaemon {
    /// Create the daemon handle without spawning the task yet.
    pub fn spawn(
        client: OpenAIClient,
        memory: Arc<Mutex<MemoryManager>>,
        trigger_count: usize,
        claim_lease_seconds: u64,
    ) -> Self {
        let repository = ReflectionRepository::new(memory.lock().storage());
        let _ = repository.enable();
        if let Ok(sessions) = repository.pending_sessions() {
            for session_id in sessions {
                let _ = repository.ensure_pending(&session_id);
            }
        }
        Self {
            sender: Mutex::new(None),
            init: Mutex::new(Some(DaemonInit {
                client,
                memory,
                trigger_count,
                claim_lease_seconds,
            })),
            repository,
        }
    }

    /// Lazily spawn the background task on first use.
    fn ensure_spawned(&self) {
        let mut sender_guard = self.sender.lock();
        if sender_guard.is_some() {
            return;
        }

        let mut init_guard = self.init.lock();
        let Some(init) = init_guard.take() else {
            return;
        };

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No Tokio runtime — put init back for a later retry.
            *init_guard = Some(init);
            return;
        };

        let (tx, mut receiver) = mpsc::channel::<String>(200);
        let DaemonInit {
            client,
            memory,
            trigger_count,
            claim_lease_seconds,
        } = init;
        let repository = self.repository.clone();

        handle.spawn(async move {
            let initial = repository.pending_sessions().unwrap_or_default();
            let mut queued: HashSet<String> = initial.iter().cloned().collect();
            let mut queue: VecDeque<String> = initial.into();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await;

            loop {
                tokio::select! {
                    maybe_session = receiver.recv() => {
                        let Some(session_id) = maybe_session else { break; };
                        if queued.insert(session_id.clone()) {
                            queue.push_back(session_id.clone());
                        }
                        if repository.pending_count(&session_id).unwrap_or(0) >= trigger_count.max(1)
                            && reflect_session(&client, &memory, &repository, &session_id, trigger_count.max(1), false, claim_lease_seconds).await
                            && repository.pending_count(&session_id).unwrap_or(0) == 0
                        {
                            queued.remove(&session_id);
                            queue.retain(|queued_id| queued_id != &session_id);
                        }
                    }
                    _ = interval.tick() => {
                        if let Ok(sessions) = repository.pending_sessions() {
                            for session_id in sessions {
                                if queued.insert(session_id.clone()) {
                                    queue.push_back(session_id);
                                }
                            }
                        }
                        // Round-robin one session per minute. A failing session
                        // goes to the back instead of starving every other one.
                        if let Some(session_id) = queue.pop_front() {
                            let _ = reflect_session(&client, &memory, &repository, &session_id, trigger_count.max(1), true, claim_lease_seconds).await;
                            if repository.pending_count(&session_id).unwrap_or(0) == 0 {
                                queued.remove(&session_id);
                            } else {
                                queue.push_back(session_id);
                            }
                        }
                    }
                }
            }

            for session_id in queue {
                let _ = reflect_session(
                    &client,
                    &memory,
                    &repository,
                    &session_id,
                    trigger_count.max(1),
                    true,
                    claim_lease_seconds,
                ).await;
            }
        });

        *sender_guard = Some(tx);
    }

    /// Start the periodic worker before the next message arrives.
    pub fn start(&self) {
        self.ensure_spawned();
    }

    /// Notify the worker about a session. Conversation content remains stored
    /// only in recall_memory; the channel carries no duplicate messages.
    pub fn notify_session(&self, session_id: &str) {
        if let Err(e) = self.repository.ensure_pending(session_id) {
            tracing::warn!(session_id, "failed to register reflection work: {e}");
        }
        self.ensure_spawned();
        let guard = self.sender.lock();
        if let Some(sender) = guard.as_ref() {
            if let Err(e) = sender.try_send(session_id.to_string()) {
                tracing::warn!(session_id, "reflection notification dropped: {e}");
            }
        }
    }
}

async fn reflect_session(
    client: &OpenAIClient,
    memory: &Arc<Mutex<MemoryManager>>,
    repository: &ReflectionRepository,
    session_id: &str,
    batch_size: usize,
    force_partial: bool,
    claim_lease_seconds: u64,
) -> bool {
    let batch = match repository.load_pending_batch(session_id, batch_size) {
        Ok(Some(batch)) => batch,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(
                session_id,
                "reflection failed to load pending messages: {e}"
            );
            return false;
        }
    };
    if !batch_ready(batch.messages.len(), batch_size, force_partial) {
        return false;
    }

    let claim_token = match repository.try_claim(session_id, claim_lease_seconds) {
        Ok(Some(token)) => token,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(session_id, "reflection failed to claim batch: {e}");
            return false;
        }
    };
    match run_reflection(client, memory, repository, &batch, &claim_token).await {
        Ok(facts_created) => {
            tracing::info!(
                session_id,
                messages = batch.messages.len(),
                facts_created,
                "reflection completed"
            );
            true
        }
        Err(e) => {
            let _ = repository.mark_error(session_id, &claim_token, &e.to_string());
            tracing::warn!(session_id, "reflection failed: {e}");
            false
        }
    }
}

fn batch_ready(message_count: usize, batch_size: usize, force_partial: bool) -> bool {
    message_count > 0 && (force_partial || message_count >= batch_size)
}

/// Pre-filter: skip messages that have no durable value.
fn should_skip(role: &str, content: &str) -> bool {
    if content.trim().is_empty() {
        return true;
    }

    // Tool output: pure stdout / exit codes
    if role == "tool" {
        if content.starts_with("stdout:")
            || content.starts_with("exit code")
            || content.starts_with("Output:")
        {
            return true;
        }
    }

    // Pure error stack traces
    if (content.contains("panic:") || content.contains("Traceback")) && content.lines().count() > 10
    {
        return true;
    }

    false
}

/// A fact extracted by the reflection LLM, with a suggested section.
#[derive(Debug, Clone, serde::Deserialize)]
struct ExtractedFact {
    section: String,
    text: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ReflectionOutput {
    summary: String,
    #[serde(default)]
    facts: Vec<ExtractedFact>,
}

/// Produce a session summary, then persist durable facts separately.
async fn run_reflection(
    client: &OpenAIClient,
    memory: &Arc<Mutex<MemoryManager>>,
    repository: &ReflectionRepository,
    batch: &ReflectionBatch,
    claim_token: &str,
) -> Result<usize> {
    let conversation_text = batch
        .messages
        .iter()
        .filter(|m| !should_skip(&m.role, &m.content))
        .map(|m| format!("[{}]: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");

    let agverse_path = crate::paths::get_global_agverse_md_path();
    let existing_memory = match tokio::fs::read_to_string(&agverse_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("failed to read agverse.md for reflection"),
    };
    let prompt = build_extraction_prompt(&conversation_text, &existing_memory);
    let messages = vec![
        Message::system(&prompt),
        Message::user(
            "Return the result now as exactly one JSON object. Do not include reasoning, prose, or Markdown fences.",
        ),
    ];

    let (response, _) = client
        .chat_completion(&messages, &[])
        .await
        .context("reflection LLM call failed")?;

    let output = match parse_reflection_output(&response) {
        Ok(output) => output,
        Err(first_error) => {
            tracing::warn!(
                session_id = batch.session_id,
                error = %first_error,
                "reflection response was not valid structured JSON; requesting one format repair"
            );
            let repair_messages = vec![
                Message::system(
                    "Repair an AI response into exactly one valid JSON object with this schema: \
                     {\"summary\":\"concise conversation summary\",\"facts\":[{\"section\":\"User Preferences\",\"text\":\"durable fact\"}]}. \
                     Preserve the meaning. Use an empty facts array when needed. Output JSON only.",
                ),
                Message::user(&format!(
                    "Original reflection task:\n{prompt}\n\nResponse to repair:\n{response}"
                )),
            ];
            let (repaired, _) = client
                .chat_completion(&repair_messages, &[])
                .await
                .context("reflection JSON repair call failed")?;
            parse_reflection_output(&repaired).with_context(|| {
                format!("reflection JSON repair failed after initial error: {first_error}")
            })?
        }
    };
    persist_reflection_output(
        memory,
        repository,
        batch,
        claim_token,
        &agverse_path,
        output,
    )
    .await
}

async fn persist_reflection_output(
    memory: &Arc<Mutex<MemoryManager>>,
    repository: &ReflectionRepository,
    batch: &ReflectionBatch,
    claim_token: &str,
    agverse_path: &std::path::Path,
    output: ReflectionOutput,
) -> Result<usize> {
    if output.summary.trim().is_empty() {
        anyhow::bail!("reflection output has an empty conversation summary");
    }
    if !repository.owns_claim(&batch.session_id, claim_token)? {
        anyhow::bail!("reflection claim was cancelled before persistence");
    }

    // Normalize + quality-filter before any persistence.
    let mut seen = HashSet::new();
    let mut always_on: Vec<ExtractedFact> = Vec::new();
    let mut archival_only: Vec<ExtractedFact> = Vec::new();
    for mut fact in output.facts {
        if let Some(canonical) = normalize_section_name(&fact.section) {
            fact.section = canonical.to_string();
        }
        let key = normalize_fact(&fact.text);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        match classify_fact(&fact.section, &fact.text) {
            FactDisposition::Reject => {}
            FactDisposition::ArchivalOnly => archival_only.push(fact),
            FactDisposition::AlwaysOn => always_on.push(fact),
        }
    }

    // Serialize all derived-product writes with permanent session deletion.
    let _persist_guard = reflection_persistence_guard();
    if !repository.owns_claim(&batch.session_id, claim_token)? {
        anyhow::bail!("reflection claim was cancelled before writing memory");
    }

    let project_cwd = repository.session_cwd(&batch.session_id)?;
    let project_path = project_cwd.as_ref().map(|cwd| PathBuf::from(cwd).join("agverse.md"));

    let existing_global = read_md_or_empty(agverse_path)?;
    let existing_project = match &project_path {
        Some(p) => read_md_or_empty(p)?,
        None => String::new(),
    };

    let mut global_facts: Vec<(String, String)> = Vec::new();
    let mut project_facts: Vec<(String, String)> = Vec::new();
    for fact in &always_on {
        match scope_for_section(&fact.section) {
            FactScope::Global => {
                if !contains_fact(&existing_global, &fact.text) {
                    global_facts.push((fact.section.clone(), fact.text.clone()));
                }
            }
            FactScope::Project => {
                // Prefer project-local agverse.md when cwd is known; else global.
                if project_path.is_some() {
                    if !contains_fact(&existing_project, &fact.text) {
                        project_facts.push((fact.section.clone(), fact.text.clone()));
                    }
                } else if !contains_fact(&existing_global, &fact.text) {
                    global_facts.push((fact.section.clone(), fact.text.clone()));
                }
            }
        }
    }

    let always_on_keys: HashSet<_> = always_on
        .iter()
        .filter(|f| {
            global_facts
                .iter()
                .any(|(_, t)| normalize_fact(t) == normalize_fact(&f.text))
                || project_facts
                    .iter()
                    .any(|(_, t)| normalize_fact(t) == normalize_fact(&f.text))
        })
        .map(|f| normalize_fact(&f.text))
        .collect();

    let mut archived = 0;
    let embedding_model = memory.lock().archival().embedding_model().cloned();
    let all_for_db: Vec<(ExtractedFact, bool, &str)> = always_on
        .into_iter()
        .map(|f| {
            let scope = match scope_for_section(&f.section) {
                FactScope::Global => "global",
                FactScope::Project => {
                    if project_path.is_some() {
                        "project"
                    } else {
                        "global"
                    }
                }
            };
            let owned = always_on_keys.contains(&normalize_fact(&f.text));
            (f, owned, scope)
        })
        .chain(archival_only.into_iter().map(|f| (f, false, "archival")))
        .collect();

    for (fact, agverse_owned, scope) in &all_for_db {
        let metadata = serde_json::json!({
            "source": "reflection",
            "section": &fact.section,
            "scope": scope,
            "agverse_owned": agverse_owned,
        })
        .to_string();
        let embedding = if let Some(model) = &embedding_model {
            crate::memory::embedding::embedding_to_bytes(&model.embed_single(&fact.text)?)
        } else {
            Vec::new()
        };
        if repository.store_fact(
            fact,
            &batch.session_id,
            *agverse_owned,
            scope,
            "active",
            &metadata,
            &embedding,
        )? {
            archived += 1;
        }
    }

    // Ownership is durable before the file mutation. If writing the file or
    // advancing the cursor fails, the next retry can safely finish the same
    // idempotent operation without losing provenance.
    if !global_facts.is_empty() {
        let mut updated = agverse_md::append_facts_to_sections(&existing_global, &global_facts);
        let (maintained, report) = agverse_md::maintain_agverse_content(&updated);
        updated = maintained;
        if report.pending_expired > 0 || report.trimmed_bullets > 0 {
            tracing::info!(
                pending_expired = report.pending_expired,
                trimmed = report.trimmed_bullets,
                "agverse.md maintenance applied after reflection"
            );
        }
        write_atomic(agverse_path, &updated)?;
    } else {
        // Still run TTL/capacity maintenance even when no new always-on facts.
        let _ = agverse_md::maintain_agverse_file(agverse_path);
    }

    if !project_facts.is_empty() {
        if let Some(ref path) = project_path {
            let updated = agverse_md::append_facts_to_sections(&existing_project, &project_facts);
            write_atomic(path, &updated)?;
        }
    }

    repository.complete_batch(batch, output.summary.trim(), claim_token)?;
    Ok(archived)
}

fn read_md_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("md.reflection.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn parse_reflection_output(response: &str) -> Result<ReflectionOutput> {
    let trimmed = response.trim();
    let mut last_error = None;
    let output = std::iter::once(trimmed)
        .chain(json_object_candidates(trimmed))
        .find_map(
            |candidate| match serde_json::from_str::<ReflectionOutput>(candidate) {
                Ok(output) => Some(output),
                Err(error) => {
                    last_error = Some(error);
                    None
                }
            },
        )
        .with_context(|| {
            let shape = format!(
                "len={}, fenced={}, reasoning={}, open_braces={}, close_braces={}",
                response.len(),
                response.contains("```"),
                response.contains("<think>") || response.contains("reasoning"),
                response.matches('{').count(),
                response.matches('}').count(),
            );
            match last_error {
                Some(error) => format!("reflection model returned invalid JSON ({shape}): {error}"),
                None => format!("reflection model returned no JSON object ({shape})"),
            }
        })?;
    if output.summary.trim().is_empty() {
        anyhow::bail!("reflection model returned an empty conversation summary");
    }
    Ok(output)
}

fn json_object_candidates(input: &str) -> impl Iterator<Item = &str> {
    input
        .char_indices()
        .filter(|(_, ch)| *ch == '{')
        .filter_map(|(start, _)| balanced_json_object(input, start))
}

fn balanced_json_object(input: &str, start: usize) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in input[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return Some(&input[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn normalize_fact(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn contains_fact(existing_memory: &str, fact: &str) -> bool {
    let needle = normalize_fact(fact);
    !needle.is_empty()
        && existing_memory.lines().any(|line| {
            let candidate = line.trim().trim_start_matches(['-', '*']).trim();
            normalize_fact(candidate) == needle
                || candidate
                    .split_once("] ")
                    .is_some_and(|(_, text)| normalize_fact(text) == needle)
        })
}

/// Append extracted facts to their matching sections in agverse.md.
/// Delegates to `agverse_md` which ensures standard sections and replaces conflicts.
#[cfg(test)]
fn append_facts_to_sections(content: &str, facts: &[ExtractedFact]) -> String {
    let pairs: Vec<(String, String)> = facts
        .iter()
        .map(|f| (f.section.clone(), f.text.clone()))
        .collect();
    agverse_md::append_facts_to_sections(content, &pairs)
}

fn build_extraction_prompt(conversation: &str, existing_memory: &str) -> String {
    format!(
        r#"You maintain two distinct memory products for an AI agent.

Return strict JSON with this shape:
{{"summary":"A concise session-scoped summary of what happened in this batch.","facts":[{{"section":"User Preferences","text":"A durable fact"}}]}}

Rules:
- summary: capture decisions, outcomes, and unresolved work in this conversation batch.
- facts: ONLY new durable facts that will still be true in 3+ months across future conversations.
- Prefer: user preferences, hard constraints, agent instructions, stable architecture decisions, coding conventions, tool choices.
- Do NOT put into facts:
  - file paths with line numbers (e.g. foo.rs:123)
  - audit checklists, defect inventories, or numbered bug dumps
  - transient progress / coverage / compile status ("now has", "passing tests", "as of this batch")
  - session-only TODOs, greetings, questions, or a general recap
- Each fact text must be <= 200 characters and stand alone as one bullet.
- Do not repeat anything already present in Existing Core Memory.
- Valid fact sections: "Project Overview", "Tech Stack & Commands", "Architecture Decisions", "Coding Conventions", "User Preferences", "Agent Instructions".
- Empty facts array is preferred when unsure. The summary is still required.

Existing Core Memory:
{existing_memory}

Conversation Batch:
{conversation}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflection_cursor_survives_repository_recreation_without_copying_messages() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall
            .store(
                "session-1",
                "user",
                "Please keep this durable architecture decision",
                None,
            )
            .unwrap();
        recall
            .store(
                "session-1",
                "assistant",
                "I will remember this architecture decision",
                None,
            )
            .unwrap();

        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let batch = repository
            .load_pending_batch("session-1", 20)
            .unwrap()
            .unwrap();
        assert_eq!(batch.messages.len(), 2);
        let claim = repository.try_claim("session-1", 2100).unwrap().unwrap();
        repository
            .complete_batch(
                &batch,
                "The session established an architecture decision.",
                &claim,
            )
            .unwrap();
        let reflected: i64 = storage
            .conn()
            .query_row(
                "SELECT last_reflected_sequence FROM reflection_state WHERE session_id = ?1",
                ["session-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reflected, batch.last_sequence());

        let reopened = ReflectionRepository::new(storage);
        assert!(
            reopened
                .load_pending_batch("session-1", 20)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn explicit_sequence_cursor_survives_deletion_and_vacuum() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall.store("s1", "user", "first", None).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let first = repository.load_pending_batch("s1", 20).unwrap().unwrap();
        let claim = repository.try_claim("s1", 2100).unwrap().unwrap();
        repository
            .complete_batch(&first, "first summary", &claim)
            .unwrap();

        {
            let db = storage.conn();
            db.execute("DELETE FROM recall_memory WHERE session_id = 's1'", [])
                .unwrap();
            db.execute_batch("VACUUM").unwrap();
        }
        recall.store("s1", "user", "second", None).unwrap();

        let second = repository.load_pending_batch("s1", 20).unwrap().unwrap();
        assert_eq!(second.messages.len(), 1);
        assert_eq!(second.messages[0].content, "second");
        assert!(second.messages[0].sequence > first.messages[0].sequence);
    }

    #[test]
    fn periodic_flush_accepts_a_single_pending_message() {
        assert!(!batch_ready(1, 20, false));
        assert!(batch_ready(1, 20, true));
        assert!(!batch_ready(0, 20, true));
    }

    #[test]
    fn unreflected_messages_are_not_pruned() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall.store("s1", "user", "pending", None).unwrap();
        repository.ensure_pending("s1").unwrap();
        assert_eq!(recall.prune_cold_memories(2.0, 1.0, 10).unwrap(), 0);

        let batch = repository.load_pending_batch("s1", 20).unwrap().unwrap();
        let claim = repository.try_claim("s1", 2100).unwrap().unwrap();
        repository.complete_batch(&batch, "done", &claim).unwrap();
        assert_eq!(recall.prune_cold_memories(2.0, 1.0, 10).unwrap(), 1);
    }

    #[test]
    fn standard_mode_messages_remain_eligible_for_cleanup() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall.store("s1", "user", "standard mode", None).unwrap();
        repository.ensure_pending("s1").unwrap();
        disable_reflection(storage).unwrap();

        assert_eq!(recall.prune_cold_memories(2.0, 1.0, 10).unwrap(), 1);
    }

    #[tokio::test]
    async fn deleted_session_cancels_in_flight_reflection() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        storage.conn().execute(
            "INSERT INTO sessions (id, title, start_time, created_at, updated_at) VALUES ('s1', 'test', ?1, ?1, ?1)",
            [&now],
        ).unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall.store("s1", "user", "delete me", None).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let batch = repository.load_pending_batch("s1", 20).unwrap().unwrap();
        let claim = repository.try_claim("s1", 2100).unwrap().unwrap();
        crate::session::SessionManager::new(storage.clone())
            .delete("s1")
            .unwrap();

        let memory = Arc::new(Mutex::new(
            crate::memory::MemoryManager::without_embedding(db_path.to_str().unwrap(), 2000, None)
                .unwrap(),
        ));
        let agverse_path = dir.path().join("agverse.md");
        let result = persist_reflection_output(
            &memory,
            &repository,
            &batch,
            &claim,
            &agverse_path,
            ReflectionOutput {
                summary: "must not survive".into(),
                facts: vec![],
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!agverse_path.exists());
        assert_eq!(repository.status().unwrap().summaries_created, 0);
    }

    #[test]
    fn status_keeps_global_last_success_and_last_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall.store("retry", "user", "done", None).unwrap();
        let repository = ReflectionRepository::new(storage);
        repository.enable().unwrap();
        let batch = repository.load_pending_batch("retry", 20).unwrap().unwrap();
        let failure_claim = repository.try_claim("retry", 2100).unwrap().unwrap();
        repository
            .mark_error("retry", &failure_claim, "model unavailable")
            .unwrap();
        let success_claim = repository.try_claim("retry", 2100).unwrap().unwrap();
        repository
            .complete_batch(&batch, "done", &success_claim)
            .unwrap();

        let status = repository.status().unwrap();
        assert_eq!(status.status, "idle");
        assert!(!status.last_success_at.is_empty());
        assert_eq!(status.last_error, "model unavailable");
    }

    #[test]
    fn only_one_worker_can_claim_a_session_batch() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let first = ReflectionRepository::new(storage.clone());
        let second = ReflectionRepository::new(storage);
        first.enable().unwrap();
        assert!(first.try_claim("session-1", 2100).unwrap().is_some());
        assert!(second.try_claim("session-1", 2100).unwrap().is_none());
    }

    #[test]
    fn disabled_reflection_rejects_old_daemon_claims() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        repository.ensure_pending("s1").unwrap();
        disable_reflection(storage).unwrap();

        assert!(repository.try_claim("s1", 2100).unwrap().is_none());
    }

    #[tokio::test]
    async fn summary_and_durable_facts_are_separate_and_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let recall = crate::memory::recall::RecallMemory::without_embedding(storage.clone());
        recall
            .store("s1", "user", "Use SQLite as the only local database", None)
            .unwrap();
        recall
            .store(
                "s1",
                "assistant",
                "Confirmed the SQLite architecture decision",
                None,
            )
            .unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        repository.enable().unwrap();
        let batch = repository.load_pending_batch("s1", 20).unwrap().unwrap();
        let memory = Arc::new(Mutex::new(
            crate::memory::MemoryManager::without_embedding(db_path.to_str().unwrap(), 2000, None)
                .unwrap(),
        ));
        let agverse_path = dir.path().join("agverse.md");
        tokio::fs::write(&agverse_path, "# Architecture Decisions\n")
            .await
            .unwrap();
        let output = ReflectionOutput {
            summary: "The team selected SQLite for local persistence.".into(),
            facts: vec![
                ExtractedFact {
                    section: "Architecture Decisions".into(),
                    text: "SQLite is the only local database.".into(),
                },
                ExtractedFact {
                    section: "Architecture Decisions".into(),
                    text: "SQLite is the only local database.".into(),
                },
            ],
        };

        let first_claim = repository.try_claim("s1", 2100).unwrap().unwrap();
        persist_reflection_output(
            &memory,
            &repository,
            &batch,
            &first_claim,
            &agverse_path,
            output.clone(),
        )
        .await
        .unwrap();
        let second_claim = repository.try_claim("s1", 2100).unwrap().unwrap();
        persist_reflection_output(
            &memory,
            &repository,
            &batch,
            &second_claim,
            &agverse_path,
            output,
        )
        .await
        .unwrap();

        let db = storage.conn();
        let summaries: i64 = db
            .query_row("SELECT COUNT(*) FROM conversation_summaries", [], |r| {
                r.get(0)
            })
            .unwrap();
        let facts: i64 = db.query_row("SELECT COUNT(*) FROM archival_memory WHERE metadata LIKE '%\"source\":\"reflection\"%'", [], |r| r.get(0)).unwrap();
        drop(db);
        let core = tokio::fs::read_to_string(&agverse_path).await.unwrap();
        assert_eq!(summaries, 1);
        assert_eq!(facts, 1);
        assert_eq!(
            core.matches("SQLite is the only local database.").count(),
            1
        );
        assert!(!core.contains("The team selected SQLite for local persistence."));
    }

    #[test]
    fn fact_provenance_protects_manual_and_shared_core_memory() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        let repository = ReflectionRepository::new(storage.clone());
        let fact = ExtractedFact {
            section: "Architecture Decisions".into(),
            text: "Shared durable fact".into(),
        };
        assert!(
            repository
                .store_fact(&fact, "s1", false, "global", "active", "{}", &[])
                .unwrap()
        );
        assert!(
            !repository
                .store_fact(&fact, "s2", false, "global", "active", "{}", &[])
                .unwrap()
        );

        let db = storage.conn();
        let owned: i64 = db
            .query_row(
                "SELECT agverse_owned FROM reflection_facts WHERE fact_key = ?1",
                [normalize_fact(&fact.text)],
                |row| row.get(0),
            )
            .unwrap();
        let sources: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM reflection_fact_sources WHERE fact_key = ?1",
                [normalize_fact(&fact.text)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owned, 0);
        assert_eq!(sources, 2);
    }

    #[test]
    fn file_operation_journal_recovers_prepared_and_committed_edits() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("memory.db");
        let target = dir.path().join("agverse.md");
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();

        std::fs::write(&target, "updated").unwrap();
        storage
            .conn()
            .execute(
                "INSERT INTO reflection_file_operations \
             (id, path, original_content, updated_content, state, created_at) \
             VALUES ('prepared', ?1, 'original', 'updated', 'prepared', ?2)",
                rusqlite::params![target.to_string_lossy(), chrono::Utc::now().to_rfc3339()],
            )
            .unwrap();
        drop(storage);
        let storage = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");

        std::fs::write(&target, "stale").unwrap();
        storage
            .conn()
            .execute(
                "INSERT INTO reflection_file_operations \
             (id, path, original_content, updated_content, state, created_at) \
             VALUES ('committed', ?1, 'original', 'final', 'committed', ?2)",
                rusqlite::params![target.to_string_lossy(), chrono::Utc::now().to_rfc3339()],
            )
            .unwrap();
        drop(storage);
        let _recovered = crate::memory::storage::Storage::new(db_path.to_str().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "final");
    }

    #[test]
    fn test_should_skip_short() {
        assert!(!should_skip("user", "ok"));
        assert!(!should_skip("user", "hi there"));
        assert!(!should_skip(
            "user",
            "I prefer using Rust for all backend work"
        ));
    }

    #[test]
    fn test_should_skip_tool_output() {
        assert!(should_skip("tool", "stdout: hello world output here"));
        assert!(should_skip("tool", "exit code: 0"));
        assert!(!should_skip(
            "tool",
            "The build succeeded with warnings about unused imports in main.rs"
        ));
    }

    #[test]
    fn test_should_skip_error_stack() {
        let stack = "panic: thread crashed\n".repeat(15);
        assert!(should_skip("assistant", &stack));
        assert!(!should_skip(
            "assistant",
            "There was a panic: but this is a short message"
        ));
    }

    #[test]
    fn reflection_output_requires_summary_and_structured_facts() {
        let output = parse_reflection_output(
            r#"```json
{"summary":"The team selected Rust.","facts":[{"section":"Tech Stack & Commands","text":"Rust is the backend language."}]}
```"#,
        ).unwrap();
        assert_eq!(output.summary, "The team selected Rust.");
        assert_eq!(output.facts.len(), 1);
        assert!(parse_reflection_output(r#"{"facts":[]}"#).is_err());
    }

    #[test]
    fn reflection_output_accepts_json_wrapped_in_model_reasoning_and_prose() {
        let response = r#"<think>I need to separate the summary from durable facts.</think>
Here is the requested result:
```json
{"summary":"The user fixed reflection persistence.","facts":[]}
```
This follows the requested schema."#;

        let output = parse_reflection_output(response)
            .expect("a valid JSON object embedded in model prose should be parsed");
        assert_eq!(output.summary, "The user fixed reflection persistence.");
        assert!(output.facts.is_empty());
    }

    #[test]
    fn test_build_extraction_prompt_contains_rules() {
        let prompt = build_extraction_prompt("test conversation", "existing fact");
        assert!(prompt.contains("two distinct memory products"));
        assert!(prompt.contains("Existing Core Memory"));
        assert!(prompt.contains("existing fact"));
        assert!(prompt.contains("test conversation"));
        assert!(prompt.contains("<= 200 characters"));
        assert!(prompt.contains("line numbers"));
    }

    #[test]
    fn test_append_facts_to_existing_section() {
        let content = "# Project Overview\n\nSome overview.\n\n# Architecture Decisions\n\n";
        let facts = vec![ExtractedFact {
            section: "Architecture Decisions".to_string(),
            text: "Use SQLite for local storage".to_string(),
        }];
        let result = append_facts_to_sections(content, &facts);
        assert!(result.contains("- Use SQLite for local storage"));
        assert!(result.contains("# Architecture Decisions"));
    }

    #[test]
    fn test_append_facts_creates_missing_section_not_pending() {
        let content = "# Project Overview\n\nSome overview.\n";
        let facts = vec![ExtractedFact {
            section: "User Preferences".to_string(),
            text: "User prefers dark mode".to_string(),
        }];
        let result = append_facts_to_sections(content, &facts);
        assert!(result.contains("# User Preferences"));
        assert!(result.contains("- User prefers dark mode"));
        assert!(!result.contains("# Pending Notes"));
    }
}
