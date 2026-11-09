use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;

use rua_mcp::store_dto::McpOAuthConfig;
use rua_mcp::McpClientStore;

use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

const SESSION_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
struct OAuthSession {
    agent_id: String,
    client_key: String,
    code_verifier: String,
    client_id: String,
    auth_endpoint: String,
    token_endpoint: String,
    redirect_uri: String,
    scope: String,
    created_at: Instant,
}

impl OAuthSession {
    fn expired(&self) -> bool {
        self.created_at.elapsed() > SESSION_TTL
    }
}

fn sessions() -> &'static RwLock<HashMap<String, OAuthSession>> {
    static SESSIONS: OnceLock<RwLock<HashMap<String, OAuthSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/mcp/oauth/start/{client_key}", post(oauth_start))
        .route("/mcp/oauth/callback", get(oauth_callback))
        .route("/mcp/oauth/status/{client_key}", get(oauth_status))
        .route("/mcp/oauth/{client_key}", delete(oauth_revoke))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/mcp/oauth/start/{client_key}",
            post(oauth_start_agent),
        )
        .route(
            "/agents/{agentId}/mcp/oauth/status/{client_key}",
            get(oauth_status_agent),
        )
        .route(
            "/agents/{agentId}/mcp/oauth/{client_key}",
            delete(oauth_revoke_agent),
        )
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthStartRequest {
    #[serde(default)]
    url: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    auth_endpoint: String,
    #[serde(default)]
    token_endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
struct OAuthStartResponse {
    auth_url: String,
    session_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct OAuthStatusResponse {
    authorized: bool,
    expires_at: f64,
    scope: String,
    client_id: String,
}

async fn build_store(state: &AppState) -> Result<McpClientStore, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for MCP OAuth store: {e}"
        ))
    })?;
    Ok(McpClientStore::with_agent_mcp_cells(
        runtime.rua_root(),
        std::sync::Arc::clone(&state.agent_mcp_cells),
    ))
    .map(|store| store.with_device_key(*state.device_key.as_bytes()))
}

async fn purge_expired_sessions() {
    sessions()
        .write()
        .await
        .retain(|_, session| !session.expired());
}

fn code_verifier() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
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

fn redirect_uri(headers: &HeaderMap) -> String {
    format!("{}/api/mcp/oauth/callback", request_base_url(headers))
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    match client
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        Ok(_) | Err(_) => None,
    }
}

async fn probe_resource_metadata_url(client: &reqwest::Client, mcp_url: &str) -> Option<String> {
    let resp = client
        .get(mcp_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if resp.status() != StatusCode::UNAUTHORIZED {
        return None;
    }
    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    for part in header.split(',') {
        let trimmed = part.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("resource_metadata=") {
            return trimmed
                .split_once('=')
                .map(|(_, v)| v.trim().trim_matches('"').to_string());
        }
    }
    None
}

async fn resolve_auth_server_url(client: &reqwest::Client, mcp_url: &str) -> Option<String> {
    if let Some(rm_url) = probe_resource_metadata_url(client, mcp_url).await {
        if let Some(prm) = fetch_json(client, &rm_url).await {
            if let Some(url) = prm
                .get("authorization_servers")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
            {
                return Some(url.to_string());
            }
        }
    }

    let parsed = reqwest::Url::parse(mcp_url).ok()?;
    let root = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default()
    );
    let root = match parsed.port() {
        Some(port) => format!("{root}:{port}"),
        None => root,
    };
    let suffix = parsed.path().trim_start_matches('/');
    let mut candidates = Vec::new();
    if !suffix.is_empty() {
        candidates.push(format!(
            "{root}/.well-known/oauth-protected-resource/{suffix}"
        ));
    }
    candidates.push(format!("{root}/.well-known/oauth-protected-resource"));
    for url in candidates {
        if let Some(prm) = fetch_json(client, &url).await {
            if let Some(auth_server) = prm
                .get("authorization_servers")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
            {
                return Some(auth_server.to_string());
            }
        }
    }
    None
}

