use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;

use rua_channel::core::console::{
    ConsoleChatRequest, ConsoleSubmitKind, ConsoleSubmitOutcome, SseOutputStream,
};
use rua_core::ChatAddress;

use crate::errors::ApiError;
use crate::services::conversation_authority::SUBMISSION_ID_KEY;
use crate::state::AppState;

pub async fn submit_single_turn(
    state: &AppState,
    address: ChatAddress,
    execution_agent_id: &str,
    mut request: ConsoleChatRequest,
) -> Response {
    let expected_turn_id = format!("turn-cli-{}", uuid::Uuid::new_v4());
    request.extra.insert(
        "turn_id".to_string(),
        serde_json::Value::String(expected_turn_id.clone()),
    );
    request.extra.insert(
        SUBMISSION_ID_KEY.to_string(),
        serde_json::Value::String(format!("submission-cli-{}", uuid::Uuid::new_v4())),
    );
    let viewer_id = format!("cli-{}", uuid::Uuid::new_v4());
    let live = state
        .console_facade
        .connect_scoped_chat_activity(address.clone(), execution_agent_id, &viewer_id, None)
        .await;
    let outcome = state
        .console_facade
        .submit_scoped_chat(address, execution_agent_id, request)
        .await;

    if !outcome.accepted || matches!(outcome.kind, ConsoleSubmitKind::Rejected) {
        return rejected_response(outcome);
    }

    let stream = single_turn_events(live, expected_turn_id);
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

fn rejected_response(outcome: ConsoleSubmitOutcome) -> Response {
    let detail = outcome
        .detail
        .unwrap_or_else(|| "Chat submission was rejected".to_string());
    ApiError::Conflict(detail).into_response()
}

fn single_turn_events(
    live: SseOutputStream,
    expected_turn_id: String,
) -> impl futures::Stream<Item = Result<Event, Infallible>> + Send + 'static {
    struct State {
        live: SseOutputStream,
        expected_turn_id: String,
        before_start: Vec<String>,
        pending: VecDeque<String>,
        started: bool,
        done: bool,
    }

    futures::stream::unfold(
        State {
            live,
            expected_turn_id,
            before_start: Vec::new(),
            pending: VecDeque::new(),
            started: false,
            done: false,
        },
        |mut state| async move {
            loop {
                if let Some(payload) = state.pending.pop_front() {
                    state.done = payload_ends_turn(&payload, &state.expected_turn_id);
                    return Some((Ok(Event::default().data(payload)), state));
                }
                if state.done {
                    return None;
                }

                let item = state.live.next().await?;
                let payload = match item {
                    Ok(payload) => payload,
                    Err(never) => match never {},
                };
                for payload in expand_live_payload(&payload) {
                    if state.started {
                        state.pending.push_back(payload);
                        continue;
                    }

                    let starts_expected_turn =
                        payload_starts_turn(&payload, &state.expected_turn_id);
                    state.before_start.push(payload);
                    if starts_expected_turn {
                        state.started = true;
                        let retain_from = state
                            .before_start
                            .iter()
                            .rposition(|payload| {
                                payload_is_other_turn_terminal(payload, &state.expected_turn_id)
                            })
                            .map(|index| index + 1)
                            .unwrap_or(0);
                        state
                            .pending
                            .extend(state.before_start.drain(retain_from..));
                        state.before_start.clear();
                    }
                }
            }
        },
    )
}

fn expand_live_payload(payload: &str) -> Vec<String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    if value.get("type").and_then(serde_json::Value::as_str) != Some("stream_replay") {
        return vec![value.to_string()];
    }

    value
        .get("events")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|event| match event {
            serde_json::Value::String(raw) => expand_live_payload(raw),
            value => expand_live_payload(&value.to_string()),
        })
        .collect()
}

fn payload_starts_turn(payload: &str, expected_turn_id: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    if value.get("type").and_then(serde_json::Value::as_str) == Some("conversation_control") {
        return value
            .pointer("/control/active_turn/turn_id")
            .and_then(serde_json::Value::as_str)
            == Some(expected_turn_id);
    }
    payload_turn_id(&value) == Some(expected_turn_id)
}

fn payload_is_other_turn_terminal(payload: &str, expected_turn_id: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    value.get("type").and_then(serde_json::Value::as_str) == Some("turn_terminal")
        && payload_turn_id(&value) != Some(expected_turn_id)
}

fn payload_ends_turn(payload: &str, expected_turn_id: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("error")
    ) || (value.get("type").and_then(serde_json::Value::as_str) == Some("turn_terminal")
        && payload_turn_id(&value) == Some(expected_turn_id))
}

fn payload_turn_id(value: &serde_json::Value) -> Option<&str> {
    value.get("turn_id").and_then(serde_json::Value::as_str)
}
