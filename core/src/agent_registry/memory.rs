//! Per-agent (and memory-group) persistent memory store.
//!
//! [`AgentMemoryStore`] is intentionally separate from the global
//! [`crate::memory::MemoryManager`]: it writes to the `agent_memory` table,
//! indexes by `memory_key` (agent id or shared group name), and never touches
//! the main `recall_memory` table. This guarantees the primary agent's memory
//! is unaffected by custom-agent execution.

use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

use crate::memory::embedding::{EmbeddingModel, cosine_similarity, embedding_to_bytes};
use crate::memory::salience::{MemoryCategory, SalienceConfig, SalienceScorer};
use crate::memory::storage::Storage;

/// A record in the per-agent `agent_memory` table.
#[derive(Debug, Clone)]
pub struct AgentMemoryRecord {
    pub id: String,
    pub agent_id: String,
    pub memory_key: String,
    pub role: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub importance: f32,
    pub memory_strength: f32,
    pub access_count: u32,
    pub last_accessed_at: Option<String>,
    pub category: MemoryCategory,
    pub source: String,
    pub created_at: String,
}

pub struct AgentMemoryStore {
    storage: Storage,
    embedding_model: Option<Arc<EmbeddingModel>>,
    scorer: SalienceScorer,
}

impl AgentMemoryStore {
    /// Create a store with embedding-based semantic search.
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model: Some(embedding_model),
            scorer: SalienceScorer::new(SalienceConfig::default()),
        }
    }

    /// Create a store without an embedding model (keyword/FTS search only).
    pub fn without_embedding(storage: Storage) -> Self {
        Self {
            storage,
            embedding_model: None,
            scorer: SalienceScorer::new(SalienceConfig::default()),
        }
    }

    pub fn with_config(mut self, config: SalienceConfig) -> Self {
        self.scorer = SalienceScorer::new(config);
        self
    }

    pub fn has_embedding(&self) -> bool {
        self.embedding_model.is_some()
    }

    // ── Store ───────────────────────────────────────────────────────

    /// Store a memory entry for the given `memory_key`.
    ///
    /// `agent_id` records which agent wrote the entry (for provenance), while
    /// `memory_key` determines isolation (agent id or shared group name).
    pub fn store(
        &self,
        memory_key: &str,
        agent_id: &str,
        role: &str,
        content: &str,
        source: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let embedding_bytes = if let Some(ref model) = self.embedding_model {
            let embedding = model.embed_single(content)?;
            embedding_to_bytes(&embedding)
        } else {
            Vec::new()
        };
        let now = Utc::now().to_rfc3339();
        let importance = self.scorer.auto_rate_importance(content, role);
        let category = MemoryCategory::classify(content, role);
        let category_str = match category {
            MemoryCategory::Conversation => "Conversation",
            MemoryCategory::Decision => "Decision",
            MemoryCategory::Code => "Code",
            MemoryCategory::Preference => "Preference",
            MemoryCategory::Trivia => "Trivia",
        };

        let db = self.storage.conn();
        db.execute(
            "INSERT INTO agent_memory \
             (id, agent_id, memory_key, role, content, embedding, importance, \
             memory_strength, access_count, last_accessed_at, category, source, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1.0, 0, NULL, ?8, ?9, ?10)",
            rusqlite::params![
                id,
                agent_id,
                memory_key,
                role,
                content,
                embedding_bytes,
                importance,
                category_str,
                source,
                now,
            ],
        )
        .context("failed to store agent memory")?;

        Ok(id)
    }

    // ── Search ──────────────────────────────────────────────────────

    /// Search agent memory. Uses vector similarity when an embedding model is
    /// available, otherwise falls back to FTS5 keyword search.
    pub fn search(
        &self,
        memory_key: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<AgentMemoryRecord>> {
        if let Some(ref model) = self.embedding_model {
            let query_embedding = model.embed_single(query)?;
            self.search_by_vector(memory_key, &query_embedding, query, top_k)
        } else {
            self.search_by_keyword(memory_key, query, top_k)
        }
    }

    /// Hybrid search alias (currently equivalent to [`search`]).
    pub fn search_hybrid(
        &self,
        memory_key: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<AgentMemoryRecord>> {
        self.search(memory_key, query, top_k)
    }

    /// Keyword-based search via FTS5, scoped to a single `memory_key`.
    pub fn search_by_keyword(
        &self,
        memory_key: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<AgentMemoryRecord>> {
        let db = self.storage.conn();
        let fts_query = build_fts_query(query);

        let mut candidates: Vec<i64> = Vec::new();

        if let Some(ref fts) = fts_query {
            if let Ok(mut stmt) = db.prepare_cached(
                "SELECT am.rowid FROM agent_memory_fts fts \
                 JOIN agent_memory am ON am.rowid = fts.rowid \
                 WHERE fts.agent_memory_fts MATCH ?1 AND am.memory_key = ?2 \
                 LIMIT 50",
            ) {
                if let Ok(rows) = stmt.query_map(rusqlite::params![fts, memory_key], |row| {
                    row.get::<_, i64>(0)
                }) {
                    for row in rows.flatten() {
                        candidates.push(row);
                    }
                }
            }
        }

        // Always include the most recent records for recency-based recall.
        if let Ok(mut stmt) = db.prepare_cached(
            "SELECT rowid FROM agent_memory WHERE memory_key = ?1 \
             ORDER BY created_at DESC LIMIT 100",
        ) {
            if let Ok(rows) =
                stmt.query_map(rusqlite::params![memory_key], |row| row.get::<_, i64>(0))
            {
                for row in rows.flatten() {
                    if !candidates.contains(&row) {
                        candidates.push(row);
                    }
                }
            }
        }

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let records = load_records_by_rowids(&db, &candidates)?;
        let now = Utc::now();
        let mut scored: Vec<(f32, AgentMemoryRecord)> = records
            .into_iter()
            .map(|r| {
                let hours_since = hours_since(&r.created_at, now);
                let score = self.scorer.retrieval_score(
                    0.0, // no semantic signal in keyword mode
                    hours_since,
                    r.memory_strength,
                    r.importance,
                    r.category,
                );
                (score, r)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    /// Vector similarity search scoped to a single `memory_key`.
    pub fn search_by_vector(
        &self,
        memory_key: &str,
        query_embedding: &[f32],
        query_text: &str,
        top_k: usize,
    ) -> Result<Vec<AgentMemoryRecord>> {
        let db = self.storage.conn();

        let rowids = collect_candidates(&db, memory_key, query_text);
        if rowids.is_empty() {
            return Ok(Vec::new());
        }

        let records = load_records_by_rowids(&db, &rowids)?;
        let now = Utc::now();
        let mut scored: Vec<(f32, AgentMemoryRecord)> = records
            .into_iter()
            .map(|r| {
                let semantic = cosine_similarity(query_embedding, &r.embedding);
                let hours_since = hours_since(&r.created_at, now);
                let score = self.scorer.retrieval_score(
                    semantic,
                    hours_since,
                    r.memory_strength,
                    r.importance,
                    r.category,
                );
                (score, r)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    // ── Context injection ───────────────────────────────────────────

    /// Retrieve relevant memories and format them as a context-injection
    /// string suitable for prepending to an agent's active memory.
    ///
    /// Caps the result at roughly `max_tokens` (4 chars/token heuristic).
    pub fn build_context_injection(
        &self,
        memory_key: &str,
        query: &str,
        max_tokens: usize,
    ) -> String {
        let memories = match self.search_hybrid(memory_key, query, 5) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("agent memory search failed: {e}");
                return String::new();
            }
        };
        if memories.is_empty() {
            return String::new();
        }

        let mut text = String::from("## Relevant Memory from Previous Executions\n\n");
        let budget_bytes = max_tokens.saturating_mul(4);
        for (i, mem) in memories.iter().enumerate() {
            let entry = format!(
                "### Memory {} (importance: {:.2})\n{}\n\n",
                i + 1,
                mem.importance,
                mem.content
            );
            if text.len() + entry.len() > budget_bytes {
                break;
            }
            text.push_str(&entry);
        }
        text
    }

    // ── Maintenance ─────────────────────────────────────────────────

    /// Approximate count of memories for a key.
    pub fn count(&self, memory_key: &str) -> Result<usize> {
        let db = self.storage.conn();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM agent_memory WHERE memory_key = ?1",
                rusqlite::params![memory_key],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count as usize)
    }

    /// Bump the memory strength of a record (call on access).
    pub fn bump_strength(&self, id: &str) -> Result<()> {
        let db = self.storage.conn();
        let now = Utc::now().to_rfc3339();
        let new_strength = self.scorer.bump_strength(1.0);
        db.execute(
            "UPDATE agent_memory SET memory_strength = ?2, access_count = access_count + 1, \
             last_accessed_at = ?3 WHERE id = ?1",
            rusqlite::params![id, new_strength, now],
        )
        .context("failed to bump agent memory strength")?;
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn hours_since(created_at: &str, now: chrono::DateTime<Utc>) -> f32 {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .ok()
        .map(|dt| (now - dt.with_timezone(&Utc)).num_hours().max(0) as f32)
        .unwrap_or(0.0)
}

/// Build an FTS5 MATCH query string from user input.
fn build_fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split_whitespace()
        .filter(|s| s.chars().count() >= 2)
        .map(|s| {
            s.chars()
                .filter(|c| c.is_alphanumeric() || !c.is_ascii())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect();

    if terms.is_empty() {
        return None;
    }

    Some(
        terms
            .iter()
            .map(|t| format!("{t}*"))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// Collect candidate rowids for a memory_key via FTS5 + recent records.
fn collect_candidates(db: &rusqlite::Connection, memory_key: &str, query_text: &str) -> Vec<i64> {
    let mut candidates: Vec<i64> = Vec::new();

    if let Some(fts_query) = build_fts_query(query_text) {
        if let Ok(mut stmt) = db.prepare(
            "SELECT am.rowid FROM agent_memory_fts fts \
             JOIN agent_memory am ON am.rowid = fts.rowid \
             WHERE fts.agent_memory_fts MATCH ?1 AND am.memory_key = ?2 LIMIT 50",
        ) {
            if let Ok(rows) = stmt.query_map(rusqlite::params![fts_query, memory_key], |row| {
                row.get::<_, i64>(0)
            }) {
                for row in rows.flatten() {
                    candidates.push(row);
                }
            }
        }
    }

    if let Ok(mut stmt) = db.prepare(
        "SELECT rowid FROM agent_memory WHERE memory_key = ?1 \
         ORDER BY created_at DESC LIMIT 100",
    ) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![memory_key], |row| row.get::<_, i64>(0))
        {
            for row in rows.flatten() {
                if !candidates.contains(&row) {
                    candidates.push(row);
                }
            }
        }
    }

    candidates
}

/// Load full records for a set of rowids.
fn load_records_by_rowids(
    db: &rusqlite::Connection,
    rowids: &[i64],
) -> Result<Vec<AgentMemoryRecord>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = rowids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, agent_id, memory_key, role, content, embedding, importance, \
         COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), last_accessed_at, \
         COALESCE(category, 'Conversation'), COALESCE(source, 'conversation'), created_at \
         FROM agent_memory WHERE rowid IN ({})",
        placeholders
    );
    let params: Vec<&dyn rusqlite::ToSql> =
        rowids.iter().map(|r| r as &dyn rusqlite::ToSql).collect();
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), parse_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

/// Parse an `agent_memory` row into an [`AgentMemoryRecord`].
fn parse_record(row: &rusqlite::Row) -> rusqlite::Result<AgentMemoryRecord> {
    let embedding_bytes: Vec<u8> = row.get("embedding")?;
    let embedding = if embedding_bytes.is_empty() {
        Vec::new()
    } else {
        crate::memory::embedding::bytes_to_embedding(&embedding_bytes)
    };
    let category_str: String = row.get("category")?;
    Ok(AgentMemoryRecord {
        id: row.get("id")?,
        agent_id: row.get("agent_id")?,
        memory_key: row.get("memory_key")?,
        role: row.get("role")?,
        content: row.get("content")?,
        embedding,
        importance: row.get("importance")?,
        memory_strength: row.get("memory_strength")?,
        access_count: row.get::<_, i64>("access_count")? as u32,
        last_accessed_at: row.get("last_accessed_at")?,
        category: MemoryCategory::from_str(&category_str),
        source: row.get("source")?,
        created_at: row.get("created_at")?,
    })
}
