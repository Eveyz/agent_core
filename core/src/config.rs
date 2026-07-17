use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApiMode {
    /// OpenAI-compatible Chat Completions (`/chat/completions`). Default / DeepSeek.
    #[default]
    ChatCompletions,
    /// OpenAI Responses API (`/responses`) with encrypted reasoning round-trip.
    Responses,
    /// Anthropic Messages API with thinking + signature round-trip.
    AnthropicMessages,
}

impl ApiMode {
    /// Infer API mode from base URL / model id when not explicitly configured.
    pub fn infer(base_url: &str, model_id: &str) -> Self {
        let url = base_url.to_lowercase();
        let model = model_id.to_lowercase();
        if url.contains("anthropic.com") || url.contains("api.anthropic") || model.contains("claude")
        {
            return Self::AnthropicMessages;
        }
        // Explicit opt-in via model id prefix or responses-style endpoints.
        if url.contains("/responses") || model.starts_with("o1") || model.starts_with("o3")
            || model.starts_with("o4") || model.contains("codex")
        {
            return Self::Responses;
        }
        Self::ChatCompletions
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelEntry {
    pub model_id: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_context_tokens: Option<usize>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Override API wire format. When unset, inferred from base_url / model_id.
    #[serde(default)]
    pub api_mode: Option<ApiMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub react_enabled: bool,
    pub system_prompt: Option<String>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    pub models: HashMap<String, ProviderModelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub react_enabled: bool,
    pub system_prompt: Option<String>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Per-model HTTP request timeout in seconds (default: 60).
    /// Increase for slow models on weak hardware.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Optional fallback model ID to use if this model fails
    #[serde(default)]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Wire-format mode. `None` → inferred from base_url / model_id at use time.
    #[serde(default)]
    pub api_mode: Option<ApiMode>,
}

fn default_request_timeout() -> u64 {
    1800
}

fn default_true() -> bool {
    true
}

fn default_max_iterations() -> usize {
    50
}

fn default_max_context_tokens() -> usize {
    128000
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model_id: "gpt-4o-mini".to_string(),
            max_context_tokens: 128000,
            temperature: None,
            max_tokens: None,
            react_enabled: true,
            system_prompt: None,
            max_iterations: 50,
            request_timeout_secs: 1800,
            fallback_model: None,
            reasoning_effort: None,
            thinking_enabled: false,
            api_mode: None,
        }
    }
}

impl ModelConfig {
    /// Resolved API mode (explicit config or inferred).
    pub fn resolved_api_mode(&self) -> ApiMode {
        self.api_mode
            .unwrap_or_else(|| ApiMode::infer(&self.base_url, &self.model_id))
    }

