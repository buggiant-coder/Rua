use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub since: Option<String>,
    pub limit: Option<usize>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/security/audit", get(get_audit_entries))
}

async fn get_audit_entries(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Query(params): Query<AuditQuery>,
) -> Response {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

    let since = match &params.since {
        Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Some(dt.with_timezone(&chrono::Utc)),
            Err(_) => {
                let body = serde_json::json!({
                    "error": "invalid 'since' parameter: expected ISO 8601 / RFC 3339"
                });
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        },
        None => None,
    };

    let audit_dir = state.rua_root.join("audit");
    let entries = rua_security::read_audit_entries(&audit_dir, since, limit);

    (StatusCode::OK, Json(entries)).into_response()
}
