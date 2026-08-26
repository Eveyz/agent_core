use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, bytes_to_embedding, cosine_similarity, embedding_to_bytes};
use super::storage::Storage;

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
    embedding_model: Option<Arc<EmbeddingModel>>,
}

impl ArchivalMemory {
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model: Some(embedding_model),
        }
    }

    pub fn without_embedding(storage: Storage) -> Self {
        Self {
            storage,
            embedding_model: None,
        }
    }

    /// Access the embedding model (if configured).
    pub fn embedding_model(&self) -> Option<&Arc<EmbeddingModel>> {
        self.embedding_model.as_ref()
    }

    pub fn insert(&self, content: &str, metadata: Option<&str>) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let embedding_bytes = if let Some(ref model) = self.embedding_model {
            let embedding = model.embed_single(content)?;
            embedding_to_bytes(&embedding)
        } else {
            Vec::new()
        };
        let now = Utc::now().to_rfc3339();

        let db = self.storage.conn();
        db.execute(
            "INSERT INTO archival_memory (id, content, embedding, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, content, embedding_bytes, metadata, now],
        )
        .context("failed to insert archival memory")?;

        Ok(id)
    }

    /// Insert with a pre-computed embedding (avoids recomputing on promotion).
    pub fn insert_with_embedding(
        &self,
        content: &str,
        embedding_bytes: &[u8],
        metadata: Option<&str>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
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
        if let Some(ref model) = self.embedding_model {
            let query_embedding = model.embed_single(query)?;
            self.search_by_vector(&query_embedding, query, top_k)
        } else {
            self.search_by_keyword(query, top_k)
        }
    }

    pub fn search_by_vector(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        top_k: usize,
    ) -> Result<Vec<ArchivalRecord>> {
        let db = self.storage.conn();

        // FTS5 pre-filter: collect candidate rowids
        let mut candidates = std::collections::HashSet::new();

        if let Some(fts_query) = build_fts_query(query_text) {
            if let Ok(mut stmt) = db.prepare_cached(
                "SELECT rowid FROM archival_memory_fts WHERE archival_memory_fts MATCH ?1 LIMIT 50",
            ) {
                if let Ok(rows) =
                    stmt.query_map(rusqlite::params![fts_query], |row| row.get::<_, i64>(0))
                {
                    for row in rows.flatten() {
                        candidates.insert(row);
                    }
                }
            }
        }

        // Always include recent records for recency
        if let Ok(mut stmt) = db
            .prepare_cached("SELECT rowid FROM archival_memory ORDER BY created_at DESC LIMIT 100")
        {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
                for row in rows.flatten() {
                    candidates.insert(row);
                }
            }
        }

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let rowids: Vec<i64> = candidates.into_iter().collect();
        let placeholders = rowids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT id, content, embedding, metadata, created_at FROM archival_memory WHERE rowid IN ({})",
            placeholders
        );

        let params: Vec<&dyn rusqlite::ToSql> =
            rowids.iter().map(|r| r as &dyn rusqlite::ToSql).collect();

        let mut stmt = db.prepare(&sql)?;
        let mut scored: Vec<(f32, ArchivalRecord)> = Vec::new();

        let rows = stmt.query_map(params.as_slice(), |row| {
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
            let score = cosine_similarity(query_embedding, &record.embedding);
            scored.push((score, record));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    pub fn search_by_keyword(&self, query: &str, top_k: usize) -> Result<Vec<ArchivalRecord>> {
        let db = self.storage.conn();

        // Try FTS5 first
        if let Some(fts_query) = build_fts_query(query) {
            let mut stmt = db.prepare_cached(
                "SELECT a.id, a.content, a.embedding, a.metadata, a.created_at \
                 FROM archival_memory_fts f \
                 JOIN archival_memory a ON a.rowid = f.rowid \
                 WHERE archival_memory_fts MATCH ?1 \
                 ORDER BY rank LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![fts_query, top_k as i64], |row| {
                let embedding_bytes: Vec<u8> = row.get(2)?;
                Ok(ArchivalRecord {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    embedding: bytes_to_embedding(&embedding_bytes),
                    metadata: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            if !results.is_empty() {
                return Ok(results);
            }
        }

        // Fallback: LIKE search
        let pattern = format!("%{}%", query);
        let mut stmt = db.prepare_cached(
            "SELECT id, content, embedding, metadata, created_at \
             FROM archival_memory WHERE content LIKE ?1 \
             ORDER BY created_at DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(rusqlite::params![pattern, top_k as i64], |row| {
            let embedding_bytes: Vec<u8> = row.get(2)?;
            Ok(ArchivalRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                embedding: bytes_to_embedding(&embedding_bytes),
                metadata: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
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
