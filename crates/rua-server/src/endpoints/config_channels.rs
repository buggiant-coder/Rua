use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rua_channel::core::{ChannelHealth, ChannelStatus};
use rua_config::{merge_with_redaction_safety, redact_secrets};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::endpoints::dto::config::{
    backfill_builtin_defaults, channel_types as builtin_channel_types, inject_is_builtin,
    is_builtin_channel_key,
};
use crate::errors::ApiError;
use crate::errors::ValidationError;
use crate::middleware::AgentExtractorContext;
use crate::qrcode_auth::QRCodeAuthParams;
use crate::state::AppState;

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route(
            "/config/channels",
            get(list_channels).put(update_all_channels),
        )
        .route("/config/channels/types", get(get_channel_types))
        .route("/config/channels/schemas", get(get_channel_schemas))
        .route(
            "/config/channels/{channel_name}",
            get(get_channel).put(update_channel),
        )
        .route(
            "/config/channels/{channel_name}/health",
            get(get_channel_health),
        )
        .route(
            "/config/channels/{channel_name}/restart",
            post(restart_channel),
        )
        .route("/config/channels/{channel}/qrcode", get(get_qrcode))
        .route(
            "/config/channels/{channel}/qrcode/status",
            get(get_qrcode_status),
        )
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/config/channels",
            get(list_channels_agent).put(update_all_channels_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/types",
            get(get_channel_types_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/schemas",
            get(get_channel_schemas_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/{channel_name}",
            get(get_channel_agent).put(update_channel_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/{channel_name}/health",
            get(get_channel_health_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/{channel_name}/restart",
            post(restart_channel_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/{channel}/qrcode",
            get(get_qrcode_agent),
        )
        .route(
            "/agents/{agentId}/config/channels/{channel}/qrcode/status",
            get(get_qrcode_status_agent),
        )
}

#[derive(Debug, serde::Deserialize)]
pub struct QrCodeStatusQuery {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct QrCodeQuery {
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ChannelHealthResponse {
    channel: String,
    status: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    components: Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    checks: Vec<ChannelHealthCheck>,
}

#[derive(Debug, serde::Serialize)]
struct ChannelHealthCheck {
    name: String,
    status: &'static str,
    detail: String,
}

#[derive(Debug, serde::Serialize)]
struct ChannelSchemasResponse {
    scope: &'static str,
    plugin_channels: &'static str,
    order: Vec<String>,
    channels: Map<String, Value>,
}

#[derive(Debug, serde::Serialize)]
struct ChannelRestartResponse {
    channel: String,
    status: &'static str,
    detail: String,
}

fn resolve_agent_id(agent_id: Option<&str>) -> &str {
    agent_id.unwrap_or("default")
}

fn channel_enabled(map: &Map<String, Value>, channel_name: &str) -> bool {
    map.get(channel_name)
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn channel_available(map: &Map<String, Value>, channel_name: &str) -> bool {
    is_builtin_channel_key(channel_name) || map.contains_key(channel_name)
}

fn channel_schemas_response() -> ChannelSchemasResponse {
    let mut channels = Map::new();
    for channel in builtin_channel_types() {
        channels.insert(channel.clone(), channel_schema(&channel));
    }
    ChannelSchemasResponse {
        scope: "builtin",
        plugin_channels: "out_of_scope",
        order: builtin_channel_types(),
        channels,
    }
}

fn channel_schema(channel: &str) -> Value {
    let required = token_required_fields(channel);
    serde_json::json!({
        "id": channel,
        "is_builtin": true,
        "label": channel_label(channel),
        "icon": channel_icon(channel),
        "docs": channel_docs(channel),
        "defaults": channel_defaults(channel),
        "required_fields": required,
        "fields": channel_fields(channel, required),
    })
}

fn channel_label(channel: &str) -> Value {
    let (en, zh) = match channel {
        "console" => ("Console", "控制台"),
        "dingtalk" => ("DingTalk", "钉钉"),
        "discord" => ("Discord", "Discord"),
        "feishu" => ("Feishu/Lark", "飞书/Lark"),
        "imessage" => ("iMessage", "iMessage"),
        "matrix" => ("Matrix", "Matrix"),
        "mattermost" => ("Mattermost", "Mattermost"),
        "mqtt" => ("MQTT", "MQTT"),
        "onebot" => ("OneBot", "OneBot"),
        "qq" => ("QQ", "QQ"),
        "sip" => ("SIP", "SIP"),
        "slack" => ("Slack", "Slack"),
        "telegram" => ("Telegram", "Telegram"),
        "voice" => ("Voice", "语音"),
        "wecom" => ("WeCom", "企业微信"),
        "wechat" => ("WeChat", "微信"),
        "xiaoyi" => ("XiaoYi", "小艺"),
        "yuanbao" => ("Yuanbao", "元宝"),
        _ => (channel, channel),
    };
    serde_json::json!({ "en": en, "zh": zh })
}

fn channel_icon(channel: &str) -> String {
    format!("channel-{channel}")
}

fn channel_docs(channel: &str) -> Value {
    let anchor = match channel {
        "dingtalk" => ("DingTalk-recommended", "钉钉推荐"),
        "feishu" => ("Feishu-Lark", "飞书"),
        "imessage" => ("iMessage-macOS-only", "iMessage仅-macOS"),
        "wecom" => ("WeCom-WeChat-Work", "企业微信"),
        "wechat" => ("WeChat-Personal-iLink", "微信个人iLink"),
        "xiaoyi" | "yuanbao" => {
            return serde_json::json!({
                "en": "https://developer.huawei.com/consumer/cn/doc/service/openclaw-0000002518410344",
                "zh": "https://developer.huawei.com/consumer/cn/doc/service/openclaw-0000002518410344",
            });
        }
        other => (other, other),
    };
    serde_json::json!({
        "en": format!("https://rua.agentscope.io/docs/channels/?lang=en#{}", anchor.0),
        "zh": format!("https://rua.agentscope.io/docs/channels/?lang=zh#{}", anchor.1),
    })
}

fn channel_defaults(channel: &str) -> Value {
    rua_config::channel_catalog::channel_defaults(channel)
}

fn channel_fields(channel: &str, required: &[&str]) -> Vec<Value> {
    rua_config::channel_catalog::channel_fields(channel)
        .into_iter()
        .filter(|field| field["hidden"] != true)
        .map(|mut field| {
            let name = field["name"].as_str().unwrap_or_default().to_string();
            field["label"] = serde_json::json!({
                "en": field_label(&name),
                "zh": field_label(&name),
            });
            field["required"] = Value::Bool(required.contains(&name.as_str()));
            field["options"] = Value::Array(
                field["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|value| {
                        let label = value
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| value.to_string());
                        serde_json::json!({ "label": label, "value": label })
                    })
                    .collect(),
            );
            field
        })
        .collect()
}

fn field_label(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn health_payload(
    channel_name: &str,
    state: &ChannelStatus,
    config: Option<&Value>,
) -> ChannelHealthResponse {
    let mut components = base_health_components(channel_name, config, state.is_running());
    components.insert(
        "runtime".into(),
        Value::String(state.runtime_label().into()),
    );
    components.insert("transport".into(), Value::String("unknown".into()));
    let (status, detail) = match state.health() {
        ChannelHealth::Healthy => (
            "healthy",
            "Channel runtime has started; transport connectivity is not observed.".into(),
        ),
        ChannelHealth::Degraded { reason } => ("degraded", reason),
        ChannelHealth::Unhealthy { reason } => ("unhealthy", reason),
    };
    ChannelHealthResponse {
        channel: channel_name.to_string(),
        status,
        detail,
        checks: health_checks_from_components(&components),
        components,
    }
}

fn disabled_health_payload(channel_name: &str) -> ChannelHealthResponse {
    let mut components = Map::new();
    components.insert("config".into(), Value::String("disabled".into()));
    components.insert("runtime".into(), Value::String("stopped".into()));
    components.insert("transport".into(), Value::String("disabled".into()));
    components.insert("queue".into(), Value::String("ready".into()));
    ChannelHealthResponse {
        channel: channel_name.to_string(),
        status: "disabled",
        detail: "Channel is disabled.".to_string(),
        checks: health_checks_from_components(&components),
        components,
    }
}

fn degraded_health_payload(
    channel_name: &str,
    config: Option<&Value>,
    detail: String,
) -> ChannelHealthResponse {
    let mut components = base_health_components(channel_name, config, false);
    components.insert("runtime".into(), Value::String("not_running".into()));
    components.insert("transport".into(), Value::String("disconnected".into()));
    if missing_required_detail(channel_name, config).is_some() {
        components.insert("token".into(), Value::String("missing_credentials".into()));
    }
    ChannelHealthResponse {
        channel: channel_name.to_string(),
        status: "degraded",
        detail,
        checks: health_checks_from_components(&components),
        components,
    }
}

fn base_health_components(
    channel_name: &str,
    config: Option<&Value>,
    running: bool,
) -> Map<String, Value> {
    let mut components = Map::new();
    components.insert(
        "config".into(),
        Value::String(
            if missing_required_detail(channel_name, config).is_some() {
                "misconfigured"
            } else {
                "enabled"
            }
            .into(),
        ),
    );
    components.insert(
        "runtime".into(),
        Value::String(if running { "running" } else { "not_running" }.into()),
    );
    components.insert(
        "transport".into(),
        Value::String(if running { "connected" } else { "disconnected" }.into()),
    );
    components.insert("queue".into(), Value::String("ready".into()));
    components
}

fn health_checks_from_components(components: &Map<String, Value>) -> Vec<ChannelHealthCheck> {
    components
        .iter()
        .map(|(name, value)| {
            let state = value.as_str().unwrap_or("unknown");
            let status = match state {
                "enabled" | "running" | "connected" | "ready" | "disabled" | "stopped" => "ok",
                "misconfigured"
                | "not_running"
                | "disconnected"
                | "missing_credentials"
                | "failed"
                | "degraded" => "degraded",
                _ => "unknown",
            };
            ChannelHealthCheck {
                name: name.clone(),
                status,
                detail: state.to_string(),
            }
        })
        .collect()
}

fn token_required_fields(channel_name: &str) -> &'static [&'static str] {
    match channel_name {
        "telegram" => &["bot_token"],
        "discord" => &["bot_token"],
        "qq" => &["app_id", "client_secret"],
        "wecom" => &["bot_id", "secret"],
        "feishu" => &["app_id", "app_secret"],
        "dingtalk" => &["client_id", "client_secret"],
        "matrix" => &["homeserver"],
        "mattermost" => &["url", "bot_token"],
        "slack" => &["bot_token", "app_token"],
        "voice" => &["twilio_account_sid", "twilio_auth_token"],
        "xiaoyi" => &["access_key", "secret_key", "agent_id"],
        "yuanbao" => &["app_id", "app_secret"],
        _ => &[],
    }
}

fn config_missing_string(config: Option<&Value>, field: &str) -> bool {
    config
        .and_then(Value::as_object)
        .and_then(|map| map.get(field))
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
}

fn missing_required_detail(channel_name: &str, config: Option<&Value>) -> Option<String> {
    let resolved = match config
        .map(|config| rua_config::channel_catalog::resolve_channel_config(channel_name, config))
        .transpose()
    {
        Ok(resolved) => resolved,
        Err(error) => return Some(error),
    };
    let config = resolved.as_ref();
    let mut missing = token_required_fields(channel_name)
        .iter()
        .copied()
        .filter(|field| config_missing_string(config, field))
        .collect::<Vec<_>>();
    if channel_name == "voice"
        && config_missing_string(config, "public_url")
        && config_missing_string(config, "phone_number_sid")
    {
        missing.push("public_url or phone_number_sid");
    }
    if channel_name == "matrix"
        && config_missing_string(config, "access_token")
        && (config_missing_string(config, "user_id") || config_missing_string(config, "password"))
    {
        missing.push("access_token or user_id + password");
    }
    if channel_name == "sip" {
        let mode = config.and_then(|v| v["sip_mode"].as_str()).unwrap_or("dev");
        if mode != "native"
            && config_missing_string(config, "dashscope_api_key")
            && std::env::var("DASHSCOPE_API_KEY")
                .unwrap_or_default()
                .is_empty()
        {
            missing.push("dashscope_api_key");
        }
        if mode == "livekit" {
            for field in ["livekit_url", "livekit_api_key", "livekit_api_secret"] {
                if config_missing_string(config, field) {
                    missing.push(field);
                }
            }
        }
    }
    if missing.is_empty() {
        None
    } else {
        Some(format!("missing required fields: {}", missing.join(", ")))
    }
}

fn channel_validation_error(loc: Vec<String>, msg: impl Into<String>) -> ApiError {
    ApiError::Validation(vec![ValidationError {
        loc,
        msg: msg.into(),
        error_type: "type_error".to_string(),
    }])
}

fn validate_channel_config_value(
    channel_name: Option<&str>,
    value: &Value,
) -> Result<(), ApiError> {
    let object = value.as_object().ok_or_else(|| {
        let loc = match channel_name {
            Some(name) => vec!["body".to_string(), name.to_string()],
            None => vec!["body".to_string()],
        };
        channel_validation_error(loc, "channel config must be an object")
    })?;

    if matches!(object.get("enabled"), Some(enabled) if !enabled.is_boolean()) {
        let mut loc = vec!["body".to_string()];
        if let Some(name) = channel_name {
            loc.push(name.to_string());
        }
        loc.push("enabled".to_string());
        return Err(channel_validation_error(loc, "enabled must be a bool"));
    }

    if let Some(name) = channel_name {
        rua_config::channel_catalog::validate_channel_fields(name, value).map_err(|message| {
            channel_validation_error(vec!["body".into(), name.into()], message)
        })?;
    }
    Ok(())
}

fn validate_channels_map(body: &Map<String, Value>) -> Result<(), ApiError> {
    for (channel_name, config) in body {
        validate_channel_config_value(Some(channel_name), config)?;
    }
    Ok(())
}

async fn read_channels_map(
    state: &AppState,
    agent_id: &str,
) -> Result<Map<String, Value>, ApiError> {
    let cell = state.agent_cells.section::<Value>(agent_id, "channels");
    match cell.read().await {
        Ok(Some(Value::Object(map))) => Ok(map),
        Ok(Some(_)) => Ok(Map::new()),
        Ok(None) => Ok(Map::new()),
        Err(e) => Err(ApiError::Internal(format!("channels cell read: {e}"))),
    }
}

fn mask_channels_map(map: Map<String, Value>) -> Map<String, Value> {
    map.into_iter()
        .map(|(key, value)| (key, redact_secrets(&value)))
        .collect()
}

async fn write_channels_value(
    state: &AppState,
    agent_id: &str,
    value: Value,
) -> Result<(), ApiError> {
    let cell = state.agent_cells.section::<Value>(agent_id, "channels");
    cell.write(value)
        .await
        .map_err(|e| ApiError::Internal(format!("channels cell write: {e}")))?;
    crate::channels_reload::apply_current(state, agent_id).await
        .map_err(|_| ApiError::Internal("Channel configuration saved, but resource application failed; retry the saved configuration".to_string()))?;
    Ok(())
}

async fn list_channels_async(state: &AppState, agent_id: Option<&str>) -> Response {
    let aid = resolve_agent_id(agent_id);
    let map = match read_channels_map(state, aid).await {
        Ok(m) => m,
        Err(e) => return e.into_response(),
    };
    let map = mask_channels_map(backfill_builtin_defaults(map));
    let map = inject_is_builtin(map);
    (StatusCode::OK, Json(map)).into_response()
}

async fn channel_schemas_async() -> Response {
    (StatusCode::OK, Json(channel_schemas_response())).into_response()
}

async fn update_all_channels_for(
    state: &AppState,
    agent_id: Option<&str>,
    body: Map<String, Value>,
) -> Response {
    if let Err(e) = validate_channels_map(&body) {
        return e.into_response();
    }

    let aid = resolve_agent_id(agent_id);
    let channels_value = Value::Object(body);
    if let Err(e) = write_channels_value(state, aid, channels_value.clone()).await {
        return e.into_response();
    }
    list_channels_async(state, agent_id).await
}

async fn get_channel_for(state: &AppState, agent_id: Option<&str>, channel_name: &str) -> Response {
    let aid = resolve_agent_id(agent_id);
    let map = match read_channels_map(state, aid).await {
        Ok(m) => m,
        Err(e) => return e.into_response(),
    };
    let map = mask_channels_map(backfill_builtin_defaults(map));
    match map.get(channel_name).cloned() {
        Some(value) => (StatusCode::OK, Json(value)).into_response(),
        None => ApiError::NotFound(format!("Channel '{channel_name}' not found")).into_response(),
    }
}

async fn update_channel_for(
    state: &AppState,
    agent_id: Option<&str>,
    channel_name: &str,
    config: Value,
) -> Response {
    let aid = resolve_agent_id(agent_id);

    let existing_map = match read_channels_map(state, aid).await {
        Ok(map) => map,
        Err(e) => return e.into_response(),
    };
    let mut save_config = config.clone();
    let force = save_config
        .as_object_mut()
        .and_then(|m| m.remove("force"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let enabled = save_config
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Err(e) = validate_channel_config_value(None, &save_config) {
        return e.into_response();
    }
    if let Err(message) =
        rua_config::channel_catalog::validate_channel_fields(channel_name, &save_config)
    {
        return channel_validation_error(vec!["body".into()], message).into_response();
    }

    let save_config = match (
        existing_map.get(channel_name).and_then(Value::as_object),
        save_config.as_object(),
    ) {
        (Some(existing), Some(incoming)) => {
            Value::Object(merge_with_redaction_safety(existing, incoming))
        }
        _ => save_config,
    };

    if enabled {
        if let Some(config_map) = save_config.as_object() {
            if let Some(identifier) =
                rua_workspace::channel_conflict::extract_identifier(channel_name, config_map)
            {
                match rua_workspace::channel_conflict::check_channel_conflict(
                    &state.agent_cells,
                    channel_name,
                    &identifier,
                    aid,
                )
                .await
                {
                    Ok(Some(conflict)) => {
                        if !force {
                            return (StatusCode::OK, Json(conflict)).into_response();
                        }
                        if let Err(e) =
                            rua_workspace::channel_conflict::disable_conflicting_channel(
                                &state.agent_cells,
                                &conflict.conflicting_agent_id,
                                channel_name,
                            )
                            .await
                        {
                            return ApiError::Internal(format!("disable conflicting channel: {e}"))
                                .into_response();
                        }
                        crate::channels_reload::reload_from_disk(
                            state,
                            &conflict.conflicting_agent_id,
                        )
                        .await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "channel conflict check failed, proceeding");
                    }
                }
            }
        }
    }

    let cell = state.agent_cells.section::<Value>(aid, "channels");
    let _channels_value = match cell
        .update(|existing| {
            let mut map = match existing {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            };
            map.insert(channel_name.to_string(), save_config.clone());
            Value::Object(map)
        })
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("channels cell update: {e}")).into_response();
        }
    };
    if crate::channels_reload::apply_current(state, aid)
        .await
        .is_err()
    {
        return ApiError::Internal("Channel configuration saved, but resource application failed; retry the saved configuration".to_string()).into_response();
    }
    (StatusCode::OK, Json(redact_secrets(&save_config))).into_response()
}

async fn get_channel_health_for(
    state: &AppState,
    agent_id: Option<&str>,
    channel_name: &str,
) -> Response {
    let aid = resolve_agent_id(agent_id);
    let map = match read_channels_map(state, aid).await {
        Ok(m) => backfill_builtin_defaults(m),
        Err(e) => return e.into_response(),
    };

    if !channel_available(&map, channel_name) {
        return ApiError::NotFound(format!("Channel '{channel_name}' not available"))
            .into_response();
    }

    let config = map.get(channel_name);
    if channel_name == "voice"
        && aid == state.voice.agent_id()
        && channel_enabled(&map, channel_name)
    {
        if let Some(error) = state.voice_runtime.error().await {
            return Json(degraded_health_payload(channel_name, config, error)).into_response();
        }
        if state.voice.is_enabled() && state.voice_runtime.public_url().await.is_some() {
            let components = base_health_components(channel_name, config, true);
            let checks = health_checks_from_components(&components);
            return Json(ChannelHealthResponse {
                channel: channel_name.into(),
                status: "healthy",
                detail: "Voice webhook is ready.".into(),
                components,
                checks,
            })
            .into_response();
        }
    }

    if channel_enabled(&map, channel_name) {
        if channel_name == "console" {
            let components = base_health_components(channel_name, config, true);
            let checks = health_checks_from_components(&components);
            return Json(ChannelHealthResponse {
                channel: channel_name.to_string(),
                status: "healthy",
                detail: "Channel runtime is running.".to_string(),
                components,
                checks,
            })
            .into_response();
        }

        if let Some(cm) = state.channel_manager_for(aid).await {
            let statuses = cm.statuses().await;
            if let Some(status) = statuses.get(channel_name) {
                return Json(health_payload(channel_name, status, config)).into_response();
            }
        }

        if !is_builtin_channel_key(channel_name) {
            return ApiError::NotFound(format!(
                "Channel '{channel_name}' is not running. It may be disabled or not configured."
            ))
            .into_response();
        }

        let detail = missing_required_detail(channel_name, config).unwrap_or_else(|| {
            format!("Channel '{channel_name}' is enabled but its runtime is not running.")
        });
        return Json(degraded_health_payload(channel_name, config, detail)).into_response();
    }

    Json(disabled_health_payload(channel_name)).into_response()
}

async fn restart_channel_for(
    state: &AppState,
    agent_id: Option<&str>,
    channel_name: &str,
) -> Response {
    let aid = resolve_agent_id(agent_id);
    let map = match read_channels_map(state, aid).await {
        Ok(m) => backfill_builtin_defaults(m),
        Err(e) => return e.into_response(),
    };

    if !channel_available(&map, channel_name) {
        return ApiError::NotFound(format!("Channel '{channel_name}' not available"))
            .into_response();
    }

    if !channel_enabled(&map, channel_name) {
        return ApiError::NotFound(format!(
            "Channel '{channel_name}' is not running. It may be disabled or not configured."
        ))
        .into_response();
    }

    if let Some(detail) = missing_required_detail(channel_name, map.get(channel_name)) {
        return ApiError::NotFound(format!(
            "Channel '{channel_name}' is not configured: {detail}"
        ))
        .into_response();
    }
    if let Err(error) = state.registry.get_or_create(aid).await {
        return ApiError::Internal(format!("Failed to initialize Channel workspace: {error}"))
            .into_response();
    }
    if crate::channels_reload::apply_current_for(state, aid, Some(channel_name))
        .await
        .is_err()
    {
        return ApiError::Internal(
            "Saved Channel configuration could not be reapplied".to_string(),
        )
        .into_response();
    }
    if !matches!(channel_name, "console" | "voice") {
        let running = match state.channel_manager_for(aid).await {
            Some(cm) => cm
                .statuses()
                .await
                .get(channel_name)
                .is_some_and(|status| status.is_running()),
            None => false,
        };
        if !running {
            return ApiError::NotFound(format!(
                "Channel '{channel_name}' is not running. It may be disabled or not configured."
            ))
            .into_response();
        }
    }
    Json(ChannelRestartResponse {
        channel: channel_name.to_string(),
        status: "restarted",
        detail: format!("Channel '{channel_name}' has been restarted."),
    })
    .into_response()
}

async fn list_channels(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    list_channels_async(&state, Some(&ctx.agent_id)).await
}

async fn update_all_channels(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    update_all_channels_for(&state, Some(&ctx.agent_id), body).await
}

async fn get_channel_types(
    State(_state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    (StatusCode::OK, Json(builtin_channel_types())).into_response()
}

async fn get_channel_schemas(
    State(_state): State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    channel_schemas_async().await
}

async fn get_channel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel_name): Path<String>,
) -> Response {
    get_channel_for(&state, Some(&ctx.agent_id), &channel_name).await
}

async fn update_channel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel_name): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    update_channel_for(&state, Some(&ctx.agent_id), &channel_name, body).await
}

