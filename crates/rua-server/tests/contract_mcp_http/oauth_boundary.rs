use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::support::{build_app, build_sync_enabled_app, read_json, read_text, request};

const AGENT_ID: &str = "mcp_oauth_contract_agent";

fn stdio_payload(client_key: &str) -> Value {
    json!({
        "client_key": client_key,
        "client": {
            "name": "oauth stdio mcp client",
            "description": "created by oauth contract tests",
            "enabled": true,
            "transport": "stdio",
            "command": "/bin/echo",
            "args": ["mcp"]
        }
    })
}

fn http_payload(client_key: &str) -> Value {
    http_payload_with_url(client_key, "https://mcp.example.test/mcp")
}

fn http_payload_with_url(client_key: &str, url: &str) -> Value {
    json!({
        "client_key": client_key,
        "client": {
            "name": "oauth http mcp client",
            "description": "created by oauth contract tests",
            "enabled": true,
            "transport": "streamable_http",
            "url": url
        }
    })
}

fn http_payload_with_oauth(client_key: &str) -> Value {
    json!({
        "client_key": client_key,
        "client": {
            "name": "oauth authorized mcp client",
            "description": "created by oauth contract tests",
            "enabled": true,
            "transport": "streamable_http",
            "url": "https://mcp.example.test/mcp",
            "oauth": {
                "client_id": "stored-client",
                "scope": "read write",
                "access_token": "stored-access-token",
                "refresh_token": "stored-refresh-token",
                "expires_at": 4_102_444_800.0,
                "token_endpoint": "https://auth.example.test/token",
                "auth_endpoint": "https://auth.example.test/authorize"
            }
        }
    })
}

