use crate::{errors::ApiError, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rua_core::phone_use::PhoneStatus;

pub fn router() -> Router<AppState> {
    Router::new().route("/platform/phone", get(status))
}

async fn status(State(state): State<AppState>) -> Result<Json<PhoneStatus>, ApiError> {
    if state.profile != crate::profiles::ServerProfile::MobileApk {
        return Err(ApiError::Unavailable(
            "Phone Use requires an Android execution device".into(),
        ));
    }
    let bridge = rua_core::phone_use::bridge()
        .ok_or_else(|| ApiError::Unavailable("Phone Use native service is not connected".into()))?;
    bridge
        .status(&tokio_util::sync::CancellationToken::new())
        .await
        .map(Json)
        .map_err(ApiError::Unavailable)
}
