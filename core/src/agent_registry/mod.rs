//! User-defined agent registry (PLAN-0009).
//!
//! This module provides persistence and runtime-component construction for
//! custom agents. It is deliberately decoupled from the workflow engine:
//! the registry only knows how to store/fetch [`AgentDef`]s and turn them into
//! the runtime pieces (`SubagentConfig`, `PermissionConfig`, `ModelConfig`)
//! that an executor needs.
//!
//! The per-agent memory store ([`AgentMemoryStore`]) and history store live
//! alongside the definitions so all three share the same `Storage`.

pub mod definition;
pub mod history;
pub mod memory;
pub mod runner;
pub mod skill_drafts;

pub use definition::{
    AgentDef, AgentDefUpdate, build_model_config, build_permission_config, build_subagent_config,
    create, delete, get, list, update,
};
pub use history::{
    AgentHistoryEntry, count as history_count, get_recent, list as history_list,
    record as history_record,
};
pub use memory::{AgentMemoryRecord, AgentMemoryStore};
pub use runner::{CustomAgentInvocation, CustomAgentRunResult, CustomAgentRunner};
pub use skill_drafts::{
    DraftGenerationResult, SkillDraft, approve_draft, generate_drafts, get_draft, list_drafts,
    reject_draft,
};

use crate::config::Config;
use crate::memory::storage::Storage;
use crate::permission::PermissionConfig;
use crate::subagent::SubagentConfig;

/// Coordinator that owns the shared [`Storage`] and exposes CRUD + builder
/// helpers for custom agents.
///
/// Cloning is cheap — it only clones the `Arc` around `Storage`.
#[derive(Clone)]
pub struct AgentRegistry {
    storage: Storage,
}

impl AgentRegistry {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    // ── CRUD delegates ──────────────────────────────────────────────

    pub fn create(&self, def: &AgentDef) -> anyhow::Result<AgentDef> {
        create(&self.storage, def)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<AgentDef> {
        get(&self.storage, id)
    }

    pub fn list(&self) -> anyhow::Result<Vec<AgentDef>> {
        list(&self.storage)
    }

    pub fn update(&self, id: &str, updates: &AgentDefUpdate) -> anyhow::Result<AgentDef> {
        update(&self.storage, id, updates)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        delete(&self.storage, id)
    }

    // ── Runtime component builders ──────────────────────────────────

    /// Build a [`SubagentConfig`] from an [`AgentDef`].
    pub fn build_subagent_config(&self, def: &AgentDef) -> SubagentConfig {
        build_subagent_config(def)
    }

    /// Build a [`PermissionConfig`] overlaying the agent's mode/rules on `base`.
    pub fn build_permission_config(
        &self,
        def: &AgentDef,
        base: &PermissionConfig,
    ) -> PermissionConfig {
        build_permission_config(def, base)
    }

    /// Build a [`crate::config::ModelConfig`] from an [`AgentDef`], falling
    /// back to the config's default model.
    pub fn build_model_config(
        &self,
        def: &AgentDef,
        config: &Config,
    ) -> crate::config::ModelConfig {
        build_model_config(def, config)
    }
}
