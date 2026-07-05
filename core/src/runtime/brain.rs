//! Brain — the reusable, shared core of the agent.
//!
//! A Brain is constructed once (from config) and shared across all Runs.
//! It holds:
//! - LLM client configuration
//! - Persistent tool registry (tools registered once, cloned per Run)
//! - Memory manager (shared SQLite + embeddings)
//! - Skill manager (shared catalog)
//! - Runtime overrides (temperature, max_tokens, tool_execution_mode)
//!
//! Per-request state lives in [`crate::runtime::Run`].

use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::sync::Arc;

use crate::client::OpenAIClient;
use crate::config::{Config, ModelConfig};
use crate::error_recovery::RecoveryEngine;
use crate::hooks::HookRegistry;
use crate::memory::MemoryManager;
use crate::memory::reflection::ReflectionDaemon;
use crate::mode::AgentMode;
use crate::permission::{PermissionConfig, PermissionPolicy};
use crate::prompt;
use crate::reflector::Reflector;
use crate::skills::SkillManager;
use crate::todo::TodoList;
use crate::tools::ToolRegistry;
use crate::types::ToolExecutionMode;

/// The reusable "brain" shared by all Runs.
///
/// Cloneable via `Arc` — each Run holds an `Arc<Brain>`.
/// Contains only immutable/shared state; per-request state lives in [`crate::runtime::Run`].
#[derive(Clone)]
pub struct Brain {
    /// The full configuration (models, permissions, memory, mcp).
    pub config: Config,
    /// Shared memory manager (if memory is enabled).
    pub memory: Option<Arc<Mutex<MemoryManager>>>,
    /// Background reflection daemon (Deep mode only, if reflection_model is set).
    pub reflection_daemon: Option<Arc<ReflectionDaemon>>,
    /// Shared skill manager (if skills are enabled).
    pub skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// Shared todo list (planning state, visible across Runs).
    pub todo_list: Arc<Mutex<TodoList>>,
    /// Offline reflector (analyzes Run event logs after completion).
    pub reflector: Option<Reflector>,
    /// The currently active model name (e.g. "openai/gpt-4o").
    /// Switching the model updates this; new Runs use the new model.
    current_model_name: String,
    /// The current agent mode (Ask / Plan / Build).
    /// Mode changes take effect on the next Run — existing Runs are unaffected.
    /// Wrapped in Arc<Mutex<>> to satisfy Clone (required by #[derive(Clone)]).
    current_mode: Arc<Mutex<AgentMode>>,
    /// Shared hook registry. When set, Runs use this instead of building fresh.
    /// This allows the Agent wrapper to register hooks that fire during Run execution.
    pub hook_registry: Arc<Mutex<HookRegistry>>,
}

impl Brain {
    /// Build a Brain from a loaded Config.
    pub fn from_config(config: Config) -> Result<Self> {
        let memory = Self::build_memory(&config);
        let reflection_daemon = Self::build_reflection_daemon(&config, &memory);
        let skill_manager = Self::build_skill_manager(&config)?;
        let reflector = Self::build_reflector(&config);

        let current_model_name = config.default_model.clone();

        Ok(Self {
            config,
            memory,
            reflection_daemon,
            skill_manager,
            todo_list: Arc::new(Mutex::new(TodoList::new())),
            reflector,
            current_model_name,
            current_mode: Arc::new(Mutex::new(AgentMode::default())),
            hook_registry: Arc::new(Mutex::new(HookRegistry::new())),
        })
    }

    /// Build a Brain by loading config from a file path.
    pub fn load_config(path: &str) -> Result<Self> {
        let config =
            Config::load(path).with_context(|| format!("failed to load config: {path}"))?;
        Self::from_config(config)
    }

