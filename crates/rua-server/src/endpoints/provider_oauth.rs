use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use rua_provider::config_loader::ProviderConfig;

use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

const SESSION_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
struct ProviderOAuthSession {
    provider_id: String,
    state: String,
    status: ProviderOAuthStatus,
    error: Option<String>,
    created_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderOAuthStatus {
    Pending,
    Completed,
    Failed,
}

impl ProviderOAuthStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

fn sessions() -> &'static RwLock<HashMap<String, ProviderOAuthSession>> {
    static SESSIONS: OnceLock<RwLock<HashMap<String, ProviderOAuthSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/providers/{provider_id}/oauth/start", post(oauth_start))
        .route(
            "/providers/{provider_id}/oauth/callback",
            get(oauth_callback),
        )
        .route("/providers/{provider_id}/oauth/status", get(oauth_status))
}

#[derive(Debug, Serialize)]
struct OAuthStartResponse {
    authorize_url: String,
    state: String,
    flow_type: String,
}

#[derive(Debug, Serialize)]
struct OAuthStatusResponse {
    status: String,
    error: Option<String>,
}

fn supports_provider_oauth(provider_id: &str) -> bool {
    provider_id == "openrouter"
}

async fn purge_expired_sessions() {
    sessions()
        .write()
        .await
        .retain(|_, session| session.created_at.elapsed() <= SESSION_TTL);
}

fn request_base_url(headers: &HeaderMap) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1");
    format!("{proto}://{host}")
}

fn callback_url(headers: &HeaderMap, provider_id: &str) -> String {
    format!(
        "{}/api/providers/{}/oauth/callback",
        request_base_url(headers),
        provider_id
    )
}

fn openrouter_authorize_url(callback_url: &str) -> Result<String, ApiError> {
    let url = reqwest::Url::parse_with_params(
        "https://openrouter.ai/auth",
        &[("callback_url", callback_url)],
    )
    .map_err(|e| ApiError::Internal(format!("failed to build OAuth URL: {e}")))?;
    Ok(url.to_string())
}