    pub fn auto_detect_max_context_tokens(&mut self) {
        let model_lower = self.model_id.to_lowercase();
        if self.max_context_tokens == 128000 {
            if model_lower.contains("deepseek") {
                self.max_context_tokens = 64000;
            } else if model_lower.contains("claude") {
                self.max_context_tokens = 200000;
            } else if model_lower.contains("gemini") {
                if model_lower.contains("flash") {
                    self.max_context_tokens = 1000000;
                } else if model_lower.contains("pro") {
                    self.max_context_tokens = 2000000;
                } else {
                    self.max_context_tokens = 1000000;
                }
            } else if model_lower.contains("o1") || model_lower.contains("o3") {
                self.max_context_tokens = 200000;
            } else if model_lower.contains("llama-3") || model_lower.contains("llama3") {
                if model_lower.contains("llama-3-") || model_lower.contains("llama-3.1") || model_lower.contains("llama3.1") {
                    self.max_context_tokens = 128000;
                } else {
                    self.max_context_tokens = 8192;
                }
            } else if model_lower.contains("llama-2") || model_lower.contains("llama2") {
                self.max_context_tokens = 4096;
            } else if model_lower.contains("gpt-4") {
                if model_lower.contains("gpt-4-32k") {
                    self.max_context_tokens = 32768;
                } else if model_lower.contains("gpt-4o") || model_lower.contains("gpt-4-turbo") || model_lower.contains("gpt-4-1106") || model_lower.contains("gpt-4-0125") {
                    self.max_context_tokens = 128000;
                } else {
                    self.max_context_tokens = 8192;
                }
            } else if model_lower.contains("gpt-3.5") {
                if model_lower.contains("16k") {
                    self.max_context_tokens = 16384;
                } else {
                    self.max_context_tokens = 4096;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeOverrides {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryMode {
    /// No memory — no SQLite, no recall, no agverse.md injection.
    Stateless,
    /// Standard dual-track: core memory + recall + agverse.md.
    Standard,
    /// Deep: standard + background reflection + advanced recall.
    Deep,
}

impl Default for MemoryMode {
    fn default() -> Self {
        Self::Standard
    }
}

impl MemoryMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "stateless" | "off" | "disabled" => Self::Stateless,
            "deep" | "advanced" => Self::Deep,
            _ => Self::Standard,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stateless => "stateless",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_max_core_blocks")]
    pub max_core_blocks: usize,
    #[serde(default = "default_block_max_chars")]
    pub default_block_max_chars: usize,
    #[serde(default = "default_true")]
    pub consolidation_enabled: bool,
    /// Enable embedding-based vector search (requires downloading a model).
    /// When false, conversation search falls back to keyword matching.
    #[serde(default = "default_true")]
    pub embedding_enabled: bool,
    /// Memory mode: "stateless", "standard", or "deep".
    #[serde(default = "default_memory_mode")]
    pub mode: String,
    /// Salience scoring configuration (weights, half-life, etc.).
    #[serde(default)]
    pub salience: Option<crate::memory::SalienceConfig>,
    /// Reflection daemon config (Deep mode only).
    #[serde(default)]
    pub reflection: Option<ReflectionConfig>,
    /// BM25 full-text search config (Standard & Deep).
    #[serde(default)]
    pub bm25: Option<Bm25Config>,
    /// HNSW approximate nearest neighbor config (Standard & Deep).
    #[serde(default)]
    pub hnsw: Option<HnswConfig>,
}

/// Configuration for the background reflection daemon (Deep mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Number of conversation messages to accumulate before triggering reflection.
    #[serde(default = "default_trigger_count")]
    pub trigger_message_count: usize,
    /// Model key (e.g. "deepseek/deepseek-chat") from config.models to use for reflection.
    /// If not set, reflection is disabled even in Deep mode.
    #[serde(default)]
    pub reflection_model: Option<String>,
}

fn default_trigger_count() -> usize {
    20
}

/// Configuration for BM25 full-text search (powered by tantivy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Config {
    /// Enable BM25-based keyword recall. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// BM25 k1 parameter — controls term frequency saturation.
    #[serde(default = "default_bm25_k1")]
    pub k1: f32,
    /// BM25 b parameter — controls document length normalization.
    #[serde(default = "default_bm25_b")]
    pub b: f32,
}

fn default_bm25_k1() -> f32 { 1.2 }
fn default_bm25_b() -> f32 { 0.75 }

/// Configuration for HNSW approximate nearest neighbor search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswConfig {
    /// Enable HNSW-based vector recall. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Number of candidates to retrieve during HNSW search (ef_search).
    #[serde(default = "default_hnsw_ef")]
    pub ef_search: usize,
}

fn default_hnsw_ef() -> usize { 150 }

fn default_memory_mode() -> String {
    "standard".to_string()
}

fn default_db_path() -> String {
    crate::paths::get_memory_db_path()
        .to_string_lossy()
        .to_string()
}

fn default_embedding_model() -> String {
    "BAAI/bge-small-en-v1.5".to_string()
}

fn default_max_core_blocks() -> usize {
    5
}

fn default_block_max_chars() -> usize {
    2000
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            db_path: crate::paths::get_memory_db_path()
                .to_string_lossy()
                .into_owned(),
            embedding_model: "BAAI/bge-small-en-v1.5".to_string(),
            max_core_blocks: 5,
            default_block_max_chars: 2000,
            consolidation_enabled: true,
            embedding_enabled: true,
            mode: "standard".to_string(),
            salience: None,
            reflection: None,
            bm25: None,
            hnsw: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_model: String,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    /// Backward compatibility: deserialize old flat `[models.xxx]` sections.
    #[serde(default, rename = "models", skip_serializing)]
    pub legacy_models: HashMap<String, ModelConfig>,
    /// Runtime flattened cache of all provider models as `provider_key/model_key`.
    #[serde(default, skip)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    #[serde(default)]
    pub permissions: crate::permission::PermissionConfig,
    #[serde(default)]
    pub mcp: crate::mcp::McpConfig,
    /// Enable offline reflection (analyzes Run event logs after completion).
    #[serde(default)]
    pub reflector_enabled: bool,
    /// Lightweight model name for `/btw` side-channel queries (provider/model).
    /// Falls back to `default_model` when unset.
    #[serde(default)]
    pub btw_model: Option<String>,
    /// Lightweight model name for `/learn` memory extraction (provider/model).
    /// Falls back to `default_model` when unset.
    #[serde(default)]
    pub learn_model: Option<String>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {path}"))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse config from {path}"))?;