    fn build_memory(config: &Config) -> Option<Arc<Mutex<MemoryManager>>> {
        let mem_config = config.memory.as_ref();

        // Determine memory mode — stateless mode skips memory entirely
        let mode = mem_config
            .map(|m| crate::config::MemoryMode::from_str(&m.mode))
            .unwrap_or_default();
        if mode == crate::config::MemoryMode::Stateless {
            tracing::info!("memory mode is stateless — skipping memory initialization");
            return None;
        }

        let default_db_path = crate::paths::get_memory_db_path().to_string_lossy().into_owned();
        let db_path = mem_config
            .map(|m| m.db_path.as_str())
            .unwrap_or(&default_db_path);
        let block_max_chars = mem_config
            .map(|m| m.default_block_max_chars)
            .unwrap_or(2000);
        let embedding_enabled = mem_config
            .map(|m| m.embedding_enabled)
            .unwrap_or(false);
        let salience_config = mem_config
            .and_then(|m| m.salience.as_ref());

        let result = if embedding_enabled {
            let embedding_model = mem_config
                .map(|m| m.embedding_model.as_str())
                .unwrap_or("BAAI/bge-small-en-v1.5");
            tracing::info!("initializing memory (mode={}) with embedding model: {embedding_model} (lazy — loaded on first use)", mode.as_str());
            MemoryManager::new(db_path, embedding_model, block_max_chars, salience_config)
        } else {
            tracing::info!("initializing memory (mode={}) without embedding (keyword search mode)", mode.as_str());
            MemoryManager::without_embedding(db_path, block_max_chars, salience_config)
        };

        match result {
            Ok(mut m) => {
                // BM25: on by default (can be disabled with bm25.enabled = false)
                let bm25_enabled = mem_config
                    .and_then(|c| c.bm25.as_ref())
                    .map(|b| b.enabled)
                    .unwrap_or(true);
                if bm25_enabled {
                    tracing::info!("building BM25 index...");
                    match Self::build_bm25_index(&m) {
                        Ok(bm25) => m.set_bm25(bm25),
                        Err(e) => tracing::warn!("failed to build BM25 index: {e}"),
                    }
                }

                // HNSW: on by default when embedding is enabled
                let hnsw_enabled = mem_config
                    .and_then(|c| c.hnsw.as_ref())
                    .map(|h| h.enabled)
                    .unwrap_or(true);
                if hnsw_enabled && embedding_enabled {
                    tracing::info!("building HNSW index...");
                    match Self::build_hnsw_index(&m) {
                        Ok(hnsw) => m.set_hnsw(hnsw),
                        Err(e) => tracing::warn!("failed to build HNSW index: {e}"),
                    }
                }

                Some(Arc::new(Mutex::new(m)))
            }
            Err(e) => {
                tracing::warn!("failed to initialize memory system: {e}; continuing without memory");
                None
            }
        }
    }

    /// Build BM25 index from existing recall records.
    fn build_bm25_index(mgr: &MemoryManager) -> anyhow::Result<crate::memory::bm25::BM25Index> {
        use crate::memory::bm25::BM25Index;
        let db = mgr.recall().storage_conn();
        let mut stmt = db.prepare("SELECT id, content FROM recall_memory")?;
        let records: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        drop(db);
        BM25Index::from_records(&records)
    }