async fn get_channel_health(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel_name): Path<String>,
) -> Response {
    get_channel_health_for(&state, Some(&ctx.agent_id), &channel_name).await
}

async fn restart_channel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel_name): Path<String>,
) -> Response {
    restart_channel_for(&state, Some(&ctx.agent_id), &channel_name).await
}

async fn get_qrcode(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel): Path<String>,
    Query(query): Query<QrCodeQuery>,
) -> Response {
    let handler = match state.qrcode_registry.get(&channel).await {
        Some(h) => h,
        None => {
            return ApiError::NotFound(format!("QR code not supported for channel: {channel}"))
                .into_response();
        }
    };
    let params = QRCodeAuthParams {
        domain: query.domain,
    };
    match handler
        .fetch_qrcode_with_params(&ctx.agent_id, &params)
        .await
    {
        Ok(result) => {
            let qrcode_img = match crate::qrcode_image::generate_qrcode_image(&result.scan_url) {
                Ok(img) => img,
                Err(e) => return e.into_response(),
            };
            Json(serde_json::json!({
                "qrcode_img": qrcode_img,
                "poll_token": result.poll_token,
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn get_qrcode_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(channel): Path<String>,
    Query(query): Query<QrCodeStatusQuery>,
) -> Response {
    let handler = match state.qrcode_registry.get(&channel).await {
        Some(h) => h,
        None => {
            return ApiError::NotFound(format!("QR code not supported for channel: {channel}"))
                .into_response();
        }
    };
    let params = QRCodeAuthParams {
        domain: query.domain,
    };
    match handler
        .poll_status_with_params(&query.token, &ctx.agent_id, &params)
        .await
    {
        Ok(result) => Json(serde_json::json!({
            "status": result.status,
            "credentials": result.credentials,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn list_channels_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    list_channels_async(&state, Some(&agent_id)).await
}

async fn update_all_channels_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    update_all_channels_for(&state, Some(&agent_id), body).await
}

async fn get_channel_types_agent(
    State(_state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    (StatusCode::OK, Json(builtin_channel_types())).into_response()
}

async fn get_channel_schemas_agent(
    State(_state): State<AppState>,
    Path(_agent_id): Path<String>,
) -> Response {
    channel_schemas_async().await
}

async fn get_channel_agent(
    State(state): State<AppState>,
    Path((agent_id, channel_name)): Path<(String, String)>,
) -> Response {
    get_channel_for(&state, Some(&agent_id), &channel_name).await
}

async fn update_channel_agent(
    State(state): State<AppState>,
    Path((agent_id, channel_name)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    update_channel_for(&state, Some(&agent_id), &channel_name, body).await
}

async fn get_channel_health_agent(
    State(state): State<AppState>,
    Path((agent_id, channel_name)): Path<(String, String)>,
) -> Response {
    get_channel_health_for(&state, Some(&agent_id), &channel_name).await
}

async fn restart_channel_agent(
    State(state): State<AppState>,
    Path((agent_id, channel_name)): Path<(String, String)>,
) -> Response {
    restart_channel_for(&state, Some(&agent_id), &channel_name).await
}

async fn get_qrcode_agent(
    State(state): State<AppState>,
    Path((agent_id, channel)): Path<(String, String)>,
    Query(query): Query<QrCodeQuery>,
) -> Response {
    let handler = match state.qrcode_registry.get(&channel).await {
        Some(h) => h,
        None => {
            return ApiError::NotFound(format!("QR code not supported for channel: {channel}"))
                .into_response();
        }
    };
    let params = QRCodeAuthParams {
        domain: query.domain,
    };
    match handler.fetch_qrcode_with_params(&agent_id, &params).await {
        Ok(result) => {
            let qrcode_img = match crate::qrcode_image::generate_qrcode_image(&result.scan_url) {
                Ok(img) => img,
                Err(e) => return e.into_response(),
            };
            Json(serde_json::json!({
                "qrcode_img": qrcode_img,
                "poll_token": result.poll_token,
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn get_qrcode_status_agent(
    State(state): State<AppState>,
    Path((agent_id, channel)): Path<(String, String)>,
    Query(query): Query<QrCodeStatusQuery>,
) -> Response {
    let handler = match state.qrcode_registry.get(&channel).await {
        Some(h) => h,
        None => {
            return ApiError::NotFound(format!("QR code not supported for channel: {channel}"))
                .into_response();
        }
    };
    let params = QRCodeAuthParams {
        domain: query.domain,
    };
    match handler
        .poll_status_with_params(&query.token, &agent_id, &params)
        .await
    {
        Ok(result) => Json(serde_json::json!({
            "status": result.status,
            "credentials": result.credentials,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

pub fn dingtalk_webhook_router() -> Router<AppState> {
    Router::new().route("/channel/dingtalk/webhook", post(dingtalk_webhook))
}

pub fn dingtalk_webhook_compat_router() -> Router<AppState> {
    Router::new().route("/dingtalk/webhook", post(dingtalk_webhook))
}

pub fn dingtalk_webhook_channels_router() -> Router<AppState> {
    Router::new().route("/channels/dingtalk/webhook", post(dingtalk_webhook))
}

async fn dingtalk_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let app_secret = read_channels_map(&state, "default")
        .await
        .ok()
        .and_then(|m| m.get("dingtalk").cloned())
        .and_then(|d| {
            d.get("app_secret")
                .and_then(|v| v.as_str().map(String::from))
        })
        .filter(|s| !s.is_empty());

    if let Some(ref secret) = app_secret {
        let timestamp = headers
            .get("timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let sign = headers
            .get("sign")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !dingtalk_verify_signature(timestamp, sign, secret) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"detail": "Invalid DingTalk signature"})),
            )
                .into_response();
        }
    }

    let body_val: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"detail": format!("Invalid request body: {}", e)})),
            )
                .into_response();
        }
    };

    let text = body_val
        .get("text")
        .and_then(|t| t.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim())
        .unwrap_or("");

    if text.is_empty() {
        tracing::debug!("dingtalk webhook: no text content");
        return (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ignored", "channel": "dingtalk"})),
        )
            .into_response();
    }

    let msg_id = body_val
        .get("msgId")
        .or_else(|| body_val.get("msg_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let sender_id = body_val
        .get("senderId")
        .or_else(|| body_val.get("sender_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let conversation_id = body_val
        .get("conversationId")
        .or_else(|| body_val.get("conversation_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    let push_msg = crate::endpoints::dto::PushMessage {
        id: msg_id,
        content: text.to_string(),
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
        extra: {
            let mut m = serde_json::Map::new();
            m.insert(
                "channel_id".into(),
                serde_json::Value::String("dingtalk".into()),
            );
            m.insert(
                "sender_id".into(),
                serde_json::Value::String(sender_id.to_string()),
            );
            m.insert(
                "conversation_id".into(),
                serde_json::Value::String(conversation_id.to_string()),
            );
            m
        },
    };
    state.push_message_store.push(push_msg).await;

    let payload = serde_json::json!({
        "channel_id": "dingtalk",
        "sender_id": sender_id,
        "conversation_id": conversation_id,
        "text": text,
    });
    let active_agent = AppState::resolve_active_agent_id(&state.rua_root);
    if let Some(cm) = state.channel_manager_for(&active_agent).await {
        if let Err(e) = cm
            .handle_incoming("dingtalk".to_string(), conversation_id.to_string(), payload)
            .await
        {
            tracing::warn!(error = %e, "channel_manager handle_incoming failed, message still stored locally");
        }
    } else {
        tracing::warn!("channel_manager not available, message stored locally only");
    }

    tracing::info!(
        channel = "dingtalk",
        sender = %sender_id,
        conversation = %conversation_id,
        "dingtalk webhook: message stored and enqueued"
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "received",
            "channel": "dingtalk",
        })),
    )
        .into_response()
}

const SHA256_BLOCK_SIZE: usize = 64;

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; SHA256_BLOCK_SIZE];
    if key.len() > SHA256_BLOCK_SIZE {
        let hash = Sha256::digest(key);
        key_block[..32].copy_from_slice(&hash);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; SHA256_BLOCK_SIZE];
    let mut opad = [0x5cu8; SHA256_BLOCK_SIZE];
    for i in 0..SHA256_BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let result = outer.finalize();

    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

fn dingtalk_base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

fn dingtalk_verify_signature(timestamp: &str, sign: &str, app_secret: &str) -> bool {
    if timestamp.is_empty() || sign.is_empty() || app_secret.is_empty() {
        return false;
    }
    let string_to_sign = format!("{}\n{}", timestamp, app_secret);
    let hmac = hmac_sha256(app_secret.as_bytes(), string_to_sign.as_bytes());
    let expected = dingtalk_base64_encode(&hmac);
    expected == sign
}

pub fn dingtalk_compute_signature(timestamp: &str, app_secret: &str) -> String {
    let string_to_sign = format!("{}\n{}", timestamp, app_secret);
    let hmac = hmac_sha256(app_secret.as_bytes(), string_to_sign.as_bytes());
    dingtalk_base64_encode(&hmac)
}
