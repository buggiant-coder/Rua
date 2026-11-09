use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use rua_doctor::fix;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/doctor/fix/{fix_id}", post(post_doctor_fix))
}

async fn post_doctor_fix(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(fix_id): Path<String>,
) -> Response {
    if fix::is_interactive_fix(&fix_id) {
        let body = serde_json::json!({
            "detail": format!(
                "fix {fix_id:?} requires interactive confirmation and cannot be run via HTTP"
            )
        });
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }

    if !fix::is_known_fix(&fix_id) {
        let body = serde_json::json!({
            "detail": format!("unknown fix_id: {fix_id:?}")
        });
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }

    let result = fix::run_fix(&fix_id, &state.rua_root);
    let status = if result.success {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };

    (status, Json(result)).into_response()
}