async fn fetch_as_metadata(
    client: &reqwest::Client,
    auth_server_url: &str,
) -> Option<serde_json::Value> {
    let parsed = reqwest::Url::parse(auth_server_url).ok()?;
    let root = format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default(),
        parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let suffix = parsed.path().trim_matches('/');
    let mut candidates = Vec::new();
    if !suffix.is_empty() {
        candidates.push(format!(
            "{root}/.well-known/oauth-authorization-server/{suffix}"
        ));
        candidates.push(format!("{root}/.well-known/openid-configuration/{suffix}"));
        candidates.push(format!(
            "{}/.well-known/openid-configuration",
            auth_server_url.trim_end_matches('/')
        ));
    } else {
        candidates.push(format!("{root}/.well-known/oauth-authorization-server"));
        candidates.push(format!("{root}/.well-known/openid-configuration"));
    }

    for url in candidates {
        if let Some(meta) = fetch_json(client, &url).await {
            if meta.get("authorization_endpoint").is_some() {
                return Some(meta);
            }
        }
    }
    None
}

async fn discover_oauth_metadata(
    mcp_url: &str,
) -> Result<(String, String, Option<String>), String> {
    let client = rua_http::build_client(
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::limited(10)),
    )
    .map_err(|e| format!("failed to build OAuth HTTP client: {e}"))?;
    let auth_server = resolve_auth_server_url(&client, mcp_url).await.ok_or_else(|| {
        "Could not discover OAuth authorization server for this MCP endpoint; provide auth_endpoint and token_endpoint manually.".to_string()
    })?;
    let meta = fetch_as_metadata(&client, &auth_server)
        .await
        .ok_or_else(|| {
            format!("Could not retrieve authorization server metadata from {auth_server}")
        })?;
    let auth_endpoint = meta
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "authorization server metadata missing authorization_endpoint".to_string())?
        .to_string();
    let token_endpoint = meta
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "authorization server metadata missing token_endpoint".to_string())?
        .to_string();
    let registration_endpoint = meta
        .get("registration_endpoint")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    Ok((auth_endpoint, token_endpoint, registration_endpoint))
}

