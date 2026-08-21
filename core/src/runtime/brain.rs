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
use crate::todo::SessionPlanStore;
use crate::tools::ToolRegistry;
use crate::types::ToolExecutionMode;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The reusable "brain" shared by all Runs.
///
/// Cloneable via `Arc` — each Run holds an `Arc<Brain>`.
/// Contains shared state and runtime overrides; per-request state lives in [`crate::runtime::Run`].
#[derive(Clone)]
pub struct Brain {
    /// The full configuration (models, permissions, memory, mcp).
    pub(crate) config: Config,
    /// Shared memory manager (if memory is enabled).
    pub(crate) memory: Option<Arc<Mutex<MemoryManager>>>,
    /// Background reflection daemon (Deep mode only, if reflection_model is set).
    pub(crate) reflection_daemon: Option<Arc<ReflectionDaemon>>,
    /// Shared skill manager (if skills are enabled).
    pub(crate) skill_manager: Option<Arc<Mutex<SkillManager>>>,
    /// Workspace-scoped managers prevent same-named project skills from
    /// changing underneath Runs in another project.
    workspace_skill_managers: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<SkillManager>>>>>,
    /// Per-session multi-plan store (active / parked plans shared across Runs).
    pub(crate) todo_lists: Arc<SessionPlanStore>,
    /// Offline reflector (analyzes Run event logs after completion).
    pub(crate) reflector: Option<Reflector>,
    /// The currently active model name (e.g. "openai/gpt-4o").
    /// Switching the model updates this; new Runs use the new model.
    current_model_name: String,
    /// The current agent mode (Ask / Plan / Build).
    /// Mode changes take effect on the next Run — existing Runs are unaffected.
    /// Wrapped in Arc<Mutex<>> to satisfy Clone (required by #[derive(Clone)]).
    current_mode: Arc<Mutex<AgentMode>>,
    /// Shared hook registry. When set, Runs use this instead of building fresh.
    /// This allows the Agent wrapper to register hooks that fire during Run execution.
    pub(crate) hook_registry: Arc<Mutex<HookRegistry>>,
    /// Persistent tool registry for CLI display / permission checking.
    /// NOT cloned into Runs — use [`Brain::register_tool_fn`] for tools that
    /// must execute during Runs (e.g. MCP tools).
    pub(crate) base_tool_registry: Arc<Mutex<ToolRegistry>>,
    /// Tool registration callbacks applied to every per-Run ToolRegistry.
    /// CLI pushes callbacks here (e.g. MCP tools) so they flow into every Run.
    extra_tool_fns: Arc<Mutex<Vec<Box<dyn Fn(&mut ToolRegistry) + Send + Sync>>>>,
    /// Per-Run approval resolvers, owned by the Brain so nested task
    /// subagents can inherit the parent Run's resolver without a process-global map.
    run_approvals: Arc<Mutex<HashMap<String, crate::runtime::ApprovalResolver>>>,
    /// Runtime temperature override (applied to each Run's client).
    pub(crate) temperature_override: Option<f64>,
    /// Runtime max_tokens override (applied to each Run's client).
    pub(crate) max_tokens_override: Option<u32>,
    /// Tool execution mode (Parallel or Sequential) for new Runs.
    pub(crate) tool_execution_mode: ToolExecutionMode,
}

