//! Periodic memory lifecycle — prune cold recall rows and promote high-value
//! records to archival storage.

use anyhow::Result;

use super::MemoryManager;

#[derive(Debug, Clone, Default)]
pub struct LifecycleReport {
    pub pruned: usize,
    pub promoted: usize,
}

impl MemoryManager {
    /// Run proactive forgetting and archival promotion.
    pub fn run_lifecycle(&self) -> Result<LifecycleReport> {
        let pruned = self.recall().prune_cold_memories(0.08, 0.35, 200)?;
        let promoted = self
            .recall()
            .promote_to_archival(self.archival(), 0.85, 50)?;
        Ok(LifecycleReport { pruned, promoted })
    }
}
