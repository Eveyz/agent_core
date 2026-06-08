use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
}

fn default_request_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_max_iterations() -> usize {
    10
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
            max_iterations: 10,
            request_timeout_secs: 60,
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
}

fn default_db_path() -> String {
    "~/.agent_core/memory.db".to_string()
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
            db_path: "~/.agent_core/memory.db".to_string(),
            embedding_model: "BAAI/bge-small-en-v1.5".to_string(),
            max_core_blocks: 5,
            default_block_max_chars: 2000,
            consolidation_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_model: String,
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    #[serde(default)]
    pub permissions: crate::permission::PermissionConfig,
    #[serde(default)]
    pub mcp: crate::mcp::McpConfig,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {path}"))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse config from {path}"))?;

        if !config.models.contains_key(&config.default_model) {
            let available: Vec<_> = config.models.keys().map(|s| s.as_str()).collect();
            anyhow::bail!(
                "default_model '{}' not found in config.\n\
                 Available models: [{}]\n\
                 Tip: Ensure the default_model value matches a [models.XXX] section name.",
                config.default_model,
                available.join(", ")
            );
        }

        for model in config.models.values_mut() {
            model.api_key = resolve_env_value(&model.api_key);
        }

        Ok(config)
    }

    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY environment variable not set. Set it or provide a valid config.toml file.")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let model_id = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

        let model = ModelConfig {
            name: "default".to_string(),
            base_url,
            api_key,
            model_id,
            ..Default::default()
        };

        let mut models = HashMap::new();
        models.insert("default".to_string(), model);

        Ok(Self {
            default_model: "default".to_string(),
            models,
            memory: None,
            permissions: crate::permission::PermissionConfig::default(),
            mcp: crate::mcp::McpConfig::default(),
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

    /// Add or replace a model configuration at runtime.
    pub fn add_model(&mut self, name: String, model: ModelConfig) {
        self.models.insert(name, model);
    }
}

fn resolve_env_value(raw: &str) -> String {
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
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agent_core").join("config.toml")
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
default_model = "gpt"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o-mini"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        assert_eq!(config.default_model, "gpt");
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.base_url, "https://api.openai.com/v1");
        assert_eq!(model.api_key, "sk-test");
        assert_eq!(model.model_id, "gpt-4o-mini");
    }

    #[test]
    fn test_defaults_on_omitted_fields() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o-mini"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.max_context_tokens, 128000);
        assert_eq!(model.temperature, None);
        assert_eq!(model.max_tokens, None);
        assert!(model.react_enabled);
        assert_eq!(model.max_iterations, 10);
        assert_eq!(model.system_prompt, None);
    }

    #[test]
    fn test_parse_all_fields() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o"
max_context_tokens = 64000
temperature = 0.3
max_tokens = 4096
react_enabled = false
max_iterations = 5
system_prompt = "You are helpful."

[memory]
db_path = "/tmp/mem.db"
embedding_model = "my-model"
max_core_blocks = 3
default_block_max_chars = 1000
consolidation_enabled = false
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.max_context_tokens, 64000);
        assert_eq!(model.temperature, Some(0.3));
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
default_model = "nonexistent"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o"
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("default_model 'nonexistent' not found"));
        assert!(msg.contains("Available models: [gpt]"));
        assert!(msg.contains("Ensure the default_model value matches"));
    }

    #[test]
    fn test_missing_required_field() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
api_key = "sk-test"
model_id = "gpt-4o"
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
default_model = "gpt"
[[models.gpt
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("failed to parse config"));
    }

    #[test]
    fn test_multiple_models() {
        let path = write_temp_config(
            r#"
default_model = "claude"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-gpt"
model_id = "gpt-4o"

[models.claude]
base_url = "https://api.anthropic.com/v1"
api_key = "sk-claude"
model_id = "claude-3"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        assert_eq!(config.default_model, "claude");
        assert_eq!(config.models.len(), 2);
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
default_model = "gpt"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "${MY_KEY}"
model_id = "gpt-4o"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.api_key, "from-env");
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
        assert_eq!(default.max_iterations, 10);
    }

    #[test]
    fn test_commented_out_fields_parse() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o"
# max_context_tokens = 131072
# temperature = 0.7
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.max_context_tokens, 128000);
        assert_eq!(model.temperature, None);
    }

    #[test]
    fn test_model_name_auto_filled_from_section() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models.gpt]
name = "GPT-4"
base_url = "https://api.openai.com/v1"
api_key = "sk-test"
model_id = "gpt-4o"
"#,
        );
        let config = Config::load(path.to_str().unwrap()).unwrap();
        let model = config.get_model("gpt").unwrap();
        assert_eq!(model.name, "GPT-4");
    }

    #[test]
    fn test_empty_models_map() {
        let path = write_temp_config(
            r#"
default_model = "gpt"

[models]
"#,
        );
        let err = Config::load(path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Available models: []"));
    }
}