        if config.providers.is_empty() && !config.legacy_models.is_empty() {
            config.migrate_legacy_models();
        }
        config.rebuild_models();

        if !config.models.contains_key(&config.default_model) {
            let available: Vec<_> = config.models.keys().map(|s| s.as_str()).collect();
            anyhow::bail!(
                "default_model '{}' not found in config.\n\
                 Available models: [{}]\n\
                 Tip: Ensure the default_model value matches a provider model in the format provider/model.",
                config.default_model,
                available.join(", ")
            );
        }

        // NOTE: api_key env-var resolution (`${VAR}`) is intentionally NOT done
        // here anymore. Resolving it into the in-memory Config would leak the
        // plaintext secret back to disk via save() / to the UI via get_config.
        // Instead, resolution happens at the HTTP-client construction site
        // (OpenAIClient::with_fallback), so the on-disk config keeps the
        // `${VAR}` reference and never materializes the secret in Config state.

        Ok(config)
    }

    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY environment variable not set. Set it or provide a valid config.toml file.")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let model_id = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

        let mut providers = HashMap::new();
        let mut provider_models = HashMap::new();
        provider_models.insert(
            "default".to_string(),
            ProviderModelEntry {
                model_id: model_id.clone(),
                temperature: None,
                max_tokens: None,
                system_prompt: None,
                max_context_tokens: None,
                reasoning_effort: None,
                thinking_enabled: false,
                api_mode: None,
            },
        );

        providers.insert(
            "default".to_string(),
            ProviderConfig {
                name: "default".to_string(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                max_context_tokens: 128000,
                temperature: None,
                max_tokens: None,
                react_enabled: true,
                system_prompt: None,
                max_iterations: 50,
                request_timeout_secs: 1800,
                models: provider_models,
            },
        );

        let mut models = HashMap::new();
        let mut model_config = ModelConfig {
            name: "default".to_string(),
            base_url,
            api_key: api_key.clone(),
            model_id,
            max_context_tokens: 128000,
            temperature: None,
            max_tokens: None,
            react_enabled: true,
            system_prompt: None,
            max_iterations: 50,
            request_timeout_secs: 1800,
            fallback_model: None,
            reasoning_effort: None,
            thinking_enabled: false,
            api_mode: None,
        };
        model_config.auto_detect_max_context_tokens();
        models.insert("default/default".to_string(), model_config);

        Ok(Self {
            default_model: "default/default".to_string(),
            providers,
            legacy_models: HashMap::new(),
            models,
            memory: None,
            permissions: crate::permission::PermissionConfig::default(),
            mcp: crate::mcp::McpConfig::default(),
            reflector_enabled: false,
            btw_model: None,
            learn_model: None,
        })
    }

    pub fn get_model(&self, name: &str) -> Option<&ModelConfig> {
        self.models.get(name)
    }

    pub fn default_model(&self) -> Result<&ModelConfig> {
        self.models
            .get(&self.default_model)
            .with_context(|| format!("default model '{}' not found in config", self.default_model))
    }

    pub fn rebuild_models(&mut self) {
        self.models.clear();
        for (provider_key, provider) in &self.providers {
            for (model_key, entry) in &provider.models {
                let full_key = format!("{}/{}", provider_key, model_key);
                let mut model_config = ModelConfig {
                    name: provider.name.clone(),
                    base_url: provider.base_url.clone(),
                    api_key: provider.api_key.clone(),
                    model_id: entry.model_id.clone(),
                    max_context_tokens: entry.max_context_tokens.unwrap_or(provider.max_context_tokens),
                    temperature: entry.temperature.or(provider.temperature),
                    max_tokens: entry.max_tokens.or(provider.max_tokens),
                    react_enabled: provider.react_enabled,
                    system_prompt: entry
                        .system_prompt
                        .clone()
                        .or_else(|| provider.system_prompt.clone()),
                    max_iterations: provider.max_iterations,
                    request_timeout_secs: provider.request_timeout_secs,
                    fallback_model: None,
                    reasoning_effort: entry.reasoning_effort.clone(),
                    thinking_enabled: entry.thinking_enabled,
                    api_mode: entry.api_mode,
                };
                model_config.auto_detect_max_context_tokens();
                self.models.insert(full_key, model_config);
            }
        }
    }

    /// Migrate old flat `[models.xxx]` sections into the new provider structure.
    fn migrate_legacy_models(&mut self) {
        for (key, model) in self.legacy_models.drain() {
            let provider_key = if model.name.is_empty() {
                key.clone()
            } else {
                model
                    .name
                    .to_lowercase()
                    .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
            };

            if self.default_model == key {
                self.default_model = format!("{}/{}", provider_key, key);
            }

            let provider = self
                .providers
                .entry(provider_key.clone())
                .or_insert_with(|| ProviderConfig {
                    name: model.name.clone(),
                    base_url: model.base_url.clone(),
                    api_key: model.api_key.clone(),
                    max_context_tokens: model.max_context_tokens,
                    temperature: model.temperature,
                    max_tokens: model.max_tokens,
                    react_enabled: model.react_enabled,
                    system_prompt: model.system_prompt.clone(),
                    max_iterations: model.max_iterations,
                    request_timeout_secs: model.request_timeout_secs,
                    models: HashMap::new(),
                });

            provider.models.insert(
                key,
                ProviderModelEntry {
                    model_id: model.model_id.clone(),
                    temperature: model.temperature,
                    max_tokens: model.max_tokens,
                    system_prompt: model.system_prompt.clone(),
                    max_context_tokens: Some(model.max_context_tokens),
                    reasoning_effort: model.reasoning_effort.clone(),
                    thinking_enabled: model.thinking_enabled,
                    api_mode: model.api_mode,
                },
            );
        }
    }

    /// Add or replace a model configuration at runtime.
    pub fn add_model(&mut self, full_key: String, mut model: ModelConfig) {
        let sep_pos = full_key.rfind('/').unwrap_or(0);
        let provider_key = if sep_pos > 0 {
            &full_key[..sep_pos]
        } else {
            &full_key
        };
        let model_key = if sep_pos > 0 {
            &full_key[sep_pos + 1..]
        } else {
            &full_key
        };

        model.auto_detect_max_context_tokens();

        let provider = self
            .providers
            .entry(provider_key.to_string())
            .or_insert_with(|| ProviderConfig {
                name: model.name.clone(),
                base_url: model.base_url.clone(),
                api_key: model.api_key.clone(),
                max_context_tokens: model.max_context_tokens,
                temperature: model.temperature,
                max_tokens: model.max_tokens,
                react_enabled: model.react_enabled,
                system_prompt: model.system_prompt.clone(),
                max_iterations: model.max_iterations,
                request_timeout_secs: model.request_timeout_secs,
                models: HashMap::new(),
            });

        provider.models.insert(
            model_key.to_string(),
            ProviderModelEntry {
                model_id: model.model_id.clone(),
                temperature: model.temperature,
                max_tokens: model.max_tokens,
                system_prompt: model.system_prompt.clone(),
                max_context_tokens: Some(model.max_context_tokens),
                reasoning_effort: model.reasoning_effort.clone(),
                thinking_enabled: model.thinking_enabled,
                api_mode: model.api_mode,
            },
        );

        self.models.insert(full_key, model);
    }

    /// Save config to a TOML file.
    pub fn save(&self, path: &str) -> Result<()> {
        let toml_str =
            toml::to_string_pretty(self).context("failed to serialize config to TOML")?;
        std::fs::write(path, toml_str)
            .with_context(|| format!("failed to write config to {path}"))?;
        Ok(())
    }
}

