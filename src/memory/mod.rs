pub mod archival;
pub mod block;
pub mod consolidation;
pub mod embedding;
pub mod recall;
pub mod storage;

use anyhow::Result;
use std::sync::Arc;

use self::archival::ArchivalMemory;
use self::block::CoreMemory;
use self::consolidation::MemoryConsolidator;
use self::embedding::EmbeddingModel;
use self::recall::RecallMemory;
use self::storage::Storage;

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
    ) -> Result<Self> {
        let storage = Storage::new(db_path)?;
        let embedding_model = Arc::new(EmbeddingModel::new(embedding_model_name)?);

        let core = CoreMemory::new(storage.clone(), default_block_max_chars)?;
        let recall = RecallMemory::new(storage.clone(), embedding_model.clone());
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
        self.recall.store(&self.session_id, role, content, 0.5)
    }

    pub fn search_conversation(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<recall::RecallRecord>> {
        self.recall.search(query, top_k)
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
}
