//! Shared application state.

use crate::store::AgentStore;
use agent_core::{RunManager, SessionManager};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub run_manager: Arc<Mutex<RunManager>>,
    pub session_manager: Arc<SessionManager>,
    pub agent_store: Arc<AgentStore>,
    pub api_key: String,
    pub api_key_name: String,
    /// Base URL used for artifact download links (e.g. http://127.0.0.1:8787).
    pub public_base_url: String,
}
