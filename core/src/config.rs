use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
}

fn default_request_timeout() -> u64 {
    1800
}

fn default_true() -> bool {
    true
}

fn default_max_iterations() -> usize {
    100
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
            max_iterations: 100,
            request_timeout_secs: 1800,
            fallback_model: None,
            reasoning_effort: None,
            thinking_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeOverrides {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
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
    #[serde(default)]
    pub embedding_enabled: bool,
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
            embedding_enabled: false,
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
                max_iterations: 100,
                request_timeout_secs: 1800,
                models: provider_models,
            },
        );

        let mut models = HashMap::new();
        models.insert(
            "default/default".to_string(),
            ModelConfig {
                name: "default".to_string(),
                base_url,
                api_key: api_key.clone(),
                model_id,
                max_context_tokens: 128000,
                temperature: None,
                max_tokens: None,
                react_enabled: true,
                system_prompt: None,
                max_iterations: 100,
                request_timeout_secs: 1800,
                fallback_model: None,
                reasoning_effort: None,
                thinking_enabled: false,
            },
        );

        Ok(Self {
            default_model: "default/default".to_string(),
            providers,
            legacy_models: HashMap::new(),
            models,
            memory: None,
            permissions: crate::permission::PermissionConfig::default(),
            mcp: crate::mcp::McpConfig::default(),
            reflector_enabled: false,
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
                self.models.insert(
                    full_key,
                    ModelConfig {
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
                    },
                );
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
                },
            );
        }
    }

    /// Add or replace a model configuration at runtime.
    pub fn add_model(&mut self, full_key: String, model: ModelConfig) {
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
            eprintln!("warning: env var {var_name} not found, using raw value");
            raw.to_string()
        })
    } else {
        raw.to_string()
    }
}

pub fn default_config_path() -> PathBuf {
    crate::paths::get_agverse_dir().join("config.toml")
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
        assert_eq!(model.max_iterations, 100);
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
        assert!(!mem.consolidation_enabled);
    }

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
        assert_eq!(default.max_iterations, 100);
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
