use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use rua_core::{ChatAddress, RuaError, RuntimeEvent};

use super::{ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitKind, SseOutputStream};

pub const EXTERNAL_RUNTIME_EVENTS: &str = "external_runtime_events";

pub type ExternalTurnStream = Pin<Box<dyn Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>;

async fn listen_for_turn(
    facade: &dyn ConsoleChatFacade,
    address: &ChatAddress,
    execution_agent_id: &str,
    request: &mut ConsoleChatRequest,
    viewer: &str,
) -> (String, SseOutputStream) {
    let id = uuid::Uuid::new_v4();
    if viewer == "cli" {
        request.extra.remove("turn_id");
        request.extra.remove("submission_id");
    }
    let turn_id = request
        .extra
        .get("turn_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("turn-{viewer}-{id}"));
    request
        .extra
        .insert("turn_id".into(), serde_json::json!(turn_id));
    request
        .extra
        .entry("submission_id")
        .or_insert_with(|| serde_json::json!(format!("submission-{viewer}-{id}")));
    let live = facade
        .connect_scoped_chat_activity(
            address.clone(),
            execution_agent_id,
            &format!("{viewer}-{id}"),
            None,
        )
        .await;
    (turn_id, live)
}

async fn admit_turn(
    facade: &dyn ConsoleChatFacade,
    address: ChatAddress,
    execution_agent_id: &str,
    request: ConsoleChatRequest,
) -> Result<Option<String>, String> {
    let outcome = facade
        .submit_scoped_chat(address, execution_agent_id, request)
        .await;
    if !outcome.accepted || matches!(outcome.kind, ConsoleSubmitKind::Rejected) {
        return Err(outcome
            .detail
            .unwrap_or_else(|| "Chat submission was rejected".into()));
    }
    Ok(outcome.input_id)
}

pub async fn submit_display_turn(
    facade: &dyn ConsoleChatFacade,
    address: ChatAddress,
    execution_agent_id: &str,
    mut request: ConsoleChatRequest,
) -> Result<SseOutputStream, String> {
    let (turn_id, live) =
        listen_for_turn(facade, &address, execution_agent_id, &mut request, "cli").await;
    admit_turn(facade, address, execution_agent_id, request).await?;
    Ok(Box::pin(single_turn_events(live, turn_id)))
}

struct CancelExternalTurn {
    facade: Arc<dyn ConsoleChatFacade>,
    address: ChatAddress,
    execution_agent_id: String,
    turn_id: String,
    input_id: Option<String>,
    completed: bool,
}

impl Drop for CancelExternalTurn {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let facade = Arc::clone(&self.facade);
        let address = self.address.clone();
        let agent_id = self.execution_agent_id.clone();
        let turn_id = self.turn_id.clone();
        let input_id = self.input_id.take();
        handle.spawn(async move {
            facade
                .cancel_submitted_turn(address, &agent_id, &turn_id, input_id.as_deref())
                .await;
        });
    }
}

/// Submit through the Chat authority, retaining the channel's runtime event protocol.
pub async fn submit_runtime_turn(
    facade: Arc<dyn ConsoleChatFacade>,
    address: ChatAddress,
    execution_agent_id: &str,
    mut request: ConsoleChatRequest,
) -> Result<ExternalTurnStream, String> {
    request
        .extra
        .insert(EXTERNAL_RUNTIME_EVENTS.into(), true.into());
    let (turn_id, live) = listen_for_turn(
        facade.as_ref(),
        &address,
        execution_agent_id,
        &mut request,
        "channel",
    )
    .await;
    let execution_agent_id = execution_agent_id.to_owned();
    let submitted_turn_id = turn_id.clone();
    let (receipt, accepted) = tokio::sync::oneshot::channel();
    // Admission can finish after the caller disconnects. Delivering this guard
    // to a closed receiver withdraws the precise authority receipt in that case.
    tokio::spawn(async move {
        if receipt.is_closed() {
            return;
        }
        let result = admit_turn(
            facade.as_ref(),
            address.clone(),
            &execution_agent_id,
            request,
        )
        .await
        .map(|input_id| CancelExternalTurn {
            facade,
            address,
            execution_agent_id,
            turn_id: submitted_turn_id,
            input_id,
            completed: false,
        });
        let _ = receipt.send(result);
    });
    let guard = accepted
        .await
        .map_err(|_| "Chat admission stopped before returning its receipt")??;
    let events = futures::stream::try_unfold(
        (live, VecDeque::new(), guard),
        move |(mut live, mut pending, mut guard)| {
            let turn_id = turn_id.clone();
            async move {
                loop {
                    if let Some(event) = pending.pop_front() {
                        return Ok(Some((event?, (live, pending, guard))));
                    }
                    if guard.completed {
                        return Ok(None);
                    }
                    let Some(Ok(frame)) = live.next().await else {
                        return Err(RuaError::Runtime {
                            message: "Chat event stream closed before the turn completed".into(),
                        });
                    };
                    for payload in expand_live_payload(&frame) {
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                            continue;
                        };
                        if payload_turn_id(&value) != Some(&turn_id) {
                            continue;
                        }
                        match value["type"].as_str() {
                            Some("external_runtime_event") => {
                                let event = serde_json::from_value(value["event"].clone())
                                    .map_err(|err| RuaError::Runtime {
                                        message: err.to_string(),
                                    })?;
                                pending.push_back(match event {
                                    RuntimeEvent::Error { error } => {
                                        guard.completed = true;
                                        Err(error)
                                    }
                                    event => Ok(event),
                                });
                            }
                            Some("turn_terminal") => {
                                let status = value["status"].as_str().unwrap_or("unknown");
                                if status == "completed" {
                                    pending.push_back(Ok(RuntimeEvent::End {
                                        finish_reason: Some("completed".into()),
                                    }));
                                } else {
                                    pending.push_back(Err(RuaError::Runtime {
                                        message: format!("Chat turn {turn_id} ended: {status}"),
                                    }));
                                }
                                guard.completed = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        },
    );
    Ok(Box::pin(events))
}

fn single_turn_events(
    live: SseOutputStream,
    expected_turn_id: String,
) -> impl futures::Stream<Item = Result<String, Infallible>> + Send + 'static {
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
                    return Some((Ok(payload), state));
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
