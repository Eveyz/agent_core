use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, bytes_to_embedding, cosine_similarity, embedding_to_bytes};
use super::salience::{MemoryCategory, SalienceConfig, SalienceScorer};
use super::storage::Storage;

/// A record in the recall (short-term conversational) memory.
#[derive(Debug, Clone)]
pub struct RecallRecord {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub importance: f32,
    pub memory_strength: f32,
    pub access_count: u32,
    pub last_accessed_at: Option<String>,
    pub created_at: String,
}

pub struct RecallMemory {
    storage: Storage,
    embedding_model: Arc<EmbeddingModel>,
    scorer: SalienceScorer,
}

impl RecallMemory {
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model,
            scorer: SalienceScorer::new(SalienceConfig::default()),
        }
    }

    pub fn with_config(mut self, config: SalienceConfig) -> Self {
        self.scorer = SalienceScorer::new(config);
        self
    }

    pub fn scorer(&self) -> &SalienceScorer {
        &self.scorer
    }

    // ── Store ───────────────────────────────────────────────────────

    /// Store a memory with automatic importance rating.
    /// importance: optional override (0.0-1.0). If None, uses auto-rating.
    pub fn store(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        importance: Option<f32>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let embedding = self.embedding_model.embed_single(content)?;
        let embedding_bytes = embedding_to_bytes(&embedding);
        let now = Utc::now().to_rfc3339();

        let importance = importance.unwrap_or_else(|| {
            self.scorer.auto_rate_importance(content, role)
        });

        let db = self.storage.conn();
        db.execute(
            "INSERT INTO recall_memory (id, session_id, role, content, embedding, importance, memory_strength, access_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1.0, 0, ?7)",
            rusqlite::params![id, session_id, role, content, embedding_bytes, importance, now],
        )
        .context("failed to store recall memory")?;

        Ok(id)
    }

    /// Legacy compatibility: store with raw importance float.
    pub fn store_raw(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        importance: f32,
    ) -> Result<String> {
        self.store(session_id, role, content, Some(importance))
    }

    // ── Search ──────────────────────────────────────────────────────

    /// Search recall memory using the salience scorer
    /// (Ebbinghaus decay × semantic similarity × importance).
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<RecallRecord>> {
        let query_embedding = self.embedding_model.embed_single(query)?;
        self.search_by_vector(&query_embedding, top_k)
    }

    /// Search by query embedding vector.
    pub fn search_by_vector(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<RecallRecord>> {
        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), \
                 COALESCE(access_count, 0), \
                 last_accessed_at, \
                 created_at \
                 FROM recall_memory ORDER BY created_at DESC LIMIT 1000",
            )
            .context("failed to prepare recall search query")?;

        let now = Utc::now();
        let mut scored: Vec<(f32, RecallRecord)> = Vec::new();

        let rows = stmt.query_map([], |row| {
            let embedding_bytes: Vec<u8> = row.get(4)?;
            let embedding = bytes_to_embedding(&embedding_bytes);
            Ok(RecallRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                embedding,
                importance: row.get(5)?,
                memory_strength: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;

        for row in rows {
            let record = row?;

            // Semantic similarity
            let semantic = cosine_similarity(query_embedding, &record.embedding);

            // Hours since creation
            let hours_since = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                .ok()
                .map(|dt| (now - dt.with_timezone(&Utc)).num_hours().max(0) as f32)
                .unwrap_or(0.0);

            // Combined salience score
            let score = self.scorer.retrieval_score(
                semantic,
                hours_since,
                record.memory_strength,
                record.importance,
            );

            scored.push((score, record));
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    // ── Scored search (returns breakdown) ───────────────────────────

    /// Search and return scored records with score breakdown.
    pub fn search_scored(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<super::salience::ScoredRecord>> {
        let query_embedding = self.embedding_model.embed_single(query)?;

        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), created_at \
                 FROM recall_memory ORDER BY created_at DESC LIMIT 1000",
            )
            .context("failed to prepare scored search")?;

        let now = Utc::now();
        let mut scored: Vec<(f32, super::salience::ScoredRecord)> = Vec::new();

        let rows = stmt.query_map([], |row| {
            let embedding_bytes: Vec<u8> = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                bytes_to_embedding(&embedding_bytes),
                row.get::<_, f32>(3)?,
                row.get::<_, f32>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        for row in rows {
            let (id, content, embedding, importance, memory_strength, created_at) = row?;

            let semantic = cosine_similarity(&query_embedding, &embedding);

            let hours_since = chrono::DateTime::parse_from_rfc3339(&created_at)
                .ok()
                .map(|dt| (now - dt.with_timezone(&Utc)).num_hours().max(0) as f32)
                .unwrap_or(0.0);

            let recall = self.scorer.recall_score(hours_since, memory_strength, importance);
            let total = self.scorer.retrieval_score(semantic, hours_since, memory_strength, importance);
            let category = MemoryCategory::classify(&content, "user");

            scored.push((
                total,
                super::salience::ScoredRecord {
                    id,
                    content,
                    total_score: total,
                    semantic_score: semantic,
                    recall_score: recall,
                    importance,
                    memory_strength,
                    hours_since_created: hours_since,
                    category,
                },
            ));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    // ── Strength reinforcement ──────────────────────────────────────

    /// Bump the memory_strength and access_count of a record (called after retrieval).
    pub fn bump_strength(&self, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.storage.conn();

        // Read current strength
        let current_strength: f32 = db
            .query_row(
                "SELECT COALESCE(memory_strength, 1.0) FROM recall_memory WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .unwrap_or(1.0);

        let new_strength = self.scorer.bump_strength(current_strength);

        db.execute(
            "UPDATE recall_memory SET memory_strength = ?1, access_count = access_count + 1, last_accessed_at = ?2 WHERE id = ?3",
            rusqlite::params![new_strength, now, id],
        )
        .context("failed to bump memory strength")?;

        Ok(())
    }

    /// Bump strength for multiple records (e.g., top-K search results).
    pub fn bump_strength_batch(&self, ids: &[&str]) -> Result<()> {
        for id in ids {
            let _ = self.bump_strength(id);
        }
        Ok(())
    }

    // ── Date search ─────────────────────────────────────────────────

    pub fn search_by_date(
        &self,
        start: &str,
        end: &str,
        top_k: usize,
    ) -> Result<Vec<RecallRecord>> {
        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), \
                 COALESCE(access_count, 0), \
                 last_accessed_at, \
                 created_at \
                 FROM recall_memory WHERE created_at >= ?1 AND created_at <= ?2 \
                 ORDER BY created_at DESC LIMIT ?3",
            )
            .context("failed to prepare date search query")?;

        let rows = stmt.query_map(rusqlite::params![start, end, top_k as i64], |row| {
            let embedding_bytes: Vec<u8> = row.get(4)?;
            Ok(RecallRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                embedding: bytes_to_embedding(&embedding_bytes),
                importance: row.get(5)?,
                memory_strength: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    // ── Stats ───────────────────────────────────────────────────────

    /// Get count and average strength of memories.
    pub fn stats(&self) -> Result<MemoryStats> {
        let db = self.storage.conn();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM recall_memory", [], |row| row.get(0))
            .unwrap_or(0);
        let avg_strength: f64 = db
            .query_row(
                "SELECT AVG(COALESCE(memory_strength, 1.0)) FROM recall_memory",
                [],
                |row| row.get(0),
            )
            .unwrap_or(1.0);
        let avg_importance: f64 = db
            .query_row(
                "SELECT AVG(importance) FROM recall_memory",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0.5);

        Ok(MemoryStats {
            total_count: count as usize,
            avg_strength: avg_strength as f32,
            avg_importance: avg_importance as f32,
        })
    }

    // ── Proactive forgetting ────────────────────────────────────────

    /// Prune cold memories: delete records whose estimated recall score
    /// has fallen below `min_score` AND importance is below `min_importance`.
    /// Returns the number of deleted records.
    pub fn prune_cold_memories(
        &self,
        min_score: f32,
        min_importance: f32,
        max_to_delete: usize,
    ) -> Result<usize> {
        let db = self.storage.conn();
        let now = Utc::now();

        // Fetch old memories for scoring
        let mut stmt = db
            .prepare(
                "SELECT id, importance, COALESCE(memory_strength, 1.0), created_at \
                 FROM recall_memory \
                 WHERE importance < ?1 \
                 ORDER BY created_at ASC LIMIT ?2",
            )
            .context("failed to prepare prune query")?;

        let mut to_delete = Vec::new();
        let rows = stmt.query_map(
            rusqlite::params![min_importance, max_to_delete as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f32>(1)?,
                    row.get::<_, f32>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;

        for row in rows {
            let (id, importance, strength, created_at) = row?;
            let hours_since = chrono::DateTime::parse_from_rfc3339(&created_at)
                .ok()
                .map(|dt| (now - dt.with_timezone(&Utc)).num_hours().max(0) as f32)
                .unwrap_or(0.0);

            let recall = self.scorer.recall_score(hours_since, strength, importance);
            if recall < min_score {
                to_delete.push(id);
            }
        }

        let deleted = to_delete.len();
        if deleted > 0 {
            // Build IN clause
            let placeholders: Vec<String> = to_delete.iter().enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM recall_memory WHERE id IN ({})",
                placeholders.join(", ")
            );
            let params: Vec<rusqlite::types::Value> = to_delete
                .iter()
                .map(|id| rusqlite::types::Value::Text(id.clone()))
                .collect();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p as &dyn rusqlite::types::ToSql).collect();

            db.execute(&sql, param_refs.as_slice())
                .context("failed to prune cold memories")?;
        }

        Ok(deleted)
    }

    /// Promote old high-importance memories to archival memory.
    /// Returns the number of promoted records.
    pub fn promote_to_archival(
        &self,
        archival: &super::archival::ArchivalMemory,
        min_importance: f32,
        max_to_promote: usize,
    ) -> Result<usize> {
        let db = self.storage.conn();

        let mut stmt = db
            .prepare(
                "SELECT id, content FROM recall_memory \
                 WHERE importance >= ?1 \
                 ORDER BY created_at ASC LIMIT ?2",
            )
            .context("failed to prepare promote query")?;

        let mut promoted = 0;
        let rows = stmt.query_map(
            rusqlite::params![min_importance, max_to_promote as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            },
        )?;

        for row in rows {
            let (id, content) = row?;
            if archival.insert(&content, Some("promoted from recall")).is_ok() {
                db.execute(
                    "DELETE FROM recall_memory WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                promoted += 1;
            }
        }

        Ok(promoted)
    }
}

#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_count: usize,
    pub avg_strength: f32,
    pub avg_importance: f32,
}
