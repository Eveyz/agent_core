pub mod archival;
pub mod block;
pub mod bm25;
pub mod consolidation;
pub mod embedding;
pub mod hnsw;
pub mod recall;
pub mod reflection;
pub mod rrf;
pub mod salience;
pub mod storage;

use anyhow::{Context, Result};
use std::sync::Arc;

use self::archival::ArchivalMemory;
use self::block::CoreMemory;
use self::bm25::BM25Index;
use self::consolidation::MemoryConsolidator;
use self::embedding::EmbeddingModel;
use self::hnsw::HNSWIndex;
use self::recall::RecallMemory;
use self::storage::Storage;

pub use self::recall::MemoryStats;
pub use self::salience::{MemoryCategory, SalienceConfig, SalienceScorer, ScoredRecord};
pub struct MemoryManager {
    core: CoreMemory,
    recall: RecallMemory,
    archival: ArchivalMemory,
    consolidator: MemoryConsolidator,
    session_id: String,
    bm25: Option<BM25Index>,
    hnsw: Option<HNSWIndex>,
}

impl MemoryManager {
    pub fn new(
        db_path: &str,
        embedding_model_name: &str,
        default_block_max_chars: usize,
        salience_config: Option<&SalienceConfig>,
    ) -> Result<Self> {
        Self::new_with_indexes(db_path, embedding_model_name, default_block_max_chars, salience_config, None, None)
    }

