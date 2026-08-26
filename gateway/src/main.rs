//! `agverse-gateway` — remote host for Agverse agents.

use agent_core::{Brain, RunManager, SessionManager, load_or_init_default};
use agverse_gateway::{AgentStore, AppState, agents_dir, router};
use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let api_key = std::env::var("AGVERSE_API_KEY").unwrap_or_else(|_| {
        // Dev convenience — refuse empty in production via explicit check below.
        String::new()
    });
    if api_key.is_empty() {
        bail!("AGVERSE_API_KEY is required (Bearer / Basic API key for the gateway)");
    }
    let api_key_name =
        std::env::var("AGVERSE_API_KEY_NAME").unwrap_or_else(|_| "default".to_string());

    let bind = std::env::var("AGVERSE_GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let addr: SocketAddr = bind.parse().context("invalid AGVERSE_GATEWAY_BIND")?;
    let public_base_url =
        std::env::var("AGVERSE_GATEWAY_PUBLIC_URL").unwrap_or_else(|_| format!("http://{addr}"));

    let config_path = std::env::var("AGVERSE_CONFIG")
        .ok()
        .map(std::path::PathBuf::from);
    let (config, loaded_path) = load_or_init_default(config_path.as_deref())?;
    tracing::info!(path = %loaded_path.display(), "loaded config");

    let brain = Brain::from_config(config)?;
    let session_db = agent_core::paths::get_memory_db_path();
    let session_storage =
        agent_core::memory::storage::Storage::new(session_db.to_string_lossy().as_ref())
            .context("open session DB")?;
    let session_manager = Arc::new(SessionManager::new(session_storage));
    let run_manager = RunManager::new(brain).with_session_manager(session_manager.clone());

    let agent_store = Arc::new(AgentStore::open(agents_dir())?);

    let state = AppState {
        run_manager: Arc::new(Mutex::new(run_manager)),
        session_manager,
        agent_store,
        api_key,
        api_key_name,
        public_base_url: public_base_url.clone(),
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, %public_base_url, "agverse-gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}
