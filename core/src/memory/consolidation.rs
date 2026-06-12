use anyhow::Result;
use std::sync::Arc;

use super::embedding::{EmbeddingModel, cosine_similarity};
use super::storage::Storage;

pub struct MemoryConsolidator {
    storage: Storage,
    #[allow(dead_code)]
    embedding_model: Arc<EmbeddingModel>,
}

impl MemoryConsolidator {
    pub fn new(storage: Storage, embedding_model: Arc<EmbeddingModel>) -> Self {
        Self {
            storage,
            embedding_model,
        }
    }

    pub fn consolidate(&self) -> Result<ConsolidationReport> {
        let deduped_recall = self.dedup_recall_memory()?;
        let deduped_archival = self.dedup_archival_memory()?;

        Ok(ConsolidationReport {
            deduped_recall,
            deduped_archival,
        })
    }

    fn dedup_recall_memory(&self) -> Result<usize> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT id, embedding FROM recall_memory ORDER BY created_at DESC LIMIT 5000",
        )?;

        let mut records: Vec<(String, Vec<f32>)> = Vec::new();
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let embedding_bytes: Vec<u8> = row.get(1)?;
            let embedding = super::embedding::bytes_to_embedding(&embedding_bytes);
            Ok((id, embedding))
        })?;

        for row in rows {
            records.push(row?);
        }

        let mut to_delete: Vec<String> = Vec::new();
        let threshold = 0.85;

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
                    to_delete.push(records[j].0.clone());
                }
            }
        }

        let count = to_delete.len();
        for id in &to_delete {
            db.execute(
                "DELETE FROM recall_memory WHERE id = ?1",
                rusqlite::params![id],
            )?;
        }

        Ok(count)
    }

    fn dedup_archival_memory(&self) -> Result<usize> {
        let db = self.storage.conn();
        let mut stmt = db.prepare(
            "SELECT id, embedding FROM archival_memory ORDER BY created_at DESC LIMIT 5000",
        )?;

        let mut records: Vec<(String, Vec<f32>)> = Vec::new();
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let embedding_bytes: Vec<u8> = row.get(1)?;
            let embedding = super::embedding::bytes_to_embedding(&embedding_bytes);
            Ok((id, embedding))
        })?;

        for row in rows {
            records.push(row?);
        }

        let mut to_delete: Vec<String> = Vec::new();
        let threshold = 0.90;

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
                    to_delete.push(records[j].0.clone());
                }
            }
        }

        let count = to_delete.len();
        for id in &to_delete {
            db.execute(
                "DELETE FROM archival_memory WHERE id = ?1",
                rusqlite::params![id],
            )?;
        }

        Ok(count)
    }
}

#[derive(Debug)]
pub struct ConsolidationReport {
    pub deduped_recall: usize,
    pub deduped_archival: usize,
}
