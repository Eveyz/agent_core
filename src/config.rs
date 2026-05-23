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
    pub max_context_tokens: usize,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    pub max_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub react_enabled: bool,
    pub system_prompt: Option<String>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_temperature() -> f64 {
    0.7
}

fn default_true() -> bool {
    true
}

fn default_max_iterations() -> usize {
    10
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model_id: "gpt-4o-mini".to_string(),
            max_context_tokens: 128000,
            temperature: 0.7,
            max_tokens: None,
            react_enabled: true,
            system_prompt: None,
            max_iterations: 10,
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
    pub memory: Option<MemoryConfig>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {path}"))?;
        let mut config: Config =
            toml::from_str(&content).with_context(|| format!("failed to parse config from {path}"))?;

        for model in config.models.values_mut() {
            model.api_key = resolve_env_value(&model.api_key);
        }

        Ok(config)
    }

    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY not set and no config.toml found")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let model_id = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".to_string());

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
        })
    }

    pub fn get_model(&self, name: &str) -> Option<&ModelConfig> {
        self.models.get(name)
    }

    pub fn default_model(&self) -> &ModelConfig {
        self.models
            .get(&self.default_model)
            .expect("default model not found")
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