/// Resolve a config value that may reference an env var via `${VAR}` syntax.
/// If the value is `"${VAR}"`, returns the env var; otherwise returns the raw
/// value unchanged. Used for secrets (`api_key`) at the point of use (HTTP
/// client construction) so the on-disk Config keeps the `${VAR}` reference and
/// the plaintext secret never materializes in Config state.
pub fn resolve_env_value(raw: &str) -> String {
    if raw.starts_with("${") && raw.ends_with('}') {
        let var_name = &raw[2..raw.len() - 1];
        std::env::var(var_name).unwrap_or_else(|_| {
            tracing::warn!(var_name, "env var not found, using raw value");
            raw.to_string()
        })
    } else {
        raw.to_string()
    }
}

pub fn default_config_path() -> PathBuf {
    crate::paths::get_agverse_dir().join("config.toml")
}

/// Scaffold used when neither `~/.agverse/config.toml` nor env vars are available.
/// Matches the Tauri app's first-run default so CLI and desktop stay aligned.
pub fn default_scaffold_config() -> Config {
    let mut models = HashMap::new();
    models.insert(
        "default".to_string(),
        ProviderModelEntry {
            model_id: "gpt-4o-mini".to_string(),
            temperature: None,
            max_tokens: None,
            system_prompt: None,
            max_context_tokens: None,
            reasoning_effort: None,
            thinking_enabled: false,
            api_mode: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "default".to_string(),
        ProviderConfig {
            name: "default".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            max_context_tokens: 128000,
            temperature: None,
            max_tokens: None,
            react_enabled: true,
            system_prompt: None,
            max_iterations: 100,
            request_timeout_secs: 60,
            models,
        },
    );
    let mut config = Config {
        default_model: "default/default".to_string(),
        providers,
        legacy_models: HashMap::new(),
        models: HashMap::new(),
        memory: None,
        permissions: Default::default(),
        mcp: Default::default(),
        reflector_enabled: false,
        btw_model: None,
        learn_model: None,
    };
    config.rebuild_models();
    config
}

/// Load config the same way as the Tauri app:
/// 1. Ensure `~/.agverse` exists
/// 2. `Config::load(path)` when present/valid
/// 3. else `Config::from_env()` and save
/// 4. else scaffold default and save
///
/// `path` defaults to [`default_config_path`] (`~/.agverse/config.toml`).
pub fn load_or_init_default(path: Option<&std::path::Path>) -> Result<(Config, PathBuf)> {
    let config_path = path
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);
    let agverse_dir = crate::paths::get_agverse_dir();
    if let Err(e) = std::fs::create_dir_all(&agverse_dir) {
        tracing::warn!(
            path = %agverse_dir.display(),
            error = %e,
            "could not create ~/.agverse"
        );
    }
    // When overriding path, also ensure its parent exists.
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let path_str = config_path.to_string_lossy().to_string();
    let config = if let Ok(cfg) = Config::load(&path_str) {
        cfg
    } else if let Ok(cfg) = Config::from_env() {
        let _ = cfg.save(&path_str);
        cfg
    } else {
        let default_config = default_scaffold_config();
        let _ = default_config.save(&path_str);
        default_config
    };
    Ok((config, config_path))
}

