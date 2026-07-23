//! Meta endpoints — API key info + models list.

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{ApiKeyInfo, ListModelsResponse, ModelInfo};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;

pub async fn api_key_info(
    auth: AuthUser,
    State(_state): State<AppState>,
) -> Result<Json<ApiKeyInfo>, ApiError> {
    Ok(Json(ApiKeyInfo {
        api_key_name: auth.key_name,
        user_email: None,
    }))
}

pub async fn list_models(
    _auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<ListModelsResponse>, ApiError> {
    let rm = state.run_manager.lock().await;
    let mut ids: Vec<String> = rm.brain().config.models.keys().cloned().collect();
    ids.sort();
    Ok(Json(ListModelsResponse {
        items: ids.into_iter().map(|id| ModelInfo { id }).collect(),
    }))
}

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "agverse-gateway" }))
}
