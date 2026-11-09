use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use rua_session::{MessageLogEntry, WorkspaceSessionStore};
use rua_usage::{TokenUsageFilter, TokenUsageQuery};

use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::services::chat_index;
use crate::state::AppState;
use crate::sync_publication::{
    publish_agent_config_committed, publish_sync_fact, SyncPublicationFact,
};

pub fn agent_config_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agent/running-config",
            get(get_running_config)
                .put(put_running_config)
                .delete(reset_running_config),
        )
        .route(
            "/agent/language",
            get(get_agent_language).put(put_agent_language),
        )
        .route("/agent/audio-mode", get(get_audio_mode).put(put_audio_mode))
        .route("/agent/tts-config", get(get_tts_config).put(put_tts_config))
        .route("/agent/tts-providers", get(get_tts_providers))
        .route("/agent/tts/synthesize", post(post_tts_synthesize))
        .route(
            "/agent/transcription-provider-type",
            get(get_transcription_provider_type).put(put_transcription_provider_type),
        )
        .route("/agent/local-whisper-status", get(get_local_whisper_status))
        .route(
            "/agent/transcription-providers",
            get(get_transcription_providers),
        )
        .route(
            "/agent/transcription-provider",
            put(put_transcription_provider),
        )
        .route(
            "/agents/{agentId}/running-config",
            get(get_running_config_scoped)
                .put(put_running_config_scoped)
                .delete(reset_running_config_scoped),
        )
        .route(
            "/agents/{agentId}/running-config/chat-defaults",
            get(get_chat_running_config_defaults_scoped),
        )
        .route(
            "/agents/{agentId}/language",
            get(get_agent_language_scoped).put(put_agent_language_scoped),
        )
        .route(
            "/agents/{agentId}/audio-mode",
            get(get_audio_mode_scoped).put(put_audio_mode_scoped),
        )
        .route(
            "/agents/{agentId}/tts-config",
            get(get_tts_config_scoped).put(put_tts_config_scoped),
        )
        .route(
            "/agents/{agentId}/tts-providers",
            get(get_tts_providers_scoped),
        )
        .route(
            "/agents/{agentId}/tts/synthesize",
            post(post_tts_synthesize_scoped),
        )
        .route(
            "/agents/{agentId}/transcription-provider-type",
            get(get_transcription_provider_type_scoped).put(put_transcription_provider_type_scoped),
        )
        .route(
            "/agents/{agentId}/local-whisper-status",
            get(get_local_whisper_status_scoped),
        )
        .route(
            "/agents/{agentId}/transcription-providers",
            get(get_transcription_providers_scoped),
        )
        .route(
            "/agents/{agentId}/transcription-provider",
            put(put_transcription_provider_scoped),
        )
}

fn resolve_data_dir(state: &AppState) -> PathBuf {
    state.rua_root.clone()
}

