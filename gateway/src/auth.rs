//! API key auth — Bearer token or HTTP Basic (key as username, empty password).

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;

pub struct AuthUser {
    pub key_name: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;

        let presented = if let Some(token) = header.strip_prefix("Bearer ") {
            token.trim().to_string()
        } else if let Some(encoded) = header.strip_prefix("Basic ") {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|_| ApiError::Unauthorized("invalid Basic auth encoding".into()))?;
            let pair = String::from_utf8(decoded)
                .map_err(|_| ApiError::Unauthorized("invalid Basic auth encoding".into()))?;
            // Cursor-style: API key as username, password ignored / empty.
            pair.split(':').next().unwrap_or("").to_string()
        } else {
            return Err(ApiError::Unauthorized(
                "Authorization must be Bearer <key> or Basic".into(),
            ));
        };

        if presented.is_empty() || presented != state.api_key {
            return Err(ApiError::Unauthorized("invalid API key".into()));
        }

        Ok(AuthUser {
            key_name: state.api_key_name.clone(),
        })
    }
}
