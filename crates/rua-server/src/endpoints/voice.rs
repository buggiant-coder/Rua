use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequest, Multipart, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use futures::{SinkExt, StreamExt};
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;
use rua_voice::twilio as voice_twilio;

const ASR_ALLOWED_EXTENSIONS: &[&str] = &["webm", "mp4", "m4a", "wav", "mp3", "ogg", "flac"];

fn validate_asr_filename(filename: &str) -> Result<(), Box<Response>> {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty())
        .unwrap_or_else(|| "webm".to_string());
    if ASR_ALLOWED_EXTENSIONS
        .iter()
        .any(|allowed| allowed == &ext.as_str())
    {
        return Ok(());
    }

    let allowed = ASR_ALLOWED_EXTENSIONS
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = serde_json::json!({
        "detail": {
            "code": "UNSUPPORTED_FILE_TYPE",
            "message": format!("Unsupported file type: .{ext}. Allowed: {allowed}"),
        }
    });
    Err(Box::new(
        (StatusCode::BAD_REQUEST, Json(body)).into_response(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct TtsSynthesizeRequest {
    pub text: String,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
}

async fn voice_channel_config(state: &AppState) -> Result<serde_json::Value, ApiError> {
    let channels = state
        .agent_cells
        .section::<serde_json::Value>(state.voice.agent_id(), "channels")
        .read()
        .await
        .map_err(|error| ApiError::Internal(format!("voice channel config: {error}")))?;
    let saved = channels
        .as_ref()
        .and_then(|value| value.get("voice"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut config = rua_config::channel_catalog::resolve_channel_config("voice", &saved)
        .map_err(ApiError::BadRequest)?;
    if config["public_url"]
        .as_str()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        if let Some(url) = state.voice_runtime.public_url().await {
            config["public_url"] = serde_json::json!(url);
        }
    }
    Ok(config)
}

fn signed_voice_webhook(
    config: &serde_json::Value,
    path: &str,
    headers: &HeaderMap,
    params: &BTreeMap<String, String>,
) -> bool {
    let url = format!(
        "{}{path}",
        config["public_url"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches('/')
    );
    voice_twilio::validate_signature(
        config["twilio_auth_token"].as_str().unwrap_or(""),
        &url,
        params,
        headers
            .get("x-twilio-signature")
            .and_then(|value| value.to_str().ok())
            .unwrap_or(""),
    )
}

pub async fn voice_incoming(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(params): Form<BTreeMap<String, String>>,
) -> Response {
    let config = match voice_channel_config(&state).await {
        Ok(config) => config,
        Err(error) => return error.into_response(),
    };
    if !state.voice.is_enabled() || config["enabled"] != true || config["dm_disabled"] == true {
        return (
            StatusCode::OK,
            [("Content-Type", "application/xml")],
            voice_twilio::render_reject_twiml(),
        )
            .into_response();
    }
    if !signed_voice_webhook(&config, "/voice/incoming", &headers, &params) {
        return ApiError::Unauthorized("invalid Twilio signature".into()).into_response();
    }
    let body: dto::TwilioIncomingRequest = match serde_json::from_value(serde_json::json!(params)) {
        Ok(body) => body,
        Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
    };
    let expected_account = config["twilio_account_sid"].as_str().unwrap_or("");
    let expected_phone = config["phone_number"].as_str().unwrap_or("");
    let gate: rua_channel::core::config::BaseChannelConfig =
        serde_json::from_value(config.clone()).unwrap_or_default();
    if (!expected_account.is_empty() && body.account_sid.as_deref() != Some(expected_account))
        || (!expected_phone.is_empty() && body.to != expected_phone)
        || !rua_channel::core::allowlist::ChannelAcl::from_config(&gate)
            .is_allowed(&body.from, false)
    {
        return (
            StatusCode::OK,
            [("Content-Type", "application/xml")],
            voice_twilio::render_reject_twiml(),
        )
            .into_response();
    }
    match state
        .voice
        .handle_incoming_with_config(&body.call_sid, &config)
        .await
    {
        Ok(twiml) => (StatusCode::OK, [("Content-Type", "application/xml")], twiml).into_response(),
        Err(error) => {
            tracing::error!(%error, "voice incoming failed");
            (
                StatusCode::OK,
                [("Content-Type", "application/xml")],
                voice_twilio::render_error_twiml(),
            )
                .into_response()
        }
    }
}

pub async fn voice_ws(
    State(state): State<AppState>,
    Query(query): Query<dto::VoiceWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let config = match voice_channel_config(&state).await {
        Ok(config) => config,
        Err(error) => return error.into_response(),
    };
    if !state.voice.is_enabled() || config["enabled"] != true {
        return ApiError::Unavailable("voice channel is not available".into()).into_response();
    }

    let token = match query.token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return ApiError::Unauthorized("missing voice ws token".into()).into_response();
        }
    };

    match state.voice.validate_ws_token(&token).await {
        Ok(call_sid) => {
            tracing::info!(call_sid = %call_sid, "voice ws connection authenticated");
            ws.on_upgrade(move |socket| handle_voice_ws(socket, state, call_sid))
        }
        Err(e) => {
            tracing::warn!(error = %e, "voice ws token validation failed");
            ApiError::Unauthorized("invalid voice ws token".into()).into_response()
        }
    }
}

async fn handle_voice_ws(socket: WebSocket, state: AppState, call_sid: String) {
    let (mut sender, mut receiver) = socket.split();

    tracing::info!(call_sid = %call_sid, "voice ws session started");

    let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<WsOutMsg>(16);
    let mut outbound = state.voice.attach_outbound(&call_sid).await;
    let mut prompt_buffer = String::new();

    let mut cancel_tx: Option<tokio::sync::watch::Sender<bool>> = None;
    let mut llm_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        tokio::select! {
            biased;


            message = outbound.recv() => {
                let Some(token) = message else { break; };
                let message = dto::ConversationRelayResponse::Text { token, last: true };
                if sender.send(Message::Text(serde_json::to_string(&message).unwrap_or_default().into())).await.is_err() { break; }
            }

            Some(WsOutMsg::Text { token, last }) = resp_rx.recv() => {
                let msg = dto::ConversationRelayResponse::Text { token, last };
                let json = serde_json::to_string(&msg).unwrap_or_default();
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }


            msg_result = receiver.next() => {
                let msg = match msg_result {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        tracing::warn!(call_sid = %call_sid, error = %e, "voice ws receive error");
                        break;
                    }
                    None => break,
                };

                match msg {
                    Message::Text(text) => {
                        let event: dto::ConversationRelayEvent = match serde_json::from_str(&text) {
                            Ok(ev) => ev,
                            Err(e) => {
                                tracing::warn!(
                                    call_sid = %call_sid,
                                    error = %e,
                                    text = %text,
                                    "invalid voice ws frame, skipping"
                                );
                                continue;
                            }
                        };

                        match event {
                            dto::ConversationRelayEvent::Setup {
                                call_sid: setup_sid,
                                from,
                                to,
                                ..
                            } => {
                                if setup_sid.as_deref().is_some_and(|sid| sid != call_sid) {
                                    tracing::warn!("voice setup callSid does not match its token");
                                    break;
                                }
                                let sid = &call_sid;
                                let from_num = from.as_deref().unwrap_or("unknown");
                                let to_num = to.as_deref().unwrap_or("unknown");

                                tracing::info!(
                                    call_sid = %sid,
                                    from = %from_num,
                                    to = %to_num,
                                    "voice ws setup"
                                );

                                if let Err(e) = state.voice.create_session(sid, from_num, to_num).await {
                                    tracing::error!(
                                        call_sid = %sid,
                                        error = %e,
                                        "failed to create voice session"
                                    );
                                    let end = dto::ConversationRelayResponse::End {};
                                    let json = serde_json::to_string(&end).unwrap_or_default();
                                    let _ = sender.send(Message::Text(json.into())).await;
                                    break;
                                }
                            }

                            dto::ConversationRelayEvent::Prompt { voice_prompt, extra } => {
                                if let Some(part) = voice_prompt {
                                    if !prompt_buffer.is_empty() { prompt_buffer.push(' '); }
                                    prompt_buffer.push_str(&part);
                                }
                                if extra.get("last").and_then(serde_json::Value::as_bool) == Some(false) { continue; }
                                let prompt_text = std::mem::take(&mut prompt_buffer);
                                if prompt_text.trim().is_empty() { continue; }
                                cancel_voice_turn(&mut cancel_tx, &mut llm_task).await;
                                while resp_rx.try_recv().is_ok() {}

                                tracing::debug!(
                                    call_sid = %call_sid,
                                    prompt = %prompt_text,
                                    "voice prompt received"
                                );

                                let (tx, rx) = tokio::sync::watch::channel(false);
                                cancel_tx = Some(tx);

                                let voice = state.voice.clone();
                                let sid = call_sid.clone();
                                let resp = resp_tx.clone();
                                llm_task = Some(tokio::spawn(async move {
                                    spawn_prompt_task(voice, sid, prompt_text, Some(rx), resp).await;
                                }));
                            }

                            dto::ConversationRelayEvent::Interrupt {
                                utterance_until_interrupt,
                                ..
                            } => {
                                tracing::info!(
                                    call_sid = %call_sid,
                                    utterance = ?utterance_until_interrupt,
                                    "voice interrupt: cancelling current LLM stream"
                                );

                                prompt_buffer.clear();
                                cancel_voice_turn(&mut cancel_tx, &mut llm_task).await;
                                while resp_rx.try_recv().is_ok() {}
                                state.voice.handle_interrupt(&call_sid).await;
                            }

                            dto::ConversationRelayEvent::Dtmf { digit, .. } => {
                                let digit_str = digit.as_deref().unwrap_or("");
                                if digit_str.is_empty() {
                                    continue;
                                }
                                tracing::info!(
                                    call_sid = %call_sid,
                                    digit = %digit_str,
                                    "voice dtmf received"
                                );
                                let voice = state.voice.clone();
                                let sid = call_sid.clone();
                                let resp = resp_tx.clone();
                                let d = digit_str.to_string();
                                cancel_voice_turn(&mut cancel_tx, &mut llm_task).await;
                                while resp_rx.try_recv().is_ok() {}
                                let (tx, rx) = tokio::sync::watch::channel(false);
                                cancel_tx = Some(tx);
                                llm_task = Some(tokio::spawn(async move {
                                    let prompt = format!("[DTMF] {d}");
                                    spawn_prompt_task(voice, sid, prompt, Some(rx), resp).await;
                                }));
                            }

                            dto::ConversationRelayEvent::Transcript { transcript, .. } => {
                                tracing::debug!(
                                    call_sid = %call_sid,
                                    transcript = ?transcript,
                                    "voice transcript"
                                );
                            }

                            dto::ConversationRelayEvent::Close { .. } => {
                                tracing::info!(call_sid = %call_sid, "voice ws close event");
                                break;
                            }
                        }
                    }

                    Message::Close(_) => {
                        tracing::info!(call_sid = %call_sid, "voice ws closed by client");
                        break;
                    }

                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        }
    }

    cancel_voice_turn(&mut cancel_tx, &mut llm_task).await;
    let _ = state
        .voice
        .handle_status_callback(&call_sid, "completed")
        .await;
    tracing::info!(call_sid = %call_sid, "voice ws session ended");
}

async fn cancel_voice_turn(
    cancel: &mut Option<tokio::sync::watch::Sender<bool>>,
    task: &mut Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(cancel) = cancel.take() {
        let _ = cancel.send(true);
    }
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
}

enum WsOutMsg {
    Text { token: String, last: bool },
}

async fn spawn_prompt_task(
    voice: std::sync::Arc<rua_voice::VoiceManager>,
    call_sid: String,
    prompt: String,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
    resp_tx: tokio::sync::mpsc::Sender<WsOutMsg>,
) {
    match voice
        .process_prompt_with_cancel(&call_sid, &prompt, cancel)
        .await
    {
        Ok(response_text) => {
            let _ = resp_tx
                .send(WsOutMsg::Text {
                    token: response_text,
                    last: false,
                })
                .await;
            let _ = resp_tx
                .send(WsOutMsg::Text {
                    token: String::new(),
                    last: true,
                })
                .await;
        }
        Err(e) => {
            tracing::error!(call_sid = %call_sid, error = %e, "failed to process voice prompt");
            let _ = resp_tx
                .send(WsOutMsg::Text {
                    token: "I'm sorry, I encountered an error. Please try again.".into(),
                    last: false,
                })
                .await;
            let _ = resp_tx
                .send(WsOutMsg::Text {
                    token: String::new(),
                    last: true,
                })
                .await;
        }
    }
}

pub async fn voice_status_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(params): Form<BTreeMap<String, String>>,
) -> Response {
    let config = match voice_channel_config(&state).await {
        Ok(config) => config,
        Err(error) => return error.into_response(),
    };
    if config["enabled"] != true {
        return StatusCode::NO_CONTENT.into_response();
    }
    if !signed_voice_webhook(&config, "/voice/status-callback", &headers, &params) {
        return ApiError::Unauthorized("invalid Twilio signature".into()).into_response();
    }
    let body: dto::TwilioStatusCallback = match serde_json::from_value(serde_json::json!(params)) {
        Ok(body) => body,
        Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
    };
    tracing::info!(
        call_sid = %body.call_sid,
        status = %body.call_status,
        "voice status callback"
    );

    if let Err(e) = state
        .voice
        .handle_status_callback(&body.call_sid, &body.call_status)
        .await
    {
        tracing::error!(error = %e, "failed to handle voice status callback");
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn voice_tts_synthesize(
    State(state): State<AppState>,
    Json(body): Json<TtsSynthesizeRequest>,
) -> Response {
    if body.text.is_empty() {
        return ApiError::BadRequest("text is required".into()).into_response();
    }

    match state
        .voice
        .synthesize_speech(
            &body.text,
            body.model.as_deref(),
            body.voice.as_deref(),
            body.speed,
        )
        .await
    {
        Ok(audio_bytes) => (
            StatusCode::OK,
            [("Content-Type", "audio/mpeg")],
            audio_bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "TTS synthesis failed");
            ApiError::Internal(format!("TTS synthesis failed: {e}")).into_response()
        }
    }
}

pub async fn voice_asr_transcribe(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.starts_with("audio/") {
        voice_asr_from_raw(state, req, &content_type).await
    } else {
        voice_asr_from_multipart(state, req).await
    }
}

async fn voice_asr_from_raw(
    state: AppState,
    req: axum::extract::Request,
    content_type: &str,
) -> Response {
    let bytes = match axum::body::Bytes::from_request(req, &()).await {
        Ok(b) => b,
        Err(e) => {
            return ApiError::BadRequest(format!("failed to read body: {e}")).into_response();
        }
    };
    if bytes.is_empty() {
        return ApiError::BadRequest("audio body is empty".into()).into_response();
    }
    let ext = content_type
        .split('/')
        .nth(1)
        .and_then(|s| s.split(';').next())
        .unwrap_or("wav");
    let filename = format!("audio.{ext}");
    match state
        .voice
        .transcribe_audio(bytes.to_vec(), &filename, None)
        .await
    {
        Ok(text) => {
            let body = serde_json::json!({ "text": text });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            if matches!(&e, rua_core::RuaError::NotImplemented { .. }) {
                let body = serde_json::json!({
                    "text": "",
                    "supported": false,
                    "detail": "audio transcription is not configured"
                });
                return (StatusCode::OK, Json(body)).into_response();
            }
            tracing::error!(error = %e, "ASR transcription failed");
            ApiError::Internal(format!("ASR transcription failed: {e}")).into_response()
        }
    }
}

async fn voice_asr_from_multipart(state: AppState, req: axum::extract::Request) -> Response {
    let mut multipart = match Multipart::from_request(req, &()).await {
        Ok(m) => m,
        Err(e) => {
            return ApiError::BadRequest(format!("invalid multipart request: {e}")).into_response();
        }
    };

    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut filename = "audio.wav".to_string();
    let mut model: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(fname) = field.file_name() {
                    filename = fname.to_string();
                }
                if let Err(resp) = validate_asr_filename(&filename) {
                    return *resp;
                }
                match field.bytes().await {
                    Ok(data) => audio_bytes = Some(data.to_vec()),
                    Err(e) => {
                        return ApiError::BadRequest(format!("failed to read file: {e}"))
                            .into_response();
                    }
                }
            }
            "model" => {
                if let Ok(text) = field.text().await {
                    model = Some(text);
                }
            }
            _ => {}
        }
    }

    let audio = match audio_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return ApiError::BadRequest("file field is required".into()).into_response();
        }
    };

    match state
        .voice
        .transcribe_audio(audio, &filename, model.as_deref())
        .await
    {
        Ok(text) => {
            let body = serde_json::json!({ "text": text });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            if matches!(&e, rua_core::RuaError::NotImplemented { .. }) {
                let body = serde_json::json!({
                    "text": "",
                    "supported": false,
                    "detail": "audio transcription is not configured"
                });
                return (StatusCode::OK, Json(body)).into_response();
            }
            tracing::error!(error = %e, "ASR transcription failed");
            ApiError::Internal(format!("ASR transcription failed: {e}")).into_response()
        }
    }
}

pub async fn voice_tts_config(State(state): State<AppState>) -> Response {
    let cfg = state.voice.get_tts_config();
    let body = serde_json::json!({
        "tts_provider_id": cfg.provider_id,
        "tts_model": cfg.model,
        "tts_voice": cfg.voice,
        "tts_speed": cfg.speed,
    });
    (StatusCode::OK, Json(body)).into_response()
}

pub async fn voice_asr_config(State(state): State<AppState>) -> Response {
    let cfg = state.voice.get_asr_config();
    let supported = cfg.provider_type != "disabled" && !cfg.provider_id.is_empty();
    let body = serde_json::json!({
        "transcription_provider_type": cfg.provider_type,
        "transcription_provider_id": cfg.provider_id,
        "transcription_model": cfg.model,
        "supported": supported,
    });
    (StatusCode::OK, Json(body)).into_response()
}