#[cfg(test)]
mod load_or_init_tests {
    use super::*;

    #[test]
    fn load_or_init_default_creates_scaffold_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(!path.exists());
        let (cfg, loaded) = load_or_init_default(Some(&path)).unwrap();
        assert_eq!(loaded, path);
        assert!(path.exists());
        assert_eq!(cfg.default_model, "default/default");
        assert!(cfg.models.contains_key("default/default"));
    }

    #[test]
    fn load_or_init_default_reads_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let content = r#"
default_model = "p/m"

[providers.p]
name = "p"
base_url = "http://localhost"
api_key = "k"

[providers.p.models.m]
model_id = "m"
"#;
        std::fs::write(&path, content).unwrap();
        let (cfg, _) = load_or_init_default(Some(&path)).unwrap();
        assert_eq!(cfg.default_model, "p/m");
        assert!(cfg.models.contains_key("p/m"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_config(content: &str) -> tempfile::TempPath {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.into_temp_path()
    }

    #[test]
    fn test_parse_minimal_config() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"

[providers.openai.models]
gpt = { model_id = "gpt-4o-mini" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        assert_eq!(config.default_model, "openai/gpt");
        let model = config.get_model("openai/gpt").unwrap();
        assert_eq!(model.base_url, "https://api.openai.com/v1");
        assert_eq!(model.api_key, "sk-test");
        assert_eq!(model.model_id, "gpt-4o-mini");
    }

    #[test]
    fn test_auto_detect_max_context_tokens() {
        let path = write_temp_config(
            r#"
default_model = "deepseek/chat"

[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "sk-test"

[providers.deepseek.models]
chat = { model_id = "deepseek-chat" }
claude = { model_id = "claude-3-5-sonnet" }
gemini = { model_id = "gemini-1.5-flash" }
llama = { model_id = "llama3-8b" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        
        let ds = config.get_model("deepseek/chat").unwrap();
        assert_eq!(ds.max_context_tokens, 64000);

        let claude = config.get_model("deepseek/claude").unwrap();
        assert_eq!(claude.max_context_tokens, 200000);

        let gemini = config.get_model("deepseek/gemini").unwrap();
        assert_eq!(gemini.max_context_tokens, 1000000);

        let llama = config.get_model("deepseek/llama").unwrap();
        assert_eq!(llama.max_context_tokens, 8192);
    }

    #[test]
    fn test_defaults_on_omitted_fields() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"

[providers.openai.models]
gpt = { model_id = "gpt-4o-mini" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("openai/gpt").unwrap();
        assert_eq!(model.max_context_tokens, 128000);
        assert_eq!(model.temperature, None);
        assert_eq!(model.max_tokens, None);
        assert!(model.react_enabled);
        assert_eq!(model.max_iterations, 50);
        assert_eq!(model.system_prompt, None);
    }

    #[test]
    fn test_parse_all_fields() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
max_context_tokens = 64000
temperature = 0.3
max_tokens = 4096
react_enabled = false
max_iterations = 5
system_prompt = "You are helpful."

[providers.openai.models]
gpt = { model_id = "gpt-4o", temperature = 0.5 }

[memory]
db_path = "/tmp/mem.db"
embedding_model = "my-model"
max_core_blocks = 3
default_block_max_chars = 1000
consolidation_enabled = false
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("openai/gpt").unwrap();
        assert_eq!(model.max_context_tokens, 64000);
        assert_eq!(model.temperature, Some(0.5)); // model override takes precedence
        assert_eq!(model.max_tokens, Some(4096));
        assert!(!model.react_enabled);
        assert_eq!(model.max_iterations, 5);
        assert_eq!(model.system_prompt.as_deref(), Some("You are helpful."));

        let mem = config.memory.unwrap();
        assert_eq!(mem.db_path, "/tmp/mem.db");
        assert_eq!(mem.embedding_model, "my-model");
        assert_eq!(mem.max_core_blocks, 3);
        assert_eq!(mem.default_block_max_chars, 1000);
        assert!(!mem.consolidation_enabled);    }

    #[test]
    fn test_default_model_not_found() {
        let path = write_temp_config(
            r#"
default_model = "nonexistent/missing"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("default_model 'nonexistent/missing' not found"));
        assert!(msg.contains("Available models: [openai/gpt]"));
        assert!(msg.contains("Ensure the default_model value matches"));
    }

    #[test]
    fn test_missing_required_field() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
api_key = "sk-test"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("failed to parse config"));
        assert!(msg.contains("base_url"));
    }

    #[test]
    fn test_missing_file() {
        let err = Config::load("/nonexistent/path/config.toml").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("failed to read config"));
    }

    #[test]
    fn test_invalid_toml_syntax() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"
[[providers.openai.models.gpt
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("failed to parse config"));
    }

    #[test]
    fn test_multiple_providers() {
        let path = write_temp_config(
            r#"
default_model = "anthropic/claude"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-gpt"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }

[providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com/v1"
api_key = "sk-claude"

[providers.anthropic.models]
claude = { model_id = "claude-3" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        assert_eq!(config.default_model, "anthropic/claude");
        assert_eq!(config.providers.len(), 2);
        let default = config.default_model().unwrap();
        assert_eq!(default.model_id, "claude-3");
    }

    #[test]
    fn test_resolve_env_value() {
        unsafe { std::env::set_var("TEST_API_KEY", "secret123") };
        let result = resolve_env_value("${TEST_API_KEY}");
        assert_eq!(result, "secret123");
        unsafe { std::env::remove_var("TEST_API_KEY") };
    }

    #[test]
    fn test_resolve_env_value_fallback() {
        let result = resolve_env_value("${NONEXISTENT_VAR_XYZ}");
        assert_eq!(result, "${NONEXISTENT_VAR_XYZ}");
    }

    #[test]
    fn test_resolve_plain_value() {
        let result = resolve_env_value("sk-plaintext");
        assert_eq!(result, "sk-plaintext");
    }

    #[test]
    fn test_env_var_in_api_key() {
        unsafe { std::env::set_var("MY_KEY", "from-env") };
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "${MY_KEY}"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("openai/gpt").unwrap();
        // Config::load must NOT resolve `${VAR}` into the in-memory api_key —
        // doing so would leak the plaintext secret back via save()/get_config.
        // The on-disk reference is preserved verbatim.
        assert_eq!(model.api_key, "${MY_KEY}");
        // Resolution happens at the point of use (HTTP client construction).
        assert_eq!(resolve_env_value(&model.api_key), "from-env");
        unsafe { std::env::remove_var("MY_KEY") };
    }

    #[test]
    fn test_model_default_values() {
        let default = ModelConfig::default();
        assert_eq!(default.base_url, "https://api.openai.com/v1");
        assert_eq!(default.model_id, "gpt-4o-mini");
        assert_eq!(default.max_context_tokens, 128000);
        assert_eq!(default.temperature, None);
        assert_eq!(default.max_tokens, None);
        assert!(default.react_enabled);
        assert_eq!(default.max_iterations, 50);
    }

    #[test]
    fn test_commented_out_fields_parse() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }
# max_context_tokens = 131072
# temperature = 0.7
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("openai/gpt").unwrap();
        assert_eq!(model.max_context_tokens, 128000);
        assert_eq!(model.temperature, None);
    }

    #[test]
    fn test_empty_providers_map() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Available models: []"));
    }

    #[test]
    fn test_duplicate_model_keys_across_providers() {
        let path = write_temp_config(
            r#"
default_model = "openai/gpt"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-openai"

[providers.openai.models]
gpt = { model_id = "gpt-4o" }

[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "sk-deepseek"

[providers.deepseek.models]
gpt = { model_id = "deepseek-chat" }
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let openai_gpt = config.get_model("openai/gpt").unwrap();
        let deepseek_gpt = config.get_model("deepseek/gpt").unwrap();
        assert_eq!(openai_gpt.model_id, "gpt-4o");
        assert_eq!(deepseek_gpt.model_id, "deepseek-chat");
        assert_eq!(openai_gpt.base_url, "https://api.openai.com/v1");
        assert_eq!(deepseek_gpt.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn test_legacy_models_migration() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o"

[models.claude]
name = "Anthropic"
base_url = "https://api.anthropic.com/v1"
api_key = "sk-claude"
model_id = "claude-3"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        assert!(config.legacy_models.is_empty());
        assert_eq!(config.providers.len(), 2);
        assert!(config.get_model("openai/gpt").is_some());
        assert!(config.get_model("anthropic/claude").is_some());
        assert_eq!(config.default_model, "openai/gpt");
    }
}
