use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/register", post(register))
        .route("/status", get(status))
        .route("/verify", get(verify))
        .route("/update-profile", post(update_profile))
        .route("/revoke-token", post(revoke_token))
        .route("/revoke-all-tokens", post(revoke_all_tokens))
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

async fn login(State(state): State<AppState>, Json(body): Json<dto::LoginRequest>) -> Response {
    if state.profile == crate::profiles::ServerProfile::MobileApk
        && !state.auth.has_registered_users()
    {
        return ApiError::Unauthorized(
            "Set up account access from the authenticated Rua app".into(),
        )
        .into_response();
    }
    if !state.auth.is_active() && state.profile != crate::profiles::ServerProfile::MobileApk {
        let resp = dto::LoginResponse {
            token: String::new(),
            username: body.username,
            extra: serde_json::Map::new(),
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    if body.username.is_empty() {
        return ApiError::BadRequest("Username is required".into()).into_response();
    }

    if !state.auth.has_registered_users() {
        if body.password.is_empty() {
            return ApiError::BadRequest("Password is required for first login".into())
                .into_response();
        }
        match state
            .auth
            .register_user(&body.username, &body.password, body.expires_in)
        {
            Some(token) => {
                let resp = dto::LoginResponse {
                    token,
                    username: body.username,
                    extra: serde_json::Map::new(),
                };
                return (StatusCode::OK, Json(resp)).into_response();
            }
            None => {
                return ApiError::Internal("Registration failed".into()).into_response();
            }
        }
    }

    match state
        .auth
        .authenticate(&body.username, &body.password, body.expires_in)
    {
        Some(token) => {
            let resp = dto::LoginResponse {
                token,
                username: body.username,
                extra: serde_json::Map::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        None => ApiError::Unauthorized("Invalid credentials".into()).into_response(),
    }
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<dto::RegisterRequest>,
) -> Response {
    if state.auth.has_registered_users() {
        return ApiError::Forbidden("User already registered".into()).into_response();
    }

    if body.username.is_empty() || body.password.is_empty() {
        return ApiError::BadRequest("Username and password are required".into()).into_response();
    }

    match state
        .auth
        .register_user(&body.username, &body.password, body.expires_in)
    {
        Some(token) => {
            let resp = dto::LoginResponse {
                token,
                username: body.username,
                extra: serde_json::Map::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        None => ApiError::Conflict("Registration failed".into()).into_response(),
    }
}

async fn status(State(state): State<AppState>) -> Response {
    let resp = dto::AuthStatusResponse {
        enabled: state.auth.is_enabled()
            || state.profile == crate::profiles::ServerProfile::MobileApk,
        has_users: state.auth.has_registered_users(),
        extra: serde_json::Map::new(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn verify(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.auth.is_enabled() && state.profile != crate::profiles::ServerProfile::MobileApk {
        return (
            StatusCode::OK,
            Json(serde_json::json!({"valid": true, "username": ""})),
        )
            .into_response();
    }

    let token = match extract_bearer(&headers) {
        Some(t) => t,
        None => return ApiError::Unauthorized("No token provided".into()).into_response(),
    };

    match state.auth.verify_token(&token) {
        Some(username) => (
            StatusCode::OK,
            Json(serde_json::json!({"valid": true, "username": username})),
        )
            .into_response(),
        None => ApiError::Unauthorized("Invalid or expired token".into()).into_response(),
    }
}

async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<dto::UpdateProfileRequest>,
) -> Response {
    if !state.auth.is_enabled() {
        return ApiError::Forbidden("Authentication is not enabled".into()).into_response();
    }

    if !state.auth.has_registered_users() {
        return ApiError::Forbidden("No user registered".into()).into_response();
    }

    let caller_token = match extract_bearer(&headers) {
        Some(t) => t,
        None => return ApiError::Unauthorized("Not authenticated".into()).into_response(),
    };
    if state.auth.verify_token(&caller_token).is_none() {
        return ApiError::Unauthorized("Not authenticated".into()).into_response();
    }

    if body.new_password.is_none() && body.new_username.is_none() {
        return ApiError::BadRequest("Nothing to update".into()).into_response();
    }

    if let Some(ref name) = body.new_username {
        if name.trim().is_empty() {
            return ApiError::BadRequest("Username cannot be empty".into()).into_response();
        }
    }

    if let Some(ref pw) = body.new_password {
        if pw.trim().is_empty() {
            return ApiError::BadRequest("Password cannot be empty".into()).into_response();
        }
    }

    match state.auth.update_credentials(
        &body.current_password,
        body.new_username.as_deref(),
        body.new_password.as_deref(),
        body.expires_in,
    ) {
        Some(token) => {
            let username = state.auth.verify_token(&token).unwrap_or_default();
            let resp = dto::LoginResponse {
                token,
                username,
                extra: serde_json::Map::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        None => ApiError::Unauthorized("Current password is incorrect".into()).into_response(),
    }
}

async fn revoke_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<dto::RevokeTokenRequest>,
) -> Response {
    if !state.auth.is_enabled() {
        return ApiError::Forbidden("Authentication is not enabled".into()).into_response();
    }

    let caller_token = match extract_bearer(&headers) {
        Some(t) => t,
        None => return ApiError::Unauthorized("Not authenticated".into()).into_response(),
    };
    if state.auth.verify_token(&caller_token).is_none() {
        return ApiError::Unauthorized("Not authenticated".into()).into_response();
    }

    let token_to_revoke = match body.token {
        Some(ref t) if !t.is_empty() => t.clone(),
        _ => caller_token.clone(),
    };
    let is_current = token_to_revoke == caller_token;

    if state.auth.revoke_token(&token_to_revoke) {
        let message = if is_current {
            "Current token has been revoked. Please login again."
        } else {
            "Specified token has been revoked."
        };
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": message,
                "revoked": true,
                "revoked_current_token": is_current,
            })),
        )
            .into_response()
    } else {
        ApiError::Internal("Failed to revoke token".into()).into_response()
    }
}

async fn revoke_all_tokens(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.auth.is_enabled() {
        return ApiError::Forbidden("Authentication is not enabled".into()).into_response();
    }

    let caller_token = match extract_bearer(&headers) {
        Some(t) => t,
        None => return ApiError::Unauthorized("Not authenticated".into()).into_response(),
    };
    if state.auth.verify_token(&caller_token).is_none() {
        return ApiError::Unauthorized("Not authenticated".into()).into_response();
    }

    if state.auth.revoke_all_tokens() {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": "All tokens have been revoked. Please login again.",
                "revoked": true,
            })),
        )
            .into_response()
    } else {
        ApiError::Internal("Failed to revoke tokens".into()).into_response()
    }
}
