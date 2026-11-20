use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use rua_channel::core::console::{submit_display_turn, ConsoleChatRequest};
use rua_core::ChatAddress;

use crate::errors::ApiError;
use crate::state::AppState;

pub async fn submit_single_turn(
    state: &AppState,
    address: ChatAddress,
    execution_agent_id: &str,
    request: ConsoleChatRequest,
) -> Response {
    let stream = match submit_display_turn(
        state.console_facade.as_ref(),
        address,
        execution_agent_id,
        request,
    )
    .await
    {
        Ok(stream) => stream,
        Err(detail) => return ApiError::Conflict(detail).into_response(),
    };
    Sse::new(stream.map(|item| item.map(|payload| Event::default().data(payload))))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}
