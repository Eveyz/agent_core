use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, cosine_similarity};
use super::storage::Storage;

#[derive(Clone)]
pub struct MemoryConsolidator {
    storage: Storage,
    embedding_model: Option<Arc<EmbeddingModel>>,
}

impl MemoryConsolidator {
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

    pub fn consolidate(&self) -> Result<ConsolidationReport> {
        if self.embedding_model.is_none() {
            return Ok(ConsolidationReport {
                deduped_recall: 0,
                deduped_archival: 0,
            });
        }
        let deduped_recall = self.dedup_recall_memory()?;
        let deduped_archival = self.dedup_archival_memory()?;

        Ok(ConsolidationReport {
            deduped_recall,
            deduped_archival,
        })
    }

    fn dedup_recall_memory(&self) -> Result<usize> {
        // Phase 1: read records from SQLite (lock held briefly)
        let records: Vec<(String, Vec<f32>)> = {
            let db = self.storage.conn();
            let mut stmt = db.prepare(
                "SELECT id, embedding FROM recall_memory ORDER BY created_at DESC LIMIT 5000",
            )?;
            let mut records = Vec::new();
            let rows = stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let embedding_bytes: Vec<u8> = row.get(1)?;
                let embedding = super::embedding::bytes_to_embedding(&embedding_bytes);
                Ok((id, embedding))
            })?;
            for row in rows {
                records.push(row?);
            }
            records
        }; // storage.db lock released — O(n²) runs lock-free

        // Phase 2: O(n²) dedup computation (lock-free)
        let threshold = 0.85;
        let mut to_delete: HashSet<String> = HashSet::new();

        for i in 0..records.len() {
            if to_delete.contains(&records[i].0) {
                continue;
            }
            for j in (i + 1)..records.len() {
                if to_delete.contains(&records[j].0) {
                    continue;
                }
                let sim = cosine_similarity(&records[i].1, &records[j].1);
                if sim > threshold {
                    to_delete.insert(records[j].0.clone());
                }
            }
        }

        // Phase 3: batch delete (lock held briefly)
        let count = to_delete.len();
        if count > 0 {
            let db = self.storage.conn();
            for id in &to_delete {
                db.execute(
                    "DELETE FROM recall_memory WHERE id = ?1",
                    rusqlite::params![id],
                )?;
            }
        }

        Ok(count)
    }

    fn dedup_archival_memory(&self) -> Result<usize> {
        // Phase 1: read records (lock held briefly)
        let records: Vec<(String, Vec<f32>)> = {
            let db = self.storage.conn();
            let mut stmt = db.prepare(
                "SELECT id, embedding FROM archival_memory ORDER BY created_at DESC LIMIT 5000",
            )?;
            let mut records = Vec::new();
            let rows = stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let embedding_bytes: Vec<u8> = row.get(1)?;
                let embedding = super::embedding::bytes_to_embedding(&embedding_bytes);
                Ok((id, embedding))
            })?;
            for row in rows {
                records.push(row?);
            }
            records
        }; // lock released

        // Phase 2: O(n²) computation (lock-free)
        let threshold = 0.90;
        let mut to_delete: HashSet<String> = HashSet::new();

        for i in 0..records.len() {
            if to_delete.contains(&records[i].0) {
                continue;
            }
            for j in (i + 1)..records.len() {
                if to_delete.contains(&records[j].0) {
                    continue;
                }
                let sim = cosine_similarity(&records[i].1, &records[j].1);
                if sim > threshold {
                    to_delete.insert(records[j].0.clone());
                }
            }
        }

        // Phase 3: batch delete (lock held briefly)
        let count = to_delete.len();
        if count > 0 {
            let db = self.storage.conn();
            for id in &to_delete {
                db.execute(
                    "DELETE FROM archival_memory WHERE id = ?1",
                    rusqlite::params![id],
                )?;
            }
        }

        Ok(count)
    }
}

#[derive(Debug)]
pub struct ConsolidationReport {
    pub deduped_recall: usize,
    pub deduped_archival: usize,
}
