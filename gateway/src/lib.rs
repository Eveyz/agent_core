//! Agverse HTTP gateway — Cursor Cloud Agents API–inspired control plane.

mod agents;
mod artifacts;
mod auth;
mod error;
mod meta;
mod models;
mod runs;
mod state;
mod store;
mod stream;
mod workspace;

pub use state::AppState;
pub use store::{AgentStore, agents_dir, gateway_dir};

use axum::Router;
use axum::routing::{get, post};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Build the `/v1` API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(meta::health))
        .route("/v1/me", get(meta::api_key_info))
        .route("/v1/models", get(meta::list_models))
        .route(
            "/v1/agents",
            post(agents::create_agent).get(agents::list_agents),
        )
        .route(
            "/v1/agents/{id}",
            get(agents::get_agent).delete(agents::delete_agent),
        )
        .route("/v1/agents/{id}/archive", post(agents::archive_agent))
        .route("/v1/agents/{id}/unarchive", post(agents::unarchive_agent))
        .route(
            "/v1/agents/{id}/runs",
            post(runs::create_run).get(runs::list_runs),
        )
        .route("/v1/agents/{id}/runs/{runId}", get(runs::get_run))
        .route("/v1/agents/{id}/runs/{runId}/stream", get(runs::stream_run))
        .route(
            "/v1/agents/{id}/runs/{runId}/cancel",
            post(runs::cancel_run),
        )
        .route("/v1/agents/{id}/runs/{runId}/approve", post(runs::approve))
        .route("/v1/agents/{id}/runs/{runId}/answer", post(runs::answer))
        .route("/v1/agents/{id}/artifacts", get(artifacts::list_artifacts))
        .route(
            "/v1/agents/{id}/artifacts/download",
            get(artifacts::download_artifact),
        )
        .route(
            "/v1/agents/{id}/artifacts/content",
            get(artifacts::artifact_content),
        )
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
