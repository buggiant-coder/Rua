use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures::stream::{unfold, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    agent_id: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/approval/stream", get(approval_stream))
}

async fn approval_stream(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
) -> Response {
    let rx = state.security.approval_authority().subscribe();
    let agent_filter = query.agent_id;

    let stream = unfold(rx, move |mut rx| {
        let agent_filter = agent_filter.clone();
        async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Some(ref filter) = agent_filter {
                            if event.request.agent_id != *filter {
                                continue;
                            }
                        }
                        let mut line = serde_json::to_string(&event).unwrap_or_default();
                        line.push('\n');
                        return Some((Ok::<_, std::convert::Infallible>(line), rx));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }
    })
    .fuse();

    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(stream))
        .unwrap()
}
