pub mod archival;
pub mod block;
pub mod consolidation;
pub mod embedding;
pub mod recall;
pub mod reflection;
pub mod salience;
pub mod storage;

use anyhow::Result;
use std::sync::Arc;

use self::archival::ArchivalMemory;
use self::block::CoreMemory;
use self::consolidation::MemoryConsolidator;
use self::embedding::EmbeddingModel;
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
}

impl MemoryManager {
    pub fn new(
        db_path: &str,
        embedding_model_name: &str,
        default_block_max_chars: usize,
        salience_config: Option<&SalienceConfig>,
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

    pub fn store_conversation(&self, role: &str, content: &str) -> Result<String> {
        // Use auto-rating (None = let the scorer decide)
        self.recall.store(&self.session_id, role, content, None)
    }

    pub fn search_conversation(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        let results = self.recall.search(query, top_k)?;
        // Reinforcement: bump strength for retrieved memories
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        let _ = self.recall.bump_strength_batch(&ids);
        Ok(results)
    }

    /// Search conversation and return only results above a minimum score threshold.
    /// Falls back to unfiltered search when scored search is unavailable (no embedding).
    pub fn search_conversation_filtered(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<ScoredRecord>> {
        match self.recall.search_scored(query, top_k) {
            Ok(scored) => {
                let filtered: Vec<_> = scored
                    .into_iter()
                    .filter(|s| s.total_score >= min_score)
                    .collect();
                let ids: Vec<&str> = filtered.iter().map(|r| r.id.as_str()).collect();
                let _ = self.recall.bump_strength_batch(&ids);
                Ok(filtered)
            }
            Err(_) => {
                // Fallback: unfiltered keyword search, wrap as ScoredRecord
                let results = self.recall.search(query, top_k)?;
                let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
                let _ = self.recall.bump_strength_batch(&ids);
                Ok(results
                    .into_iter()
                    .map(|r| ScoredRecord {
                        id: r.id,
                        content: r.content,
                        total_score: 1.0,
                        semantic_score: 0.0,
                        recall_score: 0.0,
                        importance: r.importance,
                        memory_strength: r.memory_strength,
                        hours_since_created: 0.0,
                        category: r.category,
                    })
                    .collect())
            }
        }
    }

    pub fn consolidate(&self) -> Result<consolidation::ConsolidationReport> {
        self.consolidator.consolidate()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn new_session(&mut self) {
        self.session_id = uuid::Uuid::new_v4().to_string();
    }

    /// Prune cold memories with low recall and importance.
    pub fn prune(&self, min_score: f32, min_importance: f32, max: usize) -> Result<usize> {
        self.recall
            .prune_cold_memories(min_score, min_importance, max)
    }

    /// Promote high-importance old memories to archival storage.
    pub fn promote_to_archival(&self, min_importance: f32, max: usize) -> Result<usize> {
        self.recall
            .promote_to_archival(&self.archival, min_importance, max)
    }

    /// Get memory stats.
    pub fn stats(&self) -> Result<recall::MemoryStats> {
        self.recall.stats()
    }
}
