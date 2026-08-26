//! `ageverse config show|validate` subcommands.

use crate::bootstrap::resolve_config_path;
use agent_core::{
    Config, Message, ModelConfig, client::OpenAIClient, default_config_path, resolve_env_value,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

pub struct ConfigShowArgs {
    pub config: Option<String>,
}

pub struct ConfigValidateArgs {
    pub config: Option<String>,
    pub probe: bool,
}

pub async fn run_config_show(args: ConfigShowArgs) -> anyhow::Result<ExitCode> {
    let path = config_path(args.config.as_deref());
    let cfg = load_existing(&path)?;
    print_config_redacted(&path, &cfg);
    Ok(ExitCode::SUCCESS)
}

pub async fn run_config_validate(args: ConfigValidateArgs) -> anyhow::Result<ExitCode> {
    let path = config_path(args.config.as_deref());
    let cfg = load_existing(&path)?;
    println!("OK: syntax and models for {}", path.display());
    println!("  default_model: {}", cfg.default_model);
    println!("  models: {}", cfg.models.len());
    println!("  providers: {}", cfg.providers.len());

    if args.probe {
        probe_default_model(&cfg).await?;
    }
    Ok(ExitCode::SUCCESS)
}

fn config_path(override_path: Option<&str>) -> PathBuf {
    resolve_config_path(override_path).unwrap_or_else(default_config_path)
}

fn load_existing(path: &Path) -> anyhow::Result<Config> {
    if !path.exists() {
        anyhow::bail!(
            "config not found: {}\nCreate one or pass --config <path>",
            path.display()
        );
    }
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("config path is not valid UTF-8"))?;
    Config::load(path_str)
}

fn redact_secret(raw: &str) -> String {
    if raw.starts_with("${") && raw.ends_with('}') {
        return format!("{raw} (env ref)");
    }
    if raw.is_empty() {
        return "(empty)".into();
    }
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().min(8));
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{suffix}")
}

fn print_config_redacted(path: &Path, cfg: &Config) {
    println!("# Effective config from {}", path.display());
    println!("default_model = {:?}", cfg.default_model);
    if let Some(ref m) = cfg.btw_model {
        println!("btw_model = {m:?}");
    }
    if let Some(ref m) = cfg.learn_model {
        println!("learn_model = {m:?}");
    }
    println!("reflector_enabled = {}", cfg.reflector_enabled);
    println!();
    println!("[permissions]");
    println!("mode = {:?}", cfg.permissions.mode);
    println!();

    let mut providers: Vec<_> = cfg.providers.keys().collect();
    providers.sort();
    for key in providers {
        let p = &cfg.providers[key];
        println!("[providers.{key}]");
        println!("name = {:?}", p.name);
        println!("base_url = {:?}", p.base_url);
        println!("api_key = {:?}", redact_secret(&p.api_key));
        println!("max_context_tokens = {}", p.max_context_tokens);
        println!("models = {}", p.models.len());
        println!();
    }

    let mut models: Vec<_> = cfg.models.keys().collect();
    models.sort();
    println!("# Flattened models ({}):", models.len());
    for key in models {
        let m = &cfg.models[key];
        let marker = if *key == cfg.default_model || key.as_str() == cfg.default_model {
            " *"
        } else {
            ""
        };
        println!(
            "  {key}{marker}: model_id={} base_url={} api_key={}",
            m.model_id,
            m.base_url,
            redact_secret(&m.api_key)
        );
    }
}

async fn probe_default_model(cfg: &Config) -> anyhow::Result<()> {
    let model = cfg.default_model()?.clone();
    println!(
        "Probing default model {} ({} @ {}) …",
        cfg.default_model, model.model_id, model.base_url
    );

    let resolved = resolve_env_value(&model.api_key);
    if resolved.is_empty() || resolved.starts_with("${") {
        anyhow::bail!(
            "api_key for '{}' is empty or unresolved env ref '{}'",
            cfg.default_model,
            model.api_key
        );
    }

    let mut probe_model = ModelConfig {
        request_timeout_secs: 20,
        max_tokens: Some(1),
        ..model
    };
    // Avoid long provider timeouts during CLI validate.
    probe_model.request_timeout_secs = 20;

    let client = OpenAIClient::new(probe_model);
    // Tiny completion — validates base_url + key without needing /models.
    tokio::time::timeout(
        Duration::from_secs(25),
        client.chat_completion(&[Message::user("ping")], &[]),
    )
    .await
    .map_err(|_| anyhow::anyhow!("provider probe timed out after 25s"))?
    .map_err(|e| anyhow::anyhow!("provider probe failed: {e:#}"))?;

    println!("OK: provider responded");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::redact_secret;

    #[test]
    fn redacts_long_keys() {
        assert_eq!(redact_secret("sk-abcdefghijklmnop"), "sk-a…mnop");
    }

    #[test]
    fn keeps_env_refs() {
        assert_eq!(
            redact_secret("${OPENAI_API_KEY}"),
            "${OPENAI_API_KEY} (env ref)"
        );
    }

    #[test]
    fn short_keys_fully_masked() {
        assert_eq!(redact_secret("short"), "*****");
    }
}