async fn dynamic_register(registration_endpoint: &str, redirect_uri: &str) -> Option<String> {
    let payload = serde_json::json!({
        "client_name": "rua MCP Client",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let client = rua_http::default_client().ok()?;
    let resp = client
        .post(registration_endpoint)
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !(resp.status().is_success() || resp.status() == StatusCode::CREATED) {
        return None;
    }
    resp.json::<serde_json::Value>()
        .await
        .ok()?
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

async fn oauth_start_inner(
    state: AppState,
    agent_id: String,
    client_key: String,
    headers: HeaderMap,
    body: OAuthStartRequest,
) -> Response {
    purge_expired_sessions().await;

    let store = match build_store(&state).await {
        Ok(store) => store,
        Err(e) => return e.into_response(),
    };
    let client_cfg = match store
        .get_client_for_execution(Some(&agent_id), &client_key)
        .await
    {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            return ApiError::NotFound(format!("MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
    };

    let mcp_url = if body.url.trim().is_empty() {
        match &client_cfg.server.transport {
            mcp_manager::types::McpTransportConfig::StreamableHttp { url, .. }
            | mcp_manager::types::McpTransportConfig::Sse { url, .. } => url.clone(),
            mcp_manager::types::McpTransportConfig::Stdio { .. } => String::new(),
        }
    } else {
        body.url.clone()
    };
    if mcp_url.trim().is_empty() {
        return ApiError::BadRequest("MCP url is required for OAuth".into()).into_response();
    }

    let redirect_uri = redirect_uri(&headers);
    let (auth_endpoint, token_endpoint, registration_endpoint) =
        if !body.auth_endpoint.is_empty() && !body.token_endpoint.is_empty() {
            (
                body.auth_endpoint.clone(),
                body.token_endpoint.clone(),
                None,
            )
        } else {
            match discover_oauth_metadata(&mcp_url).await {
                Ok(meta) => meta,
                Err(e) => return ApiError::BadRequest(e).into_response(),
            }
        };

    let mut client_id = body.client_id.clone();
    if client_id.is_empty() {
        if let Some(registration_endpoint) = registration_endpoint {
            client_id = dynamic_register(&registration_endpoint, &redirect_uri)
                .await
                .unwrap_or_default();
        }
    }

    let verifier = code_verifier();
    let challenge = code_challenge(&verifier);
    let session_id = Uuid::new_v4().simple().to_string();
    sessions().write().await.insert(
        session_id.clone(),
        OAuthSession {
            agent_id,
            client_key,
            code_verifier: verifier,
            client_id: client_id.clone(),
            auth_endpoint: auth_endpoint.clone(),
            token_endpoint: token_endpoint.clone(),
            redirect_uri: redirect_uri.clone(),
            scope: body.scope.clone(),
            created_at: Instant::now(),
        },
    );

    let mut url = match reqwest::Url::parse(&auth_endpoint) {
        Ok(url) => url,
        Err(e) => {
            return ApiError::BadRequest(format!("invalid auth_endpoint: {e}")).into_response();
        }
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("redirect_uri", &redirect_uri);
        pairs.append_pair("state", &session_id);
        pairs.append_pair("code_challenge", &challenge);
        pairs.append_pair("code_challenge_method", "S256");
        if !client_id.is_empty() {
            pairs.append_pair("client_id", &client_id);
        }
        if !body.scope.is_empty() {
            pairs.append_pair("scope", &body.scope);
        }
    }

    (
        StatusCode::OK,
        Json(OAuthStartResponse {
            auth_url: url.to_string(),
            session_id,
        }),
    )
        .into_response()
}

async fn oauth_start(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    headers: HeaderMap,
    Path(client_key): Path<String>,
    Json(body): Json<OAuthStartRequest>,
) -> Response {
    oauth_start_inner(state, ctx.agent_id, client_key, headers, body).await
}

async fn oauth_start_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_id, client_key)): Path<(String, String)>,
    Json(body): Json<OAuthStartRequest>,
) -> Response {
    oauth_start_inner(state, agent_id, client_key, headers, body).await
}

async fn exchange_code_for_tokens(
    session: &OAuthSession,
    code: &str,
) -> Result<serde_json::Value, String> {
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", session.redirect_uri.clone()),
        ("code_verifier", session.code_verifier.clone()),
    ];
    if !session.client_id.is_empty() {
        params.push(("client_id", session.client_id.clone()));
    }
    let client = rua_http::default_client()
        .map_err(|e| format!("failed to build OAuth HTTP client: {e}"))?;
    let resp = client
        .post(&session.token_endpoint)
        .form(&params)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to read token response: {e}"))?;
    if !status.is_success() {
        return Err(format!("token exchange failed (HTTP {status}): {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid token response JSON: {e}"))
}

fn unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

async fn persist_tokens(
    state: &AppState,
    session: &OAuthSession,
    tokens: serde_json::Value,
) -> Result<(), String> {
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Token response did not contain an access_token".to_string())?
        .to_string();
    let expires_in = tokens
        .get("expires_in")
        .and_then(|v| v.as_f64())
        .unwrap_or(3600.0);
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let scope = tokens
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or(&session.scope)
        .to_string();

    let store = build_store(state).await.map_err(|e| e.to_string())?;
    let existing = store
        .get_client_for_execution(Some(&session.agent_id), &session.client_key)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("MCP client '{}' not found", session.client_key))?;
    let previous = existing.server.oauth.unwrap_or_default();
    let oauth = McpOAuthConfig {
        client_id: if session.client_id.is_empty() {
            previous.client_id
        } else {
            session.client_id.clone()
        },
        scope,
        access_token,
        refresh_token,
        expires_at: unix_secs() + expires_in,
        token_endpoint: session.token_endpoint.clone(),
        auth_endpoint: if session.auth_endpoint.is_empty() {
            previous.auth_endpoint
        } else {
            session.auth_endpoint.clone()
        },
        extra: previous.extra,
    };
    store
        .set_oauth(Some(&session.agent_id), &session.client_key, Some(oauth))
        .await
        .map_err(|e| e.to_string())?;
    let application = crate::mcp_reload::apply_current(state, &session.agent_id).await;
    crate::endpoints::mcp::publish_agent_config_after_mcp_write(state, &store, &session.agent_id)
        .await;
    if application.status == "failed" {
        return Err("OAuth configuration saved, but MCP resource application failed; retry the saved configuration".to_string());
    }
    Ok(())
}

fn popup_html(status: &str, message: &str) -> String {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>MCP OAuth</title></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#f8fafc">
<div style="background:#fff;border:1px solid #e5e7eb;border-radius:8px;padding:32px 40px;text-align:center;max-width:460px">
<p style="font-size:16px;font-weight:600">{escaped}</p>
<button onclick="window.close()" style="margin-top:16px;padding:8px 24px">Close</button>
</div>
<script>
(function(){{
  var data={{type:"mcp-oauth",status:{status:?}}};
  try{{localStorage.setItem("mcp_oauth_result",JSON.stringify(data));}}catch(e){{}}
  if(window.opener&&!window.opener.closed){{try{{window.opener.postMessage(data,"*");}}catch(e){{}}}}
  setTimeout(function(){{window.close();}},1500);
}})();
</script></body></html>"#
    )
}

