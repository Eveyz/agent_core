use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, bytes_to_embedding, cosine_similarity, embedding_to_bytes};
use super::storage::Storage;

#[derive(Debug, Clone)]
pub struct ArchivalRecord {
    pub id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: Option<String>,
    pub created_at: String,
}

pub struct ArchivalMemory {
    storage: Storage,
    embedding_model: Arc<EmbeddingModel>,
}

impl ArchivalMemory {
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model,
        }
    }

    pub fn insert(&self, content: &str, metadata: Option<&str>) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let embedding = self.embedding_model.embed_single(content)?;
        let embedding_bytes = embedding_to_bytes(&embedding);
        let now = Utc::now().to_rfc3339();

        let db = self.storage.conn();
        db.execute(
            "INSERT INTO archival_memory (id, content, embedding, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, content, embedding_bytes, metadata, now],
        )
        .context("failed to insert archival memory")?;

        Ok(id)
    }

    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<ArchivalRecord>> {
        let query_embedding = self.embedding_model.embed_single(query)?;

        let db = self.storage.conn();
        let mut stmt = db
            .prepare(
                "SELECT id, content, embedding, metadata, created_at FROM archival_memory ORDER BY created_at DESC LIMIT 1000",
            )
            .context("failed to prepare archival search query")?;

        let mut scored: Vec<(f32, ArchivalRecord)> = Vec::new();

        let rows = stmt.query_map([], |row| {
            let embedding_bytes: Vec<u8> = row.get(2)?;
            Ok(ArchivalRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                embedding: bytes_to_embedding(&embedding_bytes),
                metadata: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;

        for row in rows {
            let record = row?;
            let score = cosine_similarity(&query_embedding, &record.embedding);
            scored.push((score, record));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let db = self.storage.conn();
        let changed = db
            .execute(
                "DELETE FROM archival_memory WHERE id = ?1",
                rusqlite::params![id],
            )
            .context("failed to delete archival memory")?;

        Ok(changed > 0)
    }
}