async fn create_client(app: &axum::Router, agent_id: &str, payload: Value) {
    let create = request(app, Method::POST, "/api/mcp", Some(payload), Some(agent_id)).await;
    assert_eq!(create.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_client_oauth_routes_return_fastapi_detail_errors() {
    let app = build_app();
    let missing = "missing_oauth_client";

    let status = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{missing}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(status.status(), StatusCode::NOT_FOUND);
    assert!(read_json(status).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let start = request(
        &app.app,
        Method::POST,
        &format!("/api/mcp/oauth/start/{missing}"),
        Some(json!({
            "url": "https://mcp.example.test/mcp",
            "auth_endpoint": "https://auth.example.test/authorize",
            "token_endpoint": "https://auth.example.test/token"
        })),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(start.status(), StatusCode::NOT_FOUND);
    assert!(read_json(start).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let revoke = request(
        &app.app,
        Method::DELETE,
        &format!("/api/mcp/oauth/{missing}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::NOT_FOUND);
    assert!(read_json(revoke).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_callback_error_param_returns_html_bad_request() {
    let app = build_app();

    let callback = request(
        &app.app,
        Method::GET,
        "/api/mcp/oauth/callback?error=access_denied&error_description=Test+denied+by+user",
        None,
        None,
    )
    .await;

    assert_eq!(callback.status(), StatusCode::BAD_REQUEST);
    assert!(callback
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html")));
    let body = read_text(callback).await;
    assert!(body.contains("Test denied by user"));
    assert!(body.contains("mcp-oauth"));
    assert!(body.contains("error"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_client_without_oauth_reports_unauthorized_empty_status() {
    let app = build_app();
    let client_key = "oauth_status_empty";
    create_client(&app.app, AGENT_ID, http_payload(client_key)).await;

    let status = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    let body = read_json(status).await;
    assert_eq!(body["authorized"], false);
    assert_eq!(body["expires_at"], 0.0);
    assert_eq!(body["scope"], "");
    assert_eq!(body["client_id"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_start_with_explicit_endpoints_builds_pkce_authorization_url_without_discovery() {
    let app = build_app();
    let client_key = "oauth_start_explicit";
    create_client(&app.app, AGENT_ID, http_payload(client_key)).await;

    let start = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{AGENT_ID}/mcp/oauth/start/{client_key}"),
        Some(json!({
            "auth_endpoint": "https://auth.example.test/authorize",
            "token_endpoint": "https://auth.example.test/token",
            "client_id": "rua-test-client",
            "scope": "read write"
        })),
        None,
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    let body = read_json(start).await;
    let auth_url = body["auth_url"].as_str().expect("auth_url");
    assert!(body["session_id"]
        .as_str()
        .is_some_and(|session_id| !session_id.is_empty()));
    assert!(auth_url.starts_with("https://auth.example.test/authorize?"));
    assert!(auth_url.contains("response_type=code"));
    assert!(auth_url.contains("client_id=rua-test-client"));
    assert!(auth_url.contains("scope=read+write"));
    assert!(auth_url.contains("redirect_uri="));
    assert!(auth_url.contains("state="));
    assert!(auth_url.contains("code_challenge="));
    assert!(auth_url.contains("code_challenge_method=S256"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_discovery_treats_empty_authorization_servers_as_missing_metadata() {
    let app = build_app();
    let client_key = "oauth_empty_auth_servers";
    let mcp_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).insert_header(
            "www-authenticate",
            format!(
                r#"Bearer resource_metadata="{}/resource-metadata""#,
                mcp_server.uri()
            ),
        ))
        .mount(&mcp_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/resource-metadata"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_servers": []
        })))
        .mount(&mcp_server)
        .await;

    create_client(
        &app.app,
        AGENT_ID,
        http_payload_with_url(client_key, &format!("{}/mcp", mcp_server.uri())),
    )
    .await;

    let start = request(
        &app.app,
        Method::POST,
        &format!("/api/mcp/oauth/start/{client_key}"),
        Some(json!({})),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(start.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(start).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("Could not discover OAuth authorization server")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_start_for_stdio_client_requires_explicit_mcp_url() {
    let app = build_app();
    let client_key = "oauth_stdio_requires_url";
    create_client(&app.app, AGENT_ID, stdio_payload(client_key)).await;

    let start = request(
        &app.app,
        Method::POST,
        &format!("/api/mcp/oauth/start/{client_key}"),
        Some(json!({
            "auth_endpoint": "https://auth.example.test/authorize",
            "token_endpoint": "https://auth.example.test/token"
        })),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(start.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(start).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("MCP url is required for OAuth")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_oauth_routes_use_path_agent_over_conflicting_header() {
    let app = build_app();
    let client_key = "oauth_path_precedence";
    let path_agent = "oauth_path_agent";
    let header_agent = "oauth_header_agent";
    create_client(&app.app, path_agent, http_payload(client_key)).await;

    let path_status = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/mcp/oauth/status/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(path_status.status(), StatusCode::OK);
    assert_eq!(read_json(path_status).await["authorized"], false);

    let root_status = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(root_status.status(), StatusCode::NOT_FOUND);
    assert!(read_json(root_status).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(client_key)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_revoke_clears_existing_tokens_and_reports_empty_status() {
    let app = build_app();
    let client_key = "oauth_revoke_existing";
    create_client(&app.app, AGENT_ID, http_payload_with_oauth(client_key)).await;

    let before = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_body = read_json(before).await;
    assert_eq!(before_body["authorized"], true);
    assert_eq!(before_body["client_id"], "stored-client");
    assert_eq!(before_body["scope"], "read write");

    let revoke = request(
        &app.app,
        Method::DELETE,
        &format!("/api/mcp/oauth/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::OK);
    assert_eq!(read_json(revoke).await["message"], "OAuth tokens cleared");

    let after = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_body = read_json(after).await;
    assert_eq!(after_body["authorized"], false);
    assert_eq!(after_body["expires_at"], 0.0);
    assert_eq!(after_body["scope"], "");
    assert_eq!(after_body["client_id"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_callback_persists_encrypted_tokens_and_appends_sync_change() {
    let app = build_sync_enabled_app();
    let client_key = "oauth_callback_sync_emit";
    create_client(&app.app, AGENT_ID, http_payload(client_key)).await;

    let sync_node = app
        .state
        .sync_node
        .lock()
        .await
        .as_ref()
        .expect("sync node")
        .doc()
        .clone();
    let before = app
        .state
        .inbound_inbox
        .repair_scan_doc_entries(&sync_node)
        .await
        .expect("baseline repair scan");

    let token_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "oauth-callback-access-secret",
            "refresh_token": "oauth-callback-refresh-secret",
            "expires_in": 3600,
            "scope": "read write"
        })))
        .mount(&token_server)
        .await;

    let start = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{AGENT_ID}/mcp/oauth/start/{client_key}"),
        Some(json!({
            "auth_endpoint": "https://auth.example.test/authorize",
            "token_endpoint": format!("{}/token", token_server.uri()),
            "client_id": "callback-client",
            "scope": "read write"
        })),
        None,
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    let start_body = read_json(start).await;
    let session_id = start_body["session_id"].as_str().expect("session id");

    let callback = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/callback?code=auth-code&state={session_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(callback.status(), StatusCode::OK);
    assert!(read_text(callback)
        .await
        .contains("Authorization successful"));

    let status = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/oauth/status/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    assert_eq!(status_body["authorized"], true);
    assert_eq!(status_body["client_id"], "callback-client");
    assert_eq!(status_body["scope"], "read write");

    let client = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(client.status(), StatusCode::OK);
    let client_body = read_json(client).await;
    let client_text = client_body.to_string();
    assert!(!client_text.contains("oauth-callback-access-secret"));
    assert!(!client_text.contains("oauth-callback-refresh-secret"));
    assert!(client_body["oauth"]["access_token"]
        .as_str()
        .is_some_and(|value| value.contains('*') && value.ends_with("cret")));
    assert!(client_body["oauth"]["refresh_token"]
        .as_str()
        .is_some_and(|value| value.contains('*') && value.ends_with("cret")));

    let agent_json_path = app
        .data_dir()
        .join("workspaces")
        .join(AGENT_ID)
        .join("agent.json");
    let disk_text = std::fs::read_to_string(agent_json_path).expect("agent.json");
    assert!(!disk_text.contains("oauth-callback-access-secret"));
    assert!(!disk_text.contains("oauth-callback-refresh-secret"));
    assert!(disk_text.contains("rua_encrypted"));

    let after = app
        .state
        .inbound_inbox
        .repair_scan_doc_entries(&sync_node)
        .await
        .expect("callback repair scan");
    assert!(
        after.scanned >= before.scanned,
        "OAuth callback should leave the agent_config sync record visible; before={before:?}, after={after:?}"
    );
    assert!(
        after.recorded > 0 || after.already_applied >= before.already_applied,
        "OAuth callback sync record should be visible in the local sync doc; before={before:?}, after={after:?}"
    );
}