async fn get_running_config(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    let agent_id = ctx.agent_id;
    match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(detail)) => {
            let running = detail.running.unwrap_or_else(|| {
                serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
            });
            crate::services::history_replacement::reconcile_agent_config(&state, &agent_id).await;
            (StatusCode::OK, Json(running)).into_response()
        }
        Ok(None) => {
            let running = serde_json::to_value(rua_config::AgentsRunningConfig::default())
                .unwrap_or_default();
            (StatusCode::OK, Json(running)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_agent_language(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    let agent_id = ctx.agent_id;
    match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(detail)) => {
            let lang = if detail.language.is_empty() {
                "zh"
            } else {
                &detail.language
            };
            let body = serde_json::json!({
                "language": lang,
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(None) => {
            let body = serde_json::json!({
                "language": "zh",
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_audio_mode(State(state): State<AppState>) -> Response {
    let mode = state.voice.get_audio_mode();
    let body = serde_json::json!({
        "audio_mode": mode,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_tts_config(State(state): State<AppState>) -> Response {
    let cfg = state.voice.get_tts_config();
    let body = serde_json::json!({
        "tts_provider_type": cfg.provider_type,
        "tts_provider_id": cfg.provider_id,
        "tts_model": cfg.model,
        "tts_voice": cfg.voice,
        "tts_speed": cfg.speed,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_tts_providers() -> Response {
    let body = serde_json::json!({
        "providers": [],
        "configured_provider_id": "",
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_transcription_provider_type(State(state): State<AppState>) -> Response {
    let cfg = state.voice.get_asr_config();
    let body = serde_json::json!({
        "transcription_provider_type": cfg.provider_type,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_local_whisper_status() -> Response {
    let body = serde_json::json!({
        "available": false,
        "ffmpeg_installed": false,
        "whisper_installed": false,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_transcription_providers(State(state): State<AppState>) -> Response {
    let cfg = state.voice.get_asr_config();
    let providers = match list_voice_capable_providers(&state).await {
        Ok(providers) => providers,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let body = serde_json::json!({
        "providers": providers,
        "configured_provider_id": cfg.provider_id,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn list_voice_capable_providers(
    state: &AppState,
) -> Result<Vec<serde_json::Value>, llm_multiplex::LlmError> {
    let mut providers: Vec<_> = state
        .provider_cells
        .list()
        .await?
        .into_iter()
        .filter(|provider| {
            !provider.base_url.trim().is_empty()
                && (!provider.require_api_key || !provider.api_key.trim().is_empty())
                && is_voice_capable_provider(provider)
        })
        .map(|provider| {
            let id = provider.id;
            serde_json::json!({
                "id": id.clone(),
                "name": if provider.name.is_empty() {
                    id.clone()
                } else {
                    provider.name
                },
                "available": true,
                "type": voice_provider_type(&id, provider.chat_model.as_deref()),
            })
        })
        .collect();
    providers.sort_by(|a, b| {
        let left = a.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        let right = b.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        left.cmp(right)
    });
    Ok(providers)
}

fn voice_provider_type(provider_id: &str, chat_model: Option<&str>) -> &'static str {
    let model = chat_model.unwrap_or_default().to_ascii_lowercase();
    if provider_id.eq_ignore_ascii_case("dashscope") || model.contains("dashscope") {
        "dashscope"
    } else {
        "openai"
    }
}

fn is_voice_capable_provider(provider: &rua_provider::config_loader::ProviderConfig) -> bool {
    if voice_provider_type(&provider.id, provider.chat_model.as_deref()) == "dashscope" {
        return true;
    }
    provider.all_models().into_iter().any(|model| {
        let id = model.id.to_ascii_lowercase();
        let name = model.name.to_ascii_lowercase();
        id.contains("whisper")
            || name.contains("whisper")
            || id.contains("audio")
            || name.contains("audio")
    })
}

async fn put_running_config(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let agent_id = ctx.agent_id;
    let update = super::dto::AgentUpdate {
        running: Some(body.clone()),
        name: None,
        description: None,
        enabled: None,
        language: None,
        system_prompt_files: None,
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        llm_routing: None,
        tools: None,
        plan: None,
        security: None,
        skill_names: None,
        extra: serde_json::Map::new(),
    };
    match state.agent_service.update_agent(&agent_id, update).await {
        Ok(detail) => {
            let agent_val = serde_json::to_value(&detail).unwrap_or_default();
            publish_agent_config_committed(&state, &agent_id, agent_val).await;
            let running = detail.running.unwrap_or_else(|| {
                serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
            });
            if let Some(runtime) = state.registry.get(&agent_id).await {
                if runtime.apply_embedding_configuration().await.is_err() {
                    return ApiError::Internal(
                        "Running configuration saved, but Embedding resource application failed"
                            .to_string(),
                    )
                    .into_response();
                }
            }
            (StatusCode::OK, Json(running)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn reset_running_config(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    put_running_config(
        State(state),
        Extension(ctx),
        Json(serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()),
    )
    .await
}

async fn put_agent_language(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let language = body
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();

    let valid = ["zh", "en", "ru"];
    if !valid.contains(&language.as_str()) {
        return ApiError::BadRequest(format!(
            "Invalid language '{}'. Must be one of: {}",
            language,
            valid.join(", ")
        ))
        .into_response();
    }

    let agent_id = ctx.agent_id;
    let update = super::dto::AgentUpdate {
        language: Some(language.clone()),
        name: None,
        description: None,
        enabled: None,
        system_prompt_files: None,
        running: None,
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        llm_routing: None,
        tools: None,
        plan: None,
        security: None,
        skill_names: None,
        extra: serde_json::Map::new(),
    };
    match state.agent_service.update_agent(&agent_id, update).await {
        Ok(detail) => {
            let agent_val = serde_json::to_value(&detail).unwrap_or_default();
            publish_agent_config_committed(&state, &agent_id, agent_val).await;
            let resp = serde_json::json!({
                "language": language,
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

fn read_root_config(state: &AppState) -> Result<serde_json::Value, ApiError> {
    let config_path = resolve_data_dir(state).join("config.json");
    match std::fs::read_to_string(&config_path) {
        Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to read config.json: {e}"
        ))),
    }
}

fn write_root_config(state: &AppState, raw: &serde_json::Value) -> Result<(), ApiError> {
    let config_path = resolve_data_dir(state).join("config.json");
    rua_config::atomic_write_json(&config_path, raw)
        .map_err(|e| ApiError::Internal(format!("Failed to write config.json: {e}")))
}

async fn publish_root_agents_after_write(state: &AppState, raw: &serde_json::Value) {
    let Some(agents) = raw.get("agents").cloned() else {
        return;
    };
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::RootAgentRegistryCommitted { agents },
    )
    .await;
}

fn agents_extra_mut(
    raw: &mut serde_json::Value,
) -> &mut serde_json::Map<String, serde_json::Value> {
    let root = raw.as_object_mut().expect("root is object");
    let agents = root
        .entry("agents")
        .or_insert_with(|| serde_json::json!({}));
    if !agents.is_object() {
        *agents = serde_json::json!({});
    }
    agents.as_object_mut().unwrap()
}

async fn put_audio_mode(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mode = body
        .get("audio_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string();

    let mut raw = match read_root_config(&state) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    agents_extra_mut(&mut raw).insert("audio_mode".into(), serde_json::json!(&mode));
    if let Err(e) = write_root_config(&state, &raw) {
        return e.into_response();
    }
    publish_root_agents_after_write(&state, &raw).await;

    state.voice.set_audio_mode(mode.clone());

    let resp = serde_json::json!({ "audio_mode": mode });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn put_tts_config(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let existing = state.voice.get_tts_config();

    let provider_type = body
        .get("tts_provider_type")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.provider_type)
        .to_string();
    let provider_id = body
        .get("tts_provider_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.provider_id)
        .to_string();
    let model = body
        .get("tts_model")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.model)
        .to_string();
    let voice = body
        .get("tts_voice")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.voice)
        .to_string();
    let speed = body
        .get("tts_speed")
        .and_then(|v| v.as_f64())
        .unwrap_or(existing.speed);

    let mut raw = match read_root_config(&state) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let agents = agents_extra_mut(&mut raw);
    agents.insert(
        "tts_provider_type".into(),
        serde_json::json!(&provider_type),
    );
    agents.insert("tts_provider_id".into(), serde_json::json!(&provider_id));
    agents.insert("tts_model".into(), serde_json::json!(&model));
    agents.insert("tts_voice".into(), serde_json::json!(&voice));
    agents.insert("tts_speed".into(), serde_json::json!(speed));
    if let Err(e) = write_root_config(&state, &raw) {
        return e.into_response();
    }
    publish_root_agents_after_write(&state, &raw).await;

    state.voice.store_tts_config(rua_voice::TtsConfig {
        provider_type: provider_type.clone(),
        provider_id: provider_id.clone(),
        model: model.clone(),
        voice: voice.clone(),
        speed,
        language: existing.language,
    });

    let resp = serde_json::json!({
        "tts_provider_type": provider_type,
        "tts_provider_id": provider_id,
        "tts_model": model,
        "tts_voice": voice,
        "tts_speed": speed,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn post_tts_synthesize(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let text = match body.get("text").and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t,
        _ => return ApiError::BadRequest("text is required".into()).into_response(),
    };

    match state.voice.synthesize_tts(text).await {
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

async fn put_transcription_provider_type(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let provider_type = body
        .get("transcription_provider_type")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled")
        .to_string();
    if !is_valid_transcription_provider_type(&provider_type) {
        return ApiError::BadRequest(format!(
            "Invalid transcription_provider_type '{}'. Must be one of: disabled, whisper_api, local_whisper",
            provider_type
        ))
        .into_response();
    }

    let mut raw = match read_root_config(&state) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    agents_extra_mut(&mut raw).insert(
        "transcription_provider_type".into(),
        serde_json::json!(&provider_type),
    );
    if let Err(e) = write_root_config(&state, &raw) {
        return e.into_response();
    }
    publish_root_agents_after_write(&state, &raw).await;

    let existing = state.voice.get_asr_config();
    state.voice.store_asr_config(rua_voice::AsrConfig {
        provider_type: provider_type.clone(),
        provider_id: existing.provider_id,
        model: existing.model,
        language: existing.language,
    });

    let resp = serde_json::json!({ "transcription_provider_type": provider_type });
    (StatusCode::OK, Json(resp)).into_response()
}

fn is_valid_transcription_provider_type(provider_type: &str) -> bool {
    matches!(provider_type, "disabled" | "whisper_api" | "local_whisper")
}

async fn put_transcription_provider(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let provider_id = body
        .get("provider_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut raw = match read_root_config(&state) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    agents_extra_mut(&mut raw).insert(
        "transcription_provider_id".into(),
        serde_json::json!(&provider_id),
    );
    if let Err(e) = write_root_config(&state, &raw) {
        return e.into_response();
    }
    publish_root_agents_after_write(&state, &raw).await;

    let existing = state.voice.get_asr_config();
    state.voice.store_asr_config(rua_voice::AsrConfig {
        provider_type: existing.provider_type,
        provider_id: provider_id.clone(),
        model: existing.model,
        language: existing.language,
    });

    let resp = serde_json::json!({ "provider_id": provider_id });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn get_running_config_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(detail)) => {
            let running = detail.running.unwrap_or_else(|| {
                serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
            });
            (StatusCode::OK, Json(running)).into_response()
        }
        Ok(None) => {
            let running = serde_json::to_value(rua_config::AgentsRunningConfig::default())
                .unwrap_or_default();
            (StatusCode::OK, Json(running)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
async fn put_running_config_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let update = super::dto::AgentUpdate {
        running: Some(body.clone()),
        name: None,
        description: None,
        enabled: None,
        language: None,
        system_prompt_files: None,
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        llm_routing: None,
        tools: None,
        plan: None,
        security: None,
        skill_names: None,
        extra: serde_json::Map::new(),
    };
    match state.agent_service.update_agent(&agent_id, update).await {
        Ok(detail) => {
            let agent_val = serde_json::to_value(&detail).unwrap_or_default();
            publish_agent_config_committed(&state, &agent_id, agent_val).await;
            let running = detail.running.unwrap_or_else(|| {
                serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
            });
            (StatusCode::OK, Json(running)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn reset_running_config_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    put_running_config_scoped(
        State(state),
        Path(agent_id),
        Json(serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()),
    )
    .await
}

async fn get_chat_running_config_defaults_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let running = match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(detail)) => match detail.running {
            Some(running) => {
                match serde_json::from_value::<rua_config::AgentsRunningConfig>(running) {
                    Ok(running) => running,
                    Err(error) => {
                        return ApiError::Internal(format!(
                            "failed to decode Agent running config: {error}"
                        ))
                        .into_response();
                    }
                }
            }
            None => rua_config::AgentsRunningConfig::default(),
        },
        Ok(None) => rua_config::AgentsRunningConfig::default(),
        Err(error) => return ApiError::from(error).into_response(),
    };
    (
        StatusCode::OK,
        Json(rua_config::ChatRunningConfig::from_agent_defaults(&running)),
    )
        .into_response()
}
async fn get_agent_language_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(detail)) => {
            let lang = if detail.language.is_empty() {
                "zh"
            } else {
                &detail.language
            };
            let body = serde_json::json!({
                "language": lang,
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(None) => {
            let body = serde_json::json!({
                "language": "zh",
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
async fn put_agent_language_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let language = body
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();

    let valid = ["zh", "en", "ru"];
    if !valid.contains(&language.as_str()) {
        return ApiError::BadRequest(format!(
            "Invalid language '{}'. Must be one of: {}",
            language,
            valid.join(", ")
        ))
        .into_response();
    }

    let update = super::dto::AgentUpdate {
        language: Some(language.clone()),
        name: None,
        description: None,
        enabled: None,
        system_prompt_files: None,
        running: None,
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        llm_routing: None,
        tools: None,
        plan: None,
        security: None,
        skill_names: None,
        extra: serde_json::Map::new(),
    };
    match state.agent_service.update_agent(&agent_id, update).await {
        Ok(detail) => {
            let agent_val = serde_json::to_value(&detail).unwrap_or_default();
            publish_agent_config_committed(&state, &agent_id, agent_val).await;
            let resp = serde_json::json!({
                "language": language,
                "agent_id": agent_id,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
async fn get_audio_mode_scoped(State(state): State<AppState>, Path(_): Path<String>) -> Response {
    get_audio_mode(State(state)).await
}
async fn put_audio_mode_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    put_audio_mode(State(state), Json(body)).await
}
async fn get_tts_config_scoped(State(state): State<AppState>, Path(_): Path<String>) -> Response {
    get_tts_config(State(state)).await
}
async fn put_tts_config_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    put_tts_config(State(state), Json(body)).await
}
async fn get_tts_providers_scoped(Path(_): Path<String>) -> Response {
    get_tts_providers().await
}
async fn post_tts_synthesize_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    post_tts_synthesize(State(state), Json(body)).await
}
async fn get_transcription_provider_type_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
) -> Response {
    get_transcription_provider_type(State(state)).await
}
async fn put_transcription_provider_type_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    put_transcription_provider_type(State(state), Json(body)).await
}
async fn get_local_whisper_status_scoped(Path(_): Path<String>) -> Response {
    get_local_whisper_status().await
}
async fn get_transcription_providers_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
) -> Response {
    get_transcription_providers(State(state)).await
}
async fn put_transcription_provider_scoped(
    State(state): State<AppState>,
    Path(_): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    put_transcription_provider(State(state), Json(body)).await
}

pub fn console_debug_router() -> Router<AppState> {
    Router::new()
        .route("/console/debug/backend-logs", get(get_backend_logs))
        .route(
            "/agents/{agentId}/console/debug/backend-logs",
            get(get_backend_logs_scoped),
        )
}

pub fn agent_stats_router() -> Router<AppState> {
    Router::new().route("/agent-stats", get(get_agent_stats))
}

#[derive(serde::Deserialize)]
struct AgentStatsQuery {
    start_date: Option<String>,
    end_date: Option<String>,
}

async fn get_agent_stats(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Query(query): Query<AgentStatsQuery>,
) -> Response {
    let today = Utc::now().date_naive();
    let mut end_date = parse_iso_date(query.end_date.as_deref()).unwrap_or(today);
    let mut start_date =
        parse_iso_date(query.start_date.as_deref()).unwrap_or(end_date - ChronoDuration::days(30));
    if start_date > end_date {
        std::mem::swap(&mut start_date, &mut end_date);
    }

    let runtime = match state.registry.get_or_create(&ctx.agent_id).await {
        Ok(runtime) => runtime,
        Err(e) => {
            return ApiError::Internal(format!("failed to obtain workspace runtime: {e}"))
                .into_response();
        }
    };
    let usage_root_dir = runtime.rua_root();
    let workspace_dir = runtime.workspace_dir();

    let token_summary = {
        let usage_query = TokenUsageQuery::new(usage_root_dir);
        let filter = TokenUsageFilter {
            start_date: Some(start_date.to_string()),
            end_date: Some(end_date.to_string()),
            agent_id: Some(ctx.agent_id.clone()),
            ..Default::default()
        };
        usage_query.query(&filter).unwrap_or_default()
    };

    let conversation_stats =
        collect_agent_conversation_stats(&workspace_dir, start_date, end_date).await;

    let mut by_date = Vec::new();
    let mut current = start_date;
    while current <= end_date {
        let date_key = current.to_string();
        let usage = token_summary
            .by_date
            .get(&date_key)
            .cloned()
            .unwrap_or_default();
        let conversation = conversation_stats
            .by_date
            .get(&date_key)
            .cloned()
            .unwrap_or_default();
        by_date.push(serde_json::json!({
            "date": date_key,
            "chats": conversation.chats,
            "active_sessions": conversation.active_sessions,
            "user_messages": conversation.user_messages,
            "assistant_messages": conversation.assistant_messages,
            "total_messages": conversation.total_messages,
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "llm_calls": usage.call_count,
            "tool_calls": conversation.tool_calls,
        }));
        current += ChronoDuration::days(1);
    }

    let channel_stats = conversation_stats
        .by_channel
        .iter()
        .map(|(channel, stats)| {
            serde_json::json!({
                "channel": channel,
                "chats": stats.chats,
                "active_sessions": stats.active_sessions,
                "user_messages": stats.user_messages,
                "assistant_messages": stats.assistant_messages,
                "total_messages": stats.total_messages,
                "tool_calls": stats.tool_calls,
            })
        })
        .collect::<Vec<_>>();

    let body = serde_json::json!({
        "total_active_sessions": conversation_stats.total.active_sessions,
        "total_messages": conversation_stats.total.total_messages,
        "total_user_messages": conversation_stats.total.user_messages,
        "total_assistant_messages": conversation_stats.total.assistant_messages,
        "total_prompt_tokens": token_summary.total_prompt_tokens,
        "total_completion_tokens": token_summary.total_completion_tokens,
        "total_llm_calls": token_summary.total_calls,
        "total_tool_calls": conversation_stats.total.tool_calls,
        "by_date": by_date,
        "channel_stats": channel_stats,
        "start_date": start_date.to_string(),
        "end_date": end_date.to_string(),
    });
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Debug, Clone, Default)]
struct ConversationStatsBucket {
    chats: u64,
    active_sessions: u64,
    user_messages: u64,
    assistant_messages: u64,
    total_messages: u64,
    tool_calls: u64,
}

#[derive(Debug, Clone, Default)]
struct AgentConversationStats {
    total: ConversationStatsBucket,
    by_date: BTreeMap<String, ConversationStatsBucket>,
    by_channel: BTreeMap<String, ConversationStatsBucket>,
}

async fn collect_agent_conversation_stats(
    workspace_dir: &std::path::Path,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> AgentConversationStats {
    let mut stats = AgentConversationStats::default();
    let chats = match chat_index::list_all(workspace_dir).await {
        Ok(chats) => chats,
        Err(e) => {
            tracing::warn!(error = %e, path = %workspace_dir.display(), "failed to load chat index for agent stats");
            return stats;
        }
    };
    let store = WorkspaceSessionStore::new(workspace_dir);
    let mut active_seen = BTreeSet::new();
    let mut chat_seen = BTreeSet::new();
    let mut active_seen_by_channel: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for chat in chats {
        let chat_date = chat.updated_at.date_naive();
        let chat_in_range = date_in_range(chat_date, start_date, end_date);
        if chat_in_range {
            chat_seen.insert(chat.id.clone());
            add_chat_stat(&mut stats.total, &mut active_seen, &chat);
            let channel_active_seen = active_seen_by_channel
                .entry(chat.channel.clone())
                .or_default();
            add_chat_stat(
                stats.by_channel.entry(chat.channel.clone()).or_default(),
                channel_active_seen,
                &chat,
            );
            let bucket = stats.by_date.entry(chat_date.to_string()).or_default();
            bucket.chats += 1;
            if chat_is_active(&chat.status) {
                bucket.active_sessions += 1;
            }
        }

        let messages = match store.read_message_range(&chat.id, 1, usize::MAX).await {
            Ok(messages) => messages,
            Err(e) => {
                tracing::warn!(chat_id = %chat.id, error = %e, "failed to read chat messages for agent stats");
                continue;
            }
        };
        for message in messages {
            let Some(date) = parse_message_date(&message) else {
                continue;
            };
            if !date_in_range(date, start_date, end_date) {
                continue;
            }
            add_message_stat(&mut stats.total, &message);
            add_message_stat(stats.by_date.entry(date.to_string()).or_default(), &message);
            add_message_stat(
                stats.by_channel.entry(chat.channel.clone()).or_default(),
                &message,
            );
        }
    }

    stats.total.chats = chat_seen.len() as u64;
    stats.total.active_sessions = active_seen.len() as u64;
    stats
}

fn add_chat_stat(
    bucket: &mut ConversationStatsBucket,
    active_seen: &mut BTreeSet<String>,
    chat: &chat_index::ChatIndexEntry,
) {
    bucket.chats += 1;
    if chat_is_active(&chat.status) && active_seen.insert(chat.id.clone()) {
        bucket.active_sessions += 1;
    }
}

fn add_message_stat(bucket: &mut ConversationStatsBucket, message: &MessageLogEntry) {
    match message.role.as_str() {
        "user" => bucket.user_messages += 1,
        "assistant" => bucket.assistant_messages += 1,
        _ => {}
    }
    bucket.total_messages += 1;
    bucket.tool_calls += count_tool_calls(message);
}

fn parse_message_date(message: &MessageLogEntry) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(&message.created_at)
        .ok()
        .map(|value| value.date_naive())
        .or_else(|| parse_iso_date(Some(message.created_at.get(..10)?)))
}

fn date_in_range(date: NaiveDate, start_date: NaiveDate, end_date: NaiveDate) -> bool {
    date >= start_date && date <= end_date
}

fn chat_is_active(status: &str) -> bool {
    !matches!(status, "archived" | "deleted")
}

fn count_tool_calls(message: &MessageLogEntry) -> u64 {
    fn count_value(value: &serde_json::Value) -> u64 {
        match value {
            serde_json::Value::Array(items) => items.iter().map(count_value).sum(),
            serde_json::Value::Object(obj) => {
                let own = obj
                    .get("type")
                    .and_then(|value| value.as_str())
                    .is_some_and(|kind| matches!(kind, "tool_use" | "tool_call"))
                    || obj.contains_key("tool_call_id");
                (if own { 1 } else { 0 }) + obj.values().map(count_value).sum::<u64>()
            }
            _ => 0,
        }
    }
    count_value(&message.content)
}

fn parse_iso_date(value: Option<&str>) -> Option<NaiveDate> {
    value.and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

#[derive(serde::Deserialize)]
struct BackendLogsQuery {
    #[serde(default = "default_lines")]
    lines: usize,
}

fn default_lines() -> usize {
    200
}

async fn get_backend_logs(
    State(state): State<AppState>,
    Query(query): Query<BackendLogsQuery>,
) -> Response {
    let data_dir = resolve_data_dir(&state);
    let log_path = data_dir.join("logs").join("rua.log");
    let lines = query.lines.clamp(20, 5000);
    backend_logs_response(&log_path, lines)
}

async fn get_backend_logs_scoped(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    Query(query): Query<BackendLogsQuery>,
) -> Response {
    get_backend_logs(State(state), Query(query)).await
}

fn backend_logs_response(log_path: &std::path::Path, lines: usize) -> Response {
    match std::fs::metadata(log_path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            let content = tail_text_file(log_path, lines, 512 * 1024);
            let body = serde_json::json!({
                "path": log_path.display().to_string(),
                "exists": true,
                "lines": lines,
                "updated_at": mtime,
                "size": meta.len(),
                "content": content,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let body = serde_json::json!({
                "path": log_path.display().to_string(),
                "exists": false,
                "lines": lines,
                "updated_at": null,
                "size": 0,
                "content": "",
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => ApiError::Internal(format!("Failed to read log file: {e}")).into_response(),
    }
}

fn tail_text_file(path: &std::path::Path, lines: usize, max_bytes: usize) -> String {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len() as usize,
        Err(_) => return String::new(),
    };
    if size == 0 {
        return String::new();
    }
    let data = if size <= max_bytes {
        match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return String::new(),
        }
    } else {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return String::new(),
        };
        let _ = f.seek(SeekFrom::Start((size - max_bytes) as u64));
        let mut buf = vec![0u8; max_bytes];
        let n = f.read(&mut buf).unwrap_or(0);
        buf.truncate(n);
        buf
    };
    let text = String::from_utf8_lossy(&data);
    let all_lines: Vec<&str> = text.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    all_lines[start..].join("\n")
}
