use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::stream::{unfold, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::state::AppState;

#[derive(Deserialize)]
struct StreamQuery {
    task_id: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/local-models/download/stream", get(download_stream))
}

async fn download_stream(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
) -> Response {
    let rx = match state.local_model.subscribe_download(&query.task_id).await {
        Ok(rx) => rx,
        Err(e) => {
            let body = serde_json::json!({"error": e.to_string()});
            return (axum::http::StatusCode::NOT_FOUND, axum::Json(body)).into_response();
        }
    };

    let stream = unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    let mut line = serde_json::to_string(&msg).unwrap_or_default();
                    line.push('\n');
                    return Some((Ok::<_, std::convert::Infallible>(line), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .fuse();

    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::TRANSFER_ENCODING, "chunked")
        .body(Body::from_stream(stream))
        .unwrap()
}