    /// Build HNSW index from existing recall records with embeddings.
    fn build_hnsw_index(mgr: &MemoryManager) -> anyhow::Result<crate::memory::hnsw::HNSWIndex> {
        use crate::memory::hnsw::{HNSWIndex, normalize_embedding};
        use crate::memory::embedding::bytes_to_embedding;
        let db = mgr.recall().storage_conn();
        let mut stmt = db
            .prepare("SELECT id, embedding FROM recall_memory WHERE embedding IS NOT NULL AND length(embedding) > 0")?;
        let records: Vec<(String, Vec<f32>)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let emb_bytes: Vec<u8> = row.get(1)?;
                Ok((id, emb_bytes))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, bytes)| {
                let emb = bytes_to_embedding(&bytes);
                (id, normalize_embedding(&emb))
            })
            .collect();
        drop(stmt);
        drop(db);
        HNSWIndex::from_records(records)
    }

    fn build_reflection_daemon(
        config: &Config,
        memory: &Option<Arc<Mutex<MemoryManager>>>,
    ) -> Option<Arc<ReflectionDaemon>> {
        let mem_config = config.memory.as_ref()?;
        let mode = crate::config::MemoryMode::from_str(&mem_config.mode);
        if mode != crate::config::MemoryMode::Deep {
            return None;
        }

        let reflection = mem_config.reflection.as_ref()?;
        let model_key = reflection.reflection_model.as_ref()?;
        let model_config = config.get_model(model_key)?;
        let memory = memory.clone()?;

        let client = OpenAIClient::new(model_config.clone());
        let trigger_count = reflection.trigger_message_count;

        tracing::info!(
            "reflection daemon enabled: model={}, trigger={} messages",
            model_key,
            trigger_count
        );

        let daemon = ReflectionDaemon::spawn(client, memory, trigger_count);
        Some(Arc::new(daemon))
    }

    fn build_skill_manager(_config: &Config) -> Result<Option<Arc<Mutex<SkillManager>>>> {
        let mut mgr = SkillManager::with_defaults();
        match mgr.scan() {
            Ok(count) => {
                if count > 0 {
                    tracing::info!("skill manager: {} skills discovered", count);
                }
                Ok(Some(Arc::new(Mutex::new(mgr))))
            }
            Err(e) => {
                tracing::warn!("skill manager scan failed: {e}; continuing without skills");
                Ok(None)
            }
        }
    }

    fn build_reflector(config: &Config) -> Option<Reflector> {
        if !config.reflector_enabled {
            return None;
        }
        let skills_dir = crate::paths::get_skills_dir();
        let _ = std::fs::create_dir_all(&skills_dir);
        Some(Reflector::new(skills_dir))
    }

    /// Get the currently active model's full config.
    pub fn current_model_config(&self) -> Result<ModelConfig> {
        self.config
            .get_model(&self.current_model_name)
            .cloned()
            .with_context(|| {
                format!(
                    "current model '{}' not found in config",
                    self.current_model_name
                )
            })
    }

    /// The currently active model name.
    pub fn current_model_name(&self) -> &str {
        &self.current_model_name
    }

    /// Update the brain's configuration in memory.
    pub fn update_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Switch the active model by name (must exist in config).
    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        if self.config.get_model(name).is_none() {
            anyhow::bail!("model '{name}' not found in config");
        }
        self.current_model_name = name.to_string();
        Ok(())
    }

    /// The current agent mode. New Runs inherit this mode.
    pub fn mode(&self) -> AgentMode {
        *self.current_mode.lock()
    }

    /// Set the agent mode. Takes effect on the NEXT Run — existing Runs
    /// keep their mode.
    pub fn set_mode(&self, mode: AgentMode) {
        *self.current_mode.lock() = mode;
    }

    /// Build an OpenAIClient for the current model (with fallback chain).
    ///
    /// Each Run calls this to get its own client instance. The client is
    /// cheap to construct (shares an HTTP connection pool internally).
    pub fn build_client(&self) -> Result<OpenAIClient> {
        let model_config = self.current_model_config()?;

        // Build fallback chain (up to 3 levels deep).
        let mut current = model_config.clone();
        let mut fallbacks = Vec::new();
        for _ in 0..3 {
            if let Some(ref fallback_name) = current.fallback_model {
                if let Some(fallback_cfg) = self.config.get_model(fallback_name) {
                    fallbacks.push(fallback_cfg.clone());
                    current = fallback_cfg.clone();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let mut client_opt: Option<OpenAIClient> = None;
        for fallback_cfg in fallbacks.into_iter().rev() {
            if let Some(child) = client_opt {
                client_opt = Some(OpenAIClient::with_fallback(fallback_cfg, Some(child)));
            } else {
                client_opt = Some(OpenAIClient::new(fallback_cfg));
            }
        }

        let client = if let Some(child) = client_opt {
            OpenAIClient::with_fallback(model_config, Some(child))
        } else {
            OpenAIClient::new(model_config)
        };

        Ok(client)
    }

    /// Build an `OpenAIClient` for a specific purpose (`"btw"` / `"learn"`).
    ///
    /// Uses the purpose-specific lightweight model when configured
    /// (`Config.btw_model` / `Config.learn_model`), falling back to the
    /// current active model. Unlike [`build_client`], this does NOT build a
    /// fallback chain — side-channel queries are single-shot and cheap.
    pub fn build_client_for(&self, purpose: &str) -> Result<OpenAIClient> {
        let name = match purpose {
            "btw" => self
                .config
                .btw_model
                .as_deref()
                .unwrap_or(self.current_model_name()),
            "learn" => self
                .config
                .learn_model
                .as_deref()
                .unwrap_or(self.current_model_name()),
            _ => self.current_model_name(),
        };
        let model_config = self
            .config
            .get_model(name)
            .with_context(|| format!("model '{name}' not found in config"))?
            .clone();
        Ok(OpenAIClient::new(model_config))
    }

    /// Build a fresh ToolRegistry for a Run, filtered by mode.
    ///
    /// Each Run gets its own registry so MCP tools, memory tools, and
    /// subagent tools can be registered per-Run without interference.
    ///
    /// Tools are removed based on [`AgentMode`]:
    /// - **Ask**: read-only tools only (no write, no plans, no subagents)
    /// - **Plan**: read + plan tools (no write)
    /// - **Build**: all tools
    pub fn build_tool_registry(&self, mode: AgentMode) -> ToolRegistry {
        let mut registry = ToolRegistry::with_defaults();

        // Register memory tools only in Standard and Deep modes
        let mem_mode = self.memory_mode();
        if mem_mode != crate::config::MemoryMode::Stateless {
            if let Some(ref mem) = self.memory {
                crate::tools::register_memory_tools(&mut registry, mem.clone());
            }
        }

        crate::tools::todo::register_todo_tools(&mut registry, self.todo_list.clone());

        if let Some(ref sm) = self.skill_manager {
            crate::tools::skill::register_skill_tools(&mut registry, sm.clone());
        }

        // ── Mode-based filtering ─────────────────────────────────────
        // Remove tools not allowed in this mode BEFORE registering
        // subagent tools, so subagents inherit the correct tool list.
        registry.remove_all(mode.tools_to_remove());

        // Register subagent tools with the FILTERED tool list.
        if let Ok(model_config) = self.current_model_config() {
            let available_tools: Vec<String> = registry
                .list_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            crate::tools::subagent::register_subagent_tools(
                &mut registry,
                model_config,
                available_tools,
                None,
                self.config.permissions.clone(),
            );
        }

        registry
    }

    /// Build a fresh PermissionPolicy for a Run (from config defaults).
    pub fn build_permission_policy(&self) -> PermissionPolicy {
        PermissionPolicy::with_builtin_defaults().with_config(&self.config.permissions)
    }

    /// The recovery engine (stateless strategy, shared).
    /// Wires up the current model's fallback_model so that SwitchModel
    /// recovery can use it when the primary model fails repeatedly.
    pub fn build_recovery(&self) -> RecoveryEngine {
        let mut engine = RecoveryEngine::default();
        if let Ok(mc) = self.current_model_config() {
            if let Some(ref fb) = mc.fallback_model {
                engine = engine.with_fallback_model(fb);
            }
        }
        engine
    }

    /// The identity text for the system prompt.
    pub fn identity_text(&self) -> String {
        if let Ok(model_config) = self.current_model_config() {
            if let Some(ref custom) = model_config.system_prompt {
                return custom.clone();
            }
        }
        prompt::DEFAULT_IDENTITY.to_string()
    }

    /// The principles text for the system prompt.
    pub fn principles_text(&self, permission_mode: &str) -> String {
        format!(
            "{}\n\nPermission Mode: {} — tools may require user approval before execution.\n\n{}",
            prompt::DEFAULT_PRINCIPLES,
            permission_mode,
            self.mode().system_prompt_override(),
        )
    }

    /// Build a default HookRegistry. If the shared hook_registry is set,
    /// returns a clone of the Arc (so mutations propagate to all Runs).
    pub fn build_hooks(&self) -> Arc<Mutex<HookRegistry>> {
        self.hook_registry.clone()
    }

    /// The permission config (for subagent construction).
    pub fn permission_config(&self) -> &PermissionConfig {
        &self.config.permissions
    }

    /// The current memory mode (Stateless / Standard / Deep).
    pub fn memory_mode(&self) -> crate::config::MemoryMode {
        self.config
            .memory
            .as_ref()
            .map(|m| crate::config::MemoryMode::from_str(&m.mode))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let toml = r#"
default_model = "test/default"

[providers.test]
name = "test"
base_url = "http://127.0.0.1:1"
api_key = "sk-test"

[providers.test.models]
default = { model_id = "mock" }
"#;
        let mut config: Config = toml::from_str(toml).unwrap();
        config.rebuild_models();
        config
    }

    #[test]
    fn brain_builds_from_config() {
        let brain = Brain::from_config(test_config()).unwrap();
        assert_eq!(brain.current_model_name(), "test/default");
    }

    #[test]
    fn brain_builds_client() {
        let brain = Brain::from_config(test_config()).unwrap();
        let _client = brain.build_client().unwrap();
    }

    #[test]
    fn brain_builds_tool_registry() {
        let brain = Brain::from_config(test_config()).unwrap();
        let registry = brain.build_tool_registry(AgentMode::Build);
        assert!(registry.has("read_file"));
        assert!(registry.has("bash"));
    }

    #[test]
    fn brain_switch_model() {
        let mut brain = Brain::from_config(test_config()).unwrap();
        // Switching to the same model should work.
        brain.switch_model("test/default").unwrap();
        // Switching to a nonexistent model should fail.
        assert!(brain.switch_model("nonexistent").is_err());
    }
}