    /// Full constructor with optional BM25 and HNSW indexes.
    pub fn new_with_indexes(
        db_path: &str,
        embedding_model_name: &str,
        default_block_max_chars: usize,
        salience_config: Option<&SalienceConfig>,
        bm25: Option<BM25Index>,
        hnsw: Option<HNSWIndex>,
    ) -> Result<Self> {
        let storage = Storage::new(db_path)?;
        let embedding_model = Arc::new(EmbeddingModel::new(embedding_model_name)?);

        let core = CoreMemory::new(storage.clone(), default_block_max_chars)?;
        let recall = RecallMemory::new(storage.clone(), embedding_model.clone());
        let recall = if let Some(cfg) = salience_config {
            recall.with_config(cfg.clone())
        } else {
            recall
        };

        let archival = ArchivalMemory::new(storage.clone(), embedding_model.clone());
        let consolidator = MemoryConsolidator::new(storage, embedding_model);

        let session_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            core,
            recall,
            archival,
            consolidator,
            session_id,
            bm25,
            hnsw,
        })
    }

    /// Create a MemoryManager without embedding model.
    /// Conversation search falls back to keyword matching.
    /// Core memory (manual notes) works normally.
    pub fn without_embedding(
        db_path: &str,
        default_block_max_chars: usize,
        salience_config: Option<&SalienceConfig>,
    ) -> Result<Self> {
        Self::without_embedding_with_indexes(db_path, default_block_max_chars, salience_config, None, None)
    }

    /// Without-embedding constructor with optional BM25 and HNSW indexes.
    pub fn without_embedding_with_indexes(
        db_path: &str,
        default_block_max_chars: usize,
        salience_config: Option<&SalienceConfig>,
        bm25: Option<BM25Index>,
        hnsw: Option<HNSWIndex>,
    ) -> Result<Self> {
        let storage = Storage::new(db_path)?;

        let core = CoreMemory::new(storage.clone(), default_block_max_chars)?;
        let recall = RecallMemory::without_embedding(storage.clone());
        let recall = if let Some(cfg) = salience_config {
            recall.with_config(cfg.clone())
        } else {
            recall
        };
        let archival = ArchivalMemory::without_embedding(storage.clone());
        let consolidator = MemoryConsolidator::without_embedding(storage);

        let session_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            core,
            recall,
            archival,
            consolidator,
            session_id,
            bm25,
            hnsw,
        })
    }

    pub fn core(&self) -> &CoreMemory {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut CoreMemory {
        &mut self.core
    }

    pub fn recall(&self) -> &RecallMemory {
        &self.recall
    }

    pub fn archival(&self) -> &ArchivalMemory {
        &self.archival
    }

    /// Borrow the BM25 index if enabled.
    pub fn bm25(&self) -> Option<&BM25Index> {
        self.bm25.as_ref()
    }

    /// Borrow the HNSW index if enabled.
    pub fn hnsw(&self) -> Option<&HNSWIndex> {
        self.hnsw.as_ref()
    }

    /// Inject BM25 index after construction.
    pub fn set_bm25(&mut self, bm25: BM25Index) {
        self.bm25 = Some(bm25);
    }

    /// Inject HNSW index after construction.
    pub fn set_hnsw(&mut self, hnsw: HNSWIndex) {
        self.hnsw = Some(hnsw);
    }

    pub fn store_conversation(&self, role: &str, content: &str) -> Result<String> {
        // Use auto-rating (None = let the scorer decide)
        let id = self.recall.store(&self.session_id, role, content, None)?;

        // Sync to BM25 index if enabled
        if let Some(ref bm25) = self.bm25 {
            let _ = bm25.insert(&id, content);
        }

        // Sync to HNSW index if enabled (fallback pool for immutable index)
        if let Some(ref hnsw) = self.hnsw {
            let embedding = if let Some(ref model) = self.recall.embedding_model() {
                model.embed_single(content).unwrap_or_default()
            } else {
                Vec::new()
            };
            if !embedding.is_empty() {
                let normalized = hnsw::normalize_embedding(&embedding);
                hnsw.add_fallback(id.clone(), normalized);
            }
        }

        Ok(id)
    }

    pub fn search_conversation(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        // Use hybrid pipeline if both BM25 and HNSW are available
        if self.bm25.is_some() && self.hnsw.is_some() {
            return self.search_conversation_hybrid(query, top_k);
        }
        let results = self.recall.search(query, top_k)?;
        // Reinforcement: bump strength for retrieved memories
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        let _ = self.recall.bump_strength_batch(&ids);
        Ok(results)
    }

    /// Search with a pre-computed query embedding (lock-friendly).
    ///
    /// Unlike `search_conversation()` which embeds the query internally
    /// (blocking the lock for 10-50ms per call), this accepts an already-
    /// computed embedding so the hot path Inside the MemoryManager lock
    /// is purely I/O and CPU-light ranking.
    pub fn search_conversation_precomputed(
        &self,
        query_emb: &[f32],
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let normalized = hnsw::normalize_embedding(query_emb);
        if self.bm25.is_some() && self.hnsw.is_some() {
            return self.search_conversation_hybrid_precomputed(query_emb, query, top_k);
        }
        // Fallback: pass through to recall's vector search
        self.recall.search_by_vector(query_emb, query, top_k)
    }

    /// Pure BM25 keyword search — no embedding model needed.
    ///
    /// Uses the tantivy BM25 index for candidate retrieval, loads full
    /// records from SQLite, and returns them in BM25 relevance order.
    /// Falls back to SQLite FTS5 when BM25 is not available.
    pub fn search_conversation_bm25(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let bm25 = match self.bm25.as_ref() {
            Some(b) => b,
            None => return self.recall.search_by_keyword(query, top_k),
        };

        // Phase 1: BM25 candidate retrieval
        let bm25_ids = bm25.search(query, 150).unwrap_or_default();
        if bm25_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: Load records from SQLite by ID, preserve BM25 order
        let db = self.recall.storage_conn();
        let placeholders = bm25_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT id, session_id, role, content, embedding, importance, \
             COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
             last_accessed_at, COALESCE(category, 'Conversation'), created_at \
             FROM recall_memory WHERE id IN ({})",
            placeholders
        );

        let params: Vec<String> = bm25_ids.clone();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let mut stmt = db
            .prepare_cached(&sql)
            .context("failed to prepare bm25 search query")?;
        let rows = stmt.query_map(
            param_refs.as_slice(),
            |row| recall::RecallMemory::parse_recall_row_static(row),
        )?;

        // Collect records, sort into BM25 order, truncate
        let rank: std::collections::HashMap<String, usize> = bm25_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        let mut results: Vec<recall::RecallRecord> = Vec::new();
        for row in rows {
            results.push(row?);
        }
        results.sort_by_key(|r| rank.get(&r.id).copied().unwrap_or(usize::MAX));
        results.truncate(top_k);

        Ok(results)
    }

    /// BM25 + Salience reranking — no embedding model needed.
    ///
    /// Pipeline:
    ///   BM25 top 150 → RRF from rank → Salience decay → sort → top_k → bump
    pub fn search_conversation_bm25_with_salience(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let bm25 = match self.bm25.as_ref() {
            Some(b) => b,
            None => return self.recall.search_by_keyword(query, top_k),
        };

        let bm25_ids = bm25.search(query, 150).unwrap_or_default();
        if bm25_ids.is_empty() {
            return Ok(Vec::new());
        }

        // RRF from BM25 rank only
        let rrf_map: std::collections::HashMap<String, f32> = bm25_ids
            .iter()
            .enumerate()
            .map(|(rank, id)| (id.clone(), 60.0 / (60.0 + rank as f32 + 1.0)))
            .collect();

        let now = chrono::Utc::now();
        let scorer = self.recall.scorer();

        let placeholders = bm25_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT id, session_id, role, content, embedding, importance, \
             COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
             last_accessed_at, COALESCE(category, 'Conversation'), created_at \
             FROM recall_memory WHERE id IN ({})",
            placeholders
        );

        let params: Vec<String> = bm25_ids.clone();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        // Block-scope db so it drops before bump_strength_batch
        // (which acquires its own storage_conn — same mutex = deadlock).
        let (results, bump_ids) = {
            let db = self.recall.storage_conn();
            let mut stmt = db
                .prepare_cached(&sql)
                .context("failed to prepare bm25+salience query")?;
            let rows = stmt.query_map(
                param_refs.as_slice(),
                |row| recall::RecallMemory::parse_recall_row_static(row),
            )?;

            let mut scored: Vec<(f32, recall::RecallRecord)> = Vec::new();

            for row in rows {
                let record = row?;
                let s_rrf = rrf_map.get(&record.id).copied().unwrap_or(0.1);

                let hours_since = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .ok()
                    .map(|dt| {
                        (now - dt.with_timezone(&chrono::Utc))
                            .num_hours()
                            .max(0) as f32
                    })
                    .unwrap_or(0.0);

                let score = scorer.hybrid_score(
                    s_rrf,
                    hours_since,
                    record.importance,
                    record.category,
                );
                scored.push((score, record));
            }

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);

            let results: Vec<recall::RecallRecord> =
                scored.into_iter().map(|(_, r)| r).collect();
            let bump_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
            (results, bump_ids)
        }; // db + stmt + rows dropped here

        let id_refs: Vec<&str> = bump_ids.iter().map(|s| s.as_str()).collect();
        let _ = self.recall.bump_strength_batch(&id_refs);

        Ok(results)
    }

    /// Hybrid search: BM25 + HNSW → RRF → Salience (multiplicative decay).
    fn search_conversation_hybrid(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let bm25 = self.bm25.as_ref().unwrap();
        let hnsw = self.hnsw.as_ref().unwrap();

        // Phase 1: Dual recall
        let bm25_ids = bm25.search(query, 150).unwrap_or_default();

        let hnsw_ids = if let Some(ref model) = self.recall.embedding_model() {
            let query_emb = model.embed_single(query).unwrap_or_default();
            let normalized = hnsw::normalize_embedding(&query_emb);
            hnsw.search(&normalized, 150)
        } else {
            Vec::new()
        };

        // Phase 2: RRF fusion
        let lists = vec![bm25_ids, hnsw_ids];
        let fused = rrf::fuse_normalized(&lists, 60);

        // Phase 3: Truncate to 100 candidates
        let candidates: Vec<String> = fused
            .into_iter()
            .take(100)
            .map(|(id, _)| id)
            .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 4-5: Load from SQLite + score (block-scoped for db drop)
        let (results, bump_ids) = {
            let db = self.recall.storage_conn();
            let placeholders = candidates
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");

            let sql = format!(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
                 last_accessed_at, COALESCE(category, 'Conversation'), created_at \
                 FROM recall_memory WHERE id IN ({})",
                placeholders
            );

            let params: Vec<String> = candidates.clone();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();

            let mut stmt = db.prepare(&sql).context("failed to prepare hybrid search query")?;
            let rows = stmt.query_map(param_refs.as_slice(), |row| recall::RecallMemory::parse_recall_row_static(row))?;

            let rrf_map: std::collections::HashMap<String, f32> = candidates
                .iter()
                .enumerate()
                .map(|(rank, id)| (id.clone(), 60.0 / (60.0 + rank as f32 + 1.0)))
                .collect();

            let now = chrono::Utc::now();
            let mut scored: Vec<(f32, recall::RecallRecord)> = Vec::new();

            for row in rows {
                let record = row?;
                let s_rrf = rrf_map.get(&record.id).copied().unwrap_or(0.1);
                let hours_since = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .ok()
                    .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_hours().max(0) as f32)
                    .unwrap_or(0.0);
                let score = self.recall.scorer().hybrid_score(
                    s_rrf,
                    hours_since,
                    record.importance,
                    record.category,
                );
                scored.push((score, record));
            }

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);

            let results: Vec<recall::RecallRecord> = scored.into_iter().map(|(_, r)| r).collect();
            let bump_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
            (results, bump_ids)
        }; // db dropped — bump_strength_batch can now safely acquire it

        let id_refs: Vec<&str> = bump_ids.iter().map(|s| s.as_str()).collect();
        let _ = self.recall.bump_strength_batch(&id_refs);

        Ok(results)
    }

    /// Same as `search_conversation_hybrid` but accepts a pre-computed embedding.
    /// Avoids the 10-50ms embedding call inside the lock.
    fn search_conversation_hybrid_precomputed(
        &self,
        query_emb: &[f32],
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let bm25 = self.bm25.as_ref().unwrap();
        let hnsw = self.hnsw.as_ref().unwrap();

        let bm25_ids = bm25.search(query, 150).unwrap_or_default();
        let normalized = hnsw::normalize_embedding(query_emb);
        let hnsw_ids = hnsw.search(&normalized, 150);

        let lists = vec![bm25_ids, hnsw_ids];
        let fused = rrf::fuse_normalized(&lists, 60);

        let candidates: Vec<String> = fused
            .into_iter()
            .take(100)
            .map(|(id, _)| id)
            .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let (results, bump_ids) = {
            let db = self.recall.storage_conn();
            let placeholders = candidates
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");

            let sql = format!(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
                 last_accessed_at, COALESCE(category, 'Conversation'), created_at \
                 FROM recall_memory WHERE id IN ({})",
                placeholders
            );

            let params: Vec<String> = candidates.clone();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();

            let mut stmt = db.prepare(&sql)?;
            let rows = stmt.query_map(param_refs.as_slice(), |row| recall::RecallMemory::parse_recall_row_static(row))?;

            let rrf_map: std::collections::HashMap<String, f32> = candidates
                .iter()
                .enumerate()
                .map(|(rank, id)| (id.clone(), 60.0 / (60.0 + rank as f32 + 1.0)))
                .collect();

            let now = chrono::Utc::now();
            let mut scored: Vec<(f32, recall::RecallRecord)> = Vec::new();

            for row in rows {
                let record = row?;
                let s_rrf = rrf_map.get(&record.id).copied().unwrap_or(0.1);
                let hours_since = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .ok()
                    .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_hours().max(0) as f32)
                    .unwrap_or(0.0);
                let score = self.recall.scorer().hybrid_score(
                    s_rrf,
                    hours_since,
                    record.importance,
                    record.category,
                );
                scored.push((score, record));
            }

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);

            let results: Vec<recall::RecallRecord> = scored.into_iter().map(|(_, r)| r).collect();
            let bump_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
            (results, bump_ids)
        }; // db dropped — safe for bump_strength_batch

        let id_refs: Vec<&str> = bump_ids.iter().map(|s| s.as_str()).collect();
        let _ = self.recall.bump_strength_batch(&id_refs);

        Ok(results)
    }

    /// Store a piece of knowledge in archival memory.
    pub fn store_archival(&self, content: &str, metadata: Option<&str>) -> Result<String> {
        self.archival.insert(content, metadata)
    }

    /// Search archival memory.
    pub fn search_archival(&self, query: &str, top_k: usize) -> Result<Vec<archival::ArchivalRecord>> {
        self.archival.search(query, top_k)
    }

    /// Delete from archival memory.
    pub fn delete_archival(&self, id: &str) -> Result<bool> {
        self.archival.delete(id)
    }

    /// Run memory consolidation (dedup). Call periodically.
    pub fn consolidate(&self) -> Result<consolidation::ConsolidationReport> {
        self.consolidator.consolidate()
    }

    /// Clone the consolidator (lock-free reference).
    pub fn consolidator_clone(&self) -> MemoryConsolidator {
        self.consolidator.clone()
    }

    /// Access the core memory block manager.
    pub fn core_memory(&self) -> &CoreMemory {
        &self.core
    }

    /// Get current session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Start a new session (generates a fresh session ID).
    pub fn new_session(&mut self) {
        self.session_id = uuid::Uuid::new_v4().to_string();
    }

    /// Get memory stats.
    pub fn stats(&self) -> Result<recall::MemoryStats> {
        self.recall.stats()
    }
    pub fn search_conversation_filtered(
        &self,
        query: &str,
        top_k: usize,
        role_filter: Option<&str>,
    ) -> Result<Vec<recall::RecallRecord>> {
        let bm25 = match self.bm25.as_ref() {
            Some(b) => b,
            None => return self.recall.search_by_keyword(query, top_k),
        };

        let bm25_ids = bm25.search(query, 150).unwrap_or_default();
        if bm25_ids.is_empty() {
            return Ok(Vec::new());
        }

        let db = self.recall.storage_conn();
        let placeholders = bm25_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = if role_filter.is_some() {
            format!(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
                 last_accessed_at, COALESCE(category, 'Conversation'), created_at \
                 FROM recall_memory WHERE id IN ({}) AND role = ?{}",
                placeholders,
                bm25_ids.len() + 1
            )
        } else {
            format!(
                "SELECT id, session_id, role, content, embedding, importance, \
                 COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), \
                 last_accessed_at, COALESCE(category, 'Conversation'), created_at \
                 FROM recall_memory WHERE id IN ({})",
                placeholders
            )
        };

        let mut params: Vec<String> = bm25_ids.clone();
        if let Some(role) = role_filter {
            params.push(role.to_string());
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map(
            param_refs.as_slice(),
            |row| recall::RecallMemory::parse_recall_row_static(row),
        )?;

        let rank: std::collections::HashMap<String, usize> = bm25_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        let mut results: Vec<recall::RecallRecord> = Vec::new();
        for row in rows {
            results.push(row?);
        }
        results.sort_by_key(|r| rank.get(&r.id).copied().unwrap_or(usize::MAX));
        results.truncate(top_k);

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_memory() -> (TempDir, MemoryManager) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let memory = MemoryManager::without_embedding(
            db_path.to_str().unwrap(),
            4096,
            None,
        ).unwrap();
        (dir, memory)
    }

    #[test]
    fn test_store_and_search_conversation() {
        let (_dir, memory) = setup_test_memory();
        memory.store_conversation("user", "Hello world").unwrap();
        memory.store_conversation("assistant", "Hi there").unwrap();

        let results = memory.search_conversation("hello", 5).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_store_and_search_archival() {
        let (_dir, memory) = setup_test_memory();
        memory.store_archival("Rust is a systems programming language", None).unwrap();
        let results = memory.search_archival("Rust", 5).unwrap();
        assert!(!results.is_empty());
    }
}