async fn oauth_start(
    Path(provider_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<OAuthStartResponse>, ApiError> {
    purge_expired_sessions().await;
    if !supports_provider_oauth(&provider_id) {
        return Err(ApiError::NotFound(format!(
            "Provider '{provider_id}' does not support OAuth"
        )));
    }

    let state = Uuid::new_v4().to_string();
    let redirect = callback_url(&headers, &provider_id);
    let authorize_url = openrouter_authorize_url(&redirect)?;
    let session = ProviderOAuthSession {
        provider_id: provider_id.clone(),
        state: state.clone(),
        status: ProviderOAuthStatus::Pending,
        error: None,
        created_at: Instant::now(),
    };
    sessions().write().await.insert(state.clone(), session);

    Ok(Json(OAuthStartResponse {
        authorize_url,
        state,
        flow_type: "browser_redirect".into(),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct OAuthCallbackQuery {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn oauth_callback(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    purge_expired_sessions().await;
    if !query.error.is_empty() {
        let message = if query.error_description.is_empty() {
            query.error.as_str()
        } else {
            query.error_description.as_str()
        };
        return popup_response(StatusCode::BAD_REQUEST, "error", message);
    }
    if query.code.is_empty() {
        return popup_response(StatusCode::BAD_REQUEST, "error", "Missing code parameter");
    }

    let session_key = if query.state.is_empty() {
        latest_pending_session_for_provider(&provider_id).await
    } else {
        Some(query.state.clone())
    };
    let Some(session_key) = session_key else {
        return popup_response(
            StatusCode::BAD_REQUEST,
            "error",
            "OAuth session expired or not found",
        );
    };
    let session = {
        let guard = sessions().read().await;
        guard.get(&session_key).cloned()
    };
    let Some(session) = session else {
        return popup_response(
            StatusCode::BAD_REQUEST,
            "error",
            "OAuth session expired or not found",
        );
    };
    if session.provider_id != provider_id {
        fail_session(&session_key, "Provider mismatch").await;
        return popup_response(StatusCode::BAD_REQUEST, "error", "Provider mismatch");
    }

    let result = match exchange_openrouter_code(&query.code).await {
        Ok(api_key) => persist_provider_key(&state, &provider_id, api_key).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => {
            complete_session(&session_key).await;
            popup_response(StatusCode::OK, "success", "Authorization successful")
        }
        Err(message) => {
            fail_session(&session_key, &message).await;
            popup_response(StatusCode::INTERNAL_SERVER_ERROR, "error", &message)
        }
    }
}

async fn latest_pending_session_for_provider(provider_id: &str) -> Option<String> {
    let guard = sessions().read().await;
    guard
        .iter()
        .filter(|(_, session)| {
            session.provider_id == provider_id && session.status == ProviderOAuthStatus::Pending
        })
        .max_by_key(|(_, session)| session.created_at)
        .map(|(state, _)| state.clone())
}

async fn complete_session(state: &str) {
    if let Some(session) = sessions().write().await.get_mut(state) {
        session.status = ProviderOAuthStatus::Completed;
        session.error = None;
    }
}

async fn fail_session(state: &str, error: &str) {
    if let Some(session) = sessions().write().await.get_mut(state) {
        session.status = ProviderOAuthStatus::Failed;
        session.error = Some(error.to_string());
    }
}

async fn exchange_openrouter_code(code: &str) -> Result<String, String> {
    let client = rua_http::default_client()
        .map_err(|e| format!("failed to build OAuth HTTP client: {e}"))?;
    let response = client
        .post("https://openrouter.ai/api/v1/auth/keys")
        .json(&serde_json::json!({ "code": code }))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("OAuth token exchange failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "OAuth token exchange failed with status {}",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("OAuth token response was not JSON: {e}"))?;
    body.get("key")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "OAuth token response did not contain an API key".to_string())
}

async fn persist_provider_key(
    state: &AppState,
    provider_id: &str,
    api_key: String,
) -> Result<(), String> {
    let template_config = provider_config_from_template(provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not support OAuth"))?;
    let config = state
        .provider_cells
        .cell(provider_id)
        .update(|existing| {
            let mut config = existing.unwrap_or(template_config);
            config.api_key = api_key;
            config
                .extra
                .insert("supports_oauth".into(), serde_json::Value::Bool(true));
            config
                .extra
                .insert("oauth_connected".into(), serde_json::Value::Bool(true));
            config
                .extra
                .insert("is_free_tier".into(), serde_json::Value::Bool(true));
            config
        })
        .await
        .map_err(|e| e.to_string())?;

    let provider_id = provider_id.to_string();
    let config_val = serde_json::to_value(&config).unwrap_or_default();
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id,
            config: config_val,
        },
    )
    .await;
    Ok(())
}

pub(crate) fn provider_config_from_template(provider_id: &str) -> Option<ProviderConfig> {
    let template = rua_provider::manager::PROVIDER_TEMPLATES
        .iter()
        .find(|template| template.id == provider_id)?;
    if !template.supports_oauth() {
        return None;
    }
    Some(template.to_config())
}

async fn oauth_status(
    Path(provider_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<OAuthStatusResponse> {
    purge_expired_sessions().await;
    let Some(state) = query.get("state") else {
        return Json(OAuthStatusResponse {
            status: "failed".into(),
            error: Some("Missing state".into()),
        });
    };
    let session = {
        let guard = sessions().read().await;
        guard.get(state).cloned()
    };
    let Some(session) = session else {
        return Json(OAuthStatusResponse {
            status: "failed".into(),
            error: Some("Session expired".into()),
        });
    };
    if session.provider_id != provider_id || session.state != *state {
        return Json(OAuthStatusResponse {
            status: "failed".into(),
            error: Some("Provider mismatch".into()),
        });
    }
    Json(OAuthStatusResponse {
        status: session.status.as_str().into(),
        error: session.error,
    })
}

fn popup_html(status: &str, message: &str) -> String {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Provider OAuth</title></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0">
<div style="text-align:center;max-width:460px;padding:32px 40px">
<p style="font-size:16px;font-weight:600">{escaped}</p>
<button onclick="window.close()" style="margin-top:16px;padding:8px 24px">Close</button>
</div>
<script>
(function(){{
  var data={{type:"provider-oauth",status:{status:?}}};
  try{{localStorage.setItem("provider_oauth_result",JSON.stringify(data));}}catch(e){{}}
  if(window.opener&&!window.opener.closed){{try{{window.opener.postMessage(data,"*");}}catch(e){{}}}}
  setTimeout(function(){{window.close();}},1500);
}})();
</script></body></html>"#
    )
}

fn popup_response(status: StatusCode, state: &str, message: &str) -> Response {
    (status, Html(popup_html(state, message))).into_response()
}