impl Brain {
    /// Build a Brain from a loaded Config.
    pub fn from_config(config: Config) -> Result<Self> {
        let memory = Self::build_memory(&config);
        let reflection_daemon = Self::build_reflection_daemon(&config, &memory);
        if reflection_daemon.is_none()
            && let Some(memory) = &memory
        {
            let _ = crate::memory::reflection::disable_reflection(memory.lock().storage());
        }
        let skill_manager = Self::build_skill_manager(&config)?;
        let reflector = Self::build_reflector(&config);

        let current_model_name = config.default_model.clone();
        let storage = memory.as_ref().map(|m| m.lock().storage());
        let todo_lists = Arc::new(SessionPlanStore::with_storage(storage));

        // Build a baseline tool registry for CLI display.
        let mut base_tool_registry = ToolRegistry::with_defaults();
        crate::tools::todo::register_todo_tools(
            &mut base_tool_registry,
            todo_lists.clone(),
            None,
            None,
        );
        crate::tools::ask_user::register_ask_user_tool(&mut base_tool_registry);
        if let Some(ref sm) = skill_manager {
            crate::tools::skill::register_skill_tools(&mut base_tool_registry, sm.clone());
        }

        Ok(Self {
            config,
            memory,
            reflection_daemon,
            skill_manager,
            workspace_skill_managers: Arc::new(Mutex::new(HashMap::new())),
            todo_lists,
            reflector,
            current_model_name,
            current_mode: Arc::new(Mutex::new(AgentMode::default())),
            hook_registry: Arc::new(Mutex::new(HookRegistry::new())),
            base_tool_registry: Arc::new(Mutex::new(base_tool_registry)),
            extra_tool_fns: Arc::new(Mutex::new(Vec::new())),
            run_approvals: Arc::new(Mutex::new(HashMap::new())),
            temperature_override: None,
            max_tokens_override: None,
            tool_execution_mode: ToolExecutionMode::Parallel,
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

        let default_db_path = crate::paths::get_memory_db_path()
            .to_string_lossy()
            .into_owned();
        let db_path = mem_config
            .map(|m| m.db_path.as_str())
            .unwrap_or(&default_db_path);
        let block_max_chars = mem_config
            .map(|m| m.default_block_max_chars)
            .unwrap_or(2000);
        let embedding_enabled = cfg!(feature = "embeddings")
            && mem_config.map(|m| m.embedding_enabled).unwrap_or(true);
        let salience_config = mem_config.and_then(|m| m.salience.as_ref());

        let result = if embedding_enabled {
            let embedding_model = mem_config
                .map(|m| m.embedding_model.as_str())
                .unwrap_or("BAAI/bge-small-en-v1.5");
            tracing::info!(
                "initializing memory (mode={}) with embedding model: {embedding_model} (lazy — loaded on first use)",
                mode.as_str()
            );
            MemoryManager::new(db_path, embedding_model, block_max_chars, salience_config)
        } else {
            tracing::info!(
                "initializing memory (mode={}) without embedding (keyword search mode)",
                mode.as_str()
            );
            MemoryManager::without_embedding(db_path, block_max_chars, salience_config)
        };

        match result {
            Ok(mut m) => {
                // Indexes (BM25 + HNSW) are built asynchronously after startup
                // to avoid blocking the UI. Searches fall back to SQLite
                // keyword / brute-force vector scan until the index is ready.
                // See MemoryManager::build_bm25() and build_hnsw().

                Some(Arc::new(Mutex::new(m)))
            }
            Err(e) => {
                tracing::warn!(
                    "failed to initialize memory system: {e}; continuing without memory"
                );
                None
            }
        }
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

        let claim_lease_seconds = model_config.request_timeout_secs.saturating_add(300);
        let daemon = ReflectionDaemon::spawn(client, memory, trigger_count, claim_lease_seconds);
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

    /// Return the stable SkillManager for a workspace. Managers are cached per
    /// canonical workspace so activation state persists without sharing path
    /// precedence across projects.
    pub fn skill_manager_for_workspace(
        &self,
        workspace: Option<&Path>,
    ) -> Result<Option<Arc<Mutex<SkillManager>>>> {
        let Some(base) = self.skill_manager.as_ref() else {
            return Ok(None);
        };
        let Some(workspace) = workspace else {
            return Ok(Some(base.clone()));
        };
        let key = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let mut cache = self.workspace_skill_managers.lock();
        if let Some(manager) = cache.get(&key) {
            return Ok(Some(manager.clone()));
        }
        let mut manager = SkillManager::with_global_defaults();
        manager.add_workspace_root(&key);
        manager.scan()?;
        let manager = Arc::new(Mutex::new(manager));
        cache.insert(key, manager.clone());
        Ok(Some(manager))
    }

    /// Reload the global manager and every workspace-scoped manager while
    /// preserving each manager's requested/active session state.
    pub fn reload_all_skill_managers(&self) -> Result<(usize, usize)> {
        let mut managers = Vec::new();
        if let Some(manager) = &self.skill_manager {
            managers.push(manager.clone());
        }
        managers.extend(self.workspace_skill_managers.lock().values().cloned());

        let mut discovered = 0usize;
        let mut reactivated = 0usize;
        for manager in managers {
            let (manager_discovered, manager_reactivated) =
                manager.lock().reload_preserving_active()?;
            discovered += manager_discovered;
            reactivated += manager_reactivated;
        }
        Ok((discovered, reactivated))
    }

    /// Remove session-scoped activation state from every manager cache.
    pub fn clear_skill_session(&self, session_id: &str) {
        if let Some(manager) = &self.skill_manager {
            manager.lock().clear_session(session_id);
        }
        let managers: Vec<_> = self
            .workspace_skill_managers
            .lock()
            .values()
            .cloned()
            .collect();
        for manager in managers {
            manager.lock().clear_session(session_id);
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
        self.reflection_daemon = Self::build_reflection_daemon(&config, &self.memory);
        if let Some(ref daemon) = self.reflection_daemon {
            daemon.start();
        } else if let Some(memory) = &self.memory {
            let _ = crate::memory::reflection::disable_reflection(memory.lock().storage());
        }
        self.reflector = Self::build_reflector(&config);
        self.config = config;
    }

    /// Switch the active model by name (must exist in config).
    pub fn switch_model(&mut self, name: &str) -> Result<()> {
        let resolved = self
            .config
            .resolve_model_name(name)
            .ok_or_else(|| anyhow::anyhow!("model '{name}' not found in config"))?;
        self.current_model_name = resolved;
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

    /// Build a per-Run ToolRegistry cloned from the base registry, filtered by mode.
    ///
    /// Each Run gets its own registry cloned from [`Brain::base_tool_registry`],
    /// with mode-based tool removal and subagent tool registration applied.
    ///
    /// Tools are removed based on [`AgentMode`]:
    /// - **Ask**: read-only tools only (no write, no todos)
    /// - **Plan**: read + `write_file` for plan artifacts (no shell/edit/todos)
    /// - **Build**: all tools
    pub fn build_tool_registry(&self, mode: AgentMode) -> ToolRegistry {
        self.build_tool_registry_for(mode, None, None)
    }

    /// Like [`Self::build_tool_registry`], but wires todo tools to the given session/prompt.
    pub fn build_tool_registry_for(
        &self,
        mode: AgentMode,
        session_id: Option<&str>,
        prompt_id: Option<&str>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::with_defaults();

        // Register memory tools only in Standard and Deep modes
        let mem_mode = self.memory_mode();
        if mem_mode != crate::config::MemoryMode::Stateless {
            if let Some(ref mem) = self.memory {
                crate::tools::register_memory_tools(&mut registry, mem.clone());
            }
        }

        crate::tools::todo::register_todo_tools(
            &mut registry,
            self.todo_lists.clone(),
            session_id.map(|s| s.to_string()),
            prompt_id.map(|s| s.to_string()),
        );
        crate::tools::ask_user::register_ask_user_tool(&mut registry);

        if let Some(ref sm) = self.skill_manager {
            crate::tools::skill::register_skill_tools(&mut registry, sm.clone());
        }

        // Apply extra tool registration callbacks (e.g. MCP tools).
        {
            let fns = self.extra_tool_fns.lock();
            for f in fns.iter() {
                f(&mut registry);
            }
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
                None,
                None,
                0,
                self.skill_manager.clone(),
                None,
                Some(self.todo_lists.clone()),
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

    /// The principles text for the system prompt (uses Brain's current mode).
    pub fn principles_text(&self, permission_mode: &str) -> String {
        self.principles_text_for(self.mode(), permission_mode)
    }

    /// Mode-aware principles: shared core + mode override + mode protocols.
    ///
    /// Ask omits Build planning / file-ops recipes so the model never learns
    /// tool names that were stripped from the registry.
    pub fn principles_text_for(
        &self,
        mode: crate::mode::AgentMode,
        permission_mode: &str,
    ) -> String {
        use crate::mode::AgentMode;

        let mut text = format!(
            "{}\n\nPermission Mode: {} — tools may require user approval before execution.\n\n{}",
            prompt::DEFAULT_PRINCIPLES_CORE,
            permission_mode,
            mode.system_prompt_override(),
        );

        match mode {
            AgentMode::Ask => {}
            AgentMode::Plan => {
                text.push_str("\n\n");
                text.push_str(prompt::FILE_OPS_PLAN);
                text.push_str("\n\n");
                text.push_str(prompt::PLAN_MODE_PROTOCOL);
            }
            AgentMode::Build => {
                text.push_str("\n\n");
                text.push_str(prompt::FILE_OPS_BUILD);
                text.push_str("\n\n");
                text.push_str(prompt::BUILD_PLANNING_PROTOCOL);
            }
        }

        if self.memory_mode() != crate::config::MemoryMode::Stateless {
            text.push_str("\n\n");
            text.push_str(prompt::MEMORY_PROTOCOL);
        }
        text
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

    /// Apply a temperature override for subsequent Runs.
    pub fn set_temperature(&mut self, val: f64) {
        self.temperature_override = Some(val);
    }

    /// Apply a max_tokens override for subsequent Runs.
    pub fn set_max_tokens(&mut self, val: u32) {
        self.max_tokens_override = Some(val);
    }

    /// Set the tool execution mode for subsequent Runs.
    pub fn set_tool_execution_mode(&mut self, mode: ToolExecutionMode) {
        self.tool_execution_mode = mode;
    }

    /// Register a tool registration callback. Applied to every per-Run ToolRegistry.
    /// Use this for tools that need to execute during Runs (e.g. MCP tools).
    pub fn register_tool_fn(&self, f: Box<dyn Fn(&mut ToolRegistry) + Send + Sync>) {
        self.extra_tool_fns.lock().push(f);
    }

    /// The base tool registry (for CLI display / permission checking).
    /// This is NOT used during Run execution — use [`register_tool_fn`] for that.
    pub fn display_registry(&self) -> parking_lot::MutexGuard<'_, ToolRegistry> {
        self.base_tool_registry.lock()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn permissions(&self) -> &PermissionConfig {
        &self.config.permissions
    }

    pub fn mcp_config(&self) -> &crate::mcp::McpConfig {
        &self.config.mcp
    }

    pub fn todo_store(&self) -> &SessionPlanStore {
        &self.todo_lists
    }

    pub fn memory(&self) -> Option<&Arc<Mutex<MemoryManager>>> {
        self.memory.as_ref()
    }

    pub fn skill_manager(&self) -> Option<&Arc<Mutex<SkillManager>>> {
        self.skill_manager.as_ref()
    }

    pub fn hook_registry(&self) -> &Arc<Mutex<HookRegistry>> {
        &self.hook_registry
    }

    pub fn has_reflection_daemon(&self) -> bool {
        self.reflection_daemon.is_some()
    }

    pub fn reflection_daemon(&self) -> Option<Arc<ReflectionDaemon>> {
        self.reflection_daemon.clone()
    }

    pub fn tool_execution_mode(&self) -> ToolExecutionMode {
        self.tool_execution_mode
    }

    pub fn set_permission_mode(&mut self, mode: crate::permission::PermissionMode) {
        self.config.permissions.mode = mode;
    }

    pub(crate) fn register_run_approval(
        &self,
        run_id: &str,
        resolver: crate::runtime::ApprovalResolver,
    ) {
        self.run_approvals
            .lock()
            .insert(run_id.to_string(), resolver);
    }

    pub(crate) fn approval_for_run(&self, run_id: &str) -> Option<crate::runtime::ApprovalResolver> {
        self.run_approvals.lock().get(run_id).cloned()
    }

    pub(crate) fn unregister_run_approval(&self, run_id: &str) {
        self.run_approvals.lock().remove(run_id);
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
        assert!(registry.has("shell"));
        assert!(registry.has("ask_user"));
    }

    #[test]
    fn brain_switch_model() {
        let mut brain = Brain::from_config(test_config()).unwrap();
        // Switching to the same model should work.
        brain.switch_model("test/default").unwrap();
        // Switching to a nonexistent model should fail.
        assert!(brain.switch_model("nonexistent").is_err());
    }

    #[test]
    fn workspace_skill_managers_are_isolated_reloaded_and_cleared_together() {
        let brain = Brain::from_config(test_config()).unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for (root, body) in [(first.path(), "first"), (second.path(), "second")] {
            let skill = root.join(".agents/skills/shared");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: shared\ndescription: {body}\n---\n{body}"),
            )
            .unwrap();
        }

        let first_manager = brain
            .skill_manager_for_workspace(Some(first.path()))
            .unwrap()
            .unwrap();
        let second_manager = brain
            .skill_manager_for_workspace(Some(second.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            first_manager.lock().find_by_name("shared").unwrap().description,
            "first"
        );
        assert_eq!(
            second_manager.lock().find_by_name("shared").unwrap().description,
            "second"
        );

        first_manager.lock().activate_for(Some("session-a"), "shared");
        second_manager.lock().activate_for(Some("session-a"), "shared");
        brain.clear_skill_session("session-a");
        assert!(!first_manager.lock().is_active_for(Some("session-a"), "shared"));
        assert!(!second_manager.lock().is_active_for(Some("session-a"), "shared"));

        let new_skill = first.path().join(".agents/skills/new-skill");
        std::fs::create_dir_all(&new_skill).unwrap();
        std::fs::write(
            new_skill.join("SKILL.md"),
            "---\nname: new-skill\ndescription: new\n---\nnew",
        )
        .unwrap();
        brain.reload_all_skill_managers().unwrap();
        assert!(first_manager.lock().find_by_name("new-skill").is_some());
        assert!(second_manager.lock().find_by_name("new-skill").is_none());
    }

    #[test]
    fn ask_principles_do_not_leak_todo_write() {
        let brain = Brain::from_config(test_config()).unwrap();
        let text = brain.principles_text_for(AgentMode::Ask, "Default");
        assert!(!text.contains("todo_write"));
        assert!(!text.contains("todo_update"));
        assert!(!text.contains("## Planning Protocol"));
        assert!(!text.contains("Use `write_file` ONLY when"));
        assert!(text.contains("MODE: Ask"));
    }

    #[test]
    fn plan_principles_direct_plan_md() {
        let brain = Brain::from_config(test_config()).unwrap();
        let text = brain.principles_text_for(AgentMode::Plan, "Default");
        assert!(text.contains("plan.md"));
        assert!(text.contains("## Plan Protocol"));
        assert!(!text.contains("## Planning Protocol"));
        assert!(!text.contains("FIRST call todo_write"));
    }

    #[test]
    fn ask_registry_omits_todo_and_write_tools() {
        let brain = Brain::from_config(test_config()).unwrap();
        let registry = brain.build_tool_registry(AgentMode::Ask);
        for name in ["todo_write", "todo_read", "todo_update", "write_file", "edit", "shell"] {
            assert!(
                !registry.has(name),
                "Ask registry must not expose {name}"
            );
        }
        assert!(registry.has("read_file"));
    }

    #[test]
    fn plan_registry_keeps_write_file_omits_todos() {
        let brain = Brain::from_config(test_config()).unwrap();
        let registry = brain.build_tool_registry(AgentMode::Plan);
        assert!(registry.has("write_file"));
        assert!(registry.has("read_file"));
        for name in ["todo_write", "todo_read", "edit", "shell"] {
            assert!(
                !registry.has(name),
                "Plan registry must not expose {name}"
            );
        }
    }

    #[test]
    fn run_approvals_are_scoped_to_the_brain() {
        let brain = Brain::from_config(test_config()).unwrap();
        let resolver = crate::runtime::ApprovalResolver::new();
        brain.register_run_approval("run-a", resolver.clone());
        assert!(brain.approval_for_run("run-a").is_some());
        assert!(brain.approval_for_run("run-b").is_none());
        brain.unregister_run_approval("run-a");
        assert!(brain.approval_for_run("run-a").is_none());
    }
}