fn popup_response(status: StatusCode, state: &str, message: &str) -> Response {
    (status, Html(popup_html(state, message))).into_response()
}

async fn oauth_callback(State(state): State<AppState>, request: Request) -> Response {
    purge_expired_sessions().await;
    let query = request.uri().query().unwrap_or("");
    let params: HashMap<String, String> = query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect();
    if let Some(error) = params.get("error") {
        let message = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        return popup_response(StatusCode::BAD_REQUEST, "error", message);
    }
    let Some(code) = params.get("code") else {
        return popup_response(StatusCode::BAD_REQUEST, "error", "Missing code parameter");
    };
    let Some(session_id) = params.get("state") else {
        return popup_response(StatusCode::BAD_REQUEST, "error", "Missing state parameter");
    };
    let session = {
        let guard = sessions().read().await;
        guard.get(session_id).cloned()
    };
    let Some(session) = session else {
        return popup_response(
            StatusCode::BAD_REQUEST,
            "error",
            "OAuth session expired or not found",
        );
    };
    if session.expired() {
        sessions().write().await.remove(session_id);
        return popup_response(StatusCode::BAD_REQUEST, "error", "OAuth session expired");
    }

    match exchange_code_for_tokens(&session, code).await {
        Ok(tokens) => {
            if let Err(e) = persist_tokens(&state, &session, tokens).await {
                debug!(error = %e, "failed to persist MCP OAuth tokens");
                return popup_response(StatusCode::BAD_REQUEST, "error", &e);
            }
        }
        Err(e) => return popup_response(StatusCode::BAD_REQUEST, "error", &e),
    }
    sessions().write().await.remove(session_id);
    popup_response(StatusCode::OK, "success", "Authorization successful")
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn oauth_status_inner(state: AppState, agent_id: String, client_key: String) -> Response {
    let store = match build_store(&state).await {
        Ok(store) => store,
        Err(e) => return e.into_response(),
    };
    let client = match store
        .get_client_for_execution(Some(&agent_id), &client_key)
        .await
    {
        Ok(Some(client)) => client,
        Ok(None) => {
            return ApiError::NotFound(format!("MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    let Some(oauth) = client.server.oauth else {
        return (
            StatusCode::OK,
            Json(OAuthStatusResponse {
                authorized: false,
                expires_at: 0.0,
                scope: String::new(),
                client_id: String::new(),
            }),
        )
            .into_response();
    };
    (
        StatusCode::OK,
        Json(OAuthStatusResponse {
            authorized: oauth.has_valid_access_token(),
            expires_at: oauth.expires_at,
            scope: oauth.scope,
            client_id: oauth.client_id,
        }),
    )
        .into_response()
}

async fn oauth_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
) -> Response {
    oauth_status_inner(state, ctx.agent_id, client_key).await
}

async fn oauth_status_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
) -> Response {
    oauth_status_inner(state, agent_id, client_key).await
}

async fn oauth_revoke_inner(state: AppState, agent_id: String, client_key: String) -> Response {
    let store = match build_store(&state).await {
        Ok(store) => store,
        Err(e) => return e.into_response(),
    };
    match store.set_oauth(Some(&agent_id), &client_key, None).await {
        Ok(_) => {
            let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
            crate::endpoints::mcp::publish_agent_config_after_mcp_write(&state, &store, &agent_id)
                .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({"message": "OAuth tokens cleared", "application": application})),
            )
                .into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn oauth_revoke(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
) -> Response {
    oauth_revoke_inner(state, ctx.agent_id, client_key).await
}

async fn oauth_revoke_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
) -> Response {
    oauth_revoke_inner(state, agent_id, client_key).await
}
