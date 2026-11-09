use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};

use serde::{Deserialize, Serialize};

use crate::endpoints::sync::require_sync_enabled;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/security/master-key-export",
            post(export_network_secret_backup),
        )
        .route("/security/master-key-export/qr", post(render_qr))
}

#[derive(Deserialize)]
struct ExportRequest {
    passphrase: String,
}

#[derive(Serialize)]
struct ExportResponse {
    armored_text: String,
}

#[derive(Deserialize)]
struct QrRequest {
    text: String,
}

#[derive(Serialize)]
struct QrResponse {
    svg: String,
}

async fn export_network_secret_backup(
    state: axum::extract::State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<ExportRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    if body.passphrase.len() < 8 {
        return ApiError::BadRequest("passphrase must be at least 8 characters".into())
            .into_response();
    }

    let ns = match state.sync_network_secret.read().unwrap().clone() {
        Some(n) => n,
        None => {
            return ApiError::NotFound("network secret not initialized".into()).into_response();
        }
    };

    let key_bytes = ns.to_bytes();
    match rua_sync::export::encrypt_for_export(&key_bytes, &body.passphrase) {
        Ok(armored) => (
            StatusCode::OK,
            Json(ExportResponse {
                armored_text: armored,
            }),
        )
            .into_response(),
        Err(e) => ApiError::Internal(format!("encryption failed: {e}")).into_response(),
    }
}

async fn render_qr(
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<QrRequest>,
) -> Response {
    if body.text.is_empty() {
        return ApiError::BadRequest("text is required".into()).into_response();
    }

    match rua_sync::export::render_qr_svg(&body.text) {
        Ok(svg) => (StatusCode::OK, Json(QrResponse { svg })).into_response(),
        Err(e) => ApiError::Internal(format!("qr render failed: {e}")).into_response(),
    }
}
