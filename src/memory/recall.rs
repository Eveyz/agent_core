use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, bytes_to_embedding, cosine_similarity, embedding_to_bytes};
use super::storage::Storage;

#[derive(Debug, Clone)]
pub struct RecallRecord {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub importance: f32,
    pub created_at: String,
}

pub struct RecallMemory {
    storage: Storage,
    embedding_model: Arc<EmbeddingModel>,
}

impl RecallMemory {
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model,
        }
    }

    pub fn store(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        importance: f32,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let embedding = self.embedding_model.embed_single(content)?;
        let embedding_bytes = embedding_to_bytes(&embedding);
        let now = Utc::now().to_rfc3339();

        let db = self.storage.conn();
        db.execute(
            "INSERT INTO recall_memory (id, session_id, role, content, embedding, importance, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, session_id, role, content, embedding_bytes, importance, now],
        )
        .context("failed to store recall memory")?;

        Ok(id)
    }

    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<RecallRecord>> {
        let query_embedding = self.embedding_model.embed_single(query)?;
        self.search_by_vector(&query_embedding, top_k)
    }

    pub fn search_by_vector(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<RecallRecord>> {
        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, session_id, role, content, embedding, importance, created_at FROM recall_memory ORDER BY created_at DESC LIMIT 1000",
            )
            .context("failed to prepare recall search query")?;

        let now = Utc::now();
        let mut scored: Vec<(f32, RecallRecord)> = Vec::new();

        let rows = stmt.query_map([], |row| {
            let embedding_bytes: Vec<u8> = row.get(4)?;
            let embedding = bytes_to_embedding(&embedding_bytes);
            let importance: f32 = row.get(5)?;
            let created_at: String = row.get(6)?;

            Ok(RecallRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                embedding,
                importance,
                created_at,
            })
        })?;

        for row in rows {
            let record = row?;
            let semantic = cosine_similarity(query_embedding, &record.embedding);

            let hours_since = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                .ok()
                .map(|dt| (now - dt.with_timezone(&Utc)).num_hours().max(0) as f32)
                .unwrap_or(0.0);
            let recency = 1.0 / (1.0 + hours_since);

            let score = 0.6 * semantic + 0.2 * recency + 0.2 * record.importance;
            scored.push((score, record));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    pub fn search_by_date(
        &self,
        start: &str,
        end: &str,
        top_k: usize,
    ) -> Result<Vec<RecallRecord>> {
        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, session_id, role, content, embedding, importance, created_at FROM recall_memory WHERE created_at >= ?1 AND created_at <= ?2 ORDER BY created_at DESC LIMIT ?3",
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
                created_at: row.get(6)?,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }
}
