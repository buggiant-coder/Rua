use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use mcp_manager::client::McpClient;
use mcp_manager::manager::McpManager;
use mcp_manager::types::{
    AutoConnectPolicy, McpConfig, McpOAuthConfig, McpServerConfig, McpServerId, McpTransportConfig,
    ToolFilter,
};

use rua_mcp::manager::McpManagerImpl;
use rua_mcp::transports::http::{HttpConfig, HttpMcpClient};
use rua_mcp::{McpMobilePolicy, PlatformProfile};

#[derive(Clone, Default)]
struct Captured {
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    auth_headers: Arc<Mutex<Vec<Vec<String>>>>,
}

async fn mock_mcp_handler(
    State(captured): State<Captured>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);

    captured.requests.lock().await.push(parsed.clone());
    captured.auth_headers.lock().await.push(
        headers
            .get_all(axum::http::header::AUTHORIZATION)
            .iter()
            .filter_map(|v| v.to_str().ok().map(ToString::to_string))
            .collect(),
    );

    let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = parsed.get("id").cloned();

    let response = match method {
        "initialize" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "mock", "version": "0.1"},
                "capabilities": {}
            }
        }),
        "tools/list" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "echoes input",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }
        }),
        "tools/call" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "isError": false
            }
        }),
        "notifications/initialized" => {
            return (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{}",
            )
                .into_response();
        }
        _ => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
    };

    let mut resp_headers = axum::http::HeaderMap::new();
    resp_headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    if let Some(sid) = headers.get("mcp-session-id") {
        resp_headers.insert("mcp-session-id", sid.clone());
    } else {
        resp_headers.insert("mcp-session-id", "test-session-42".parse().unwrap());
    }

    (resp_headers, serde_json::to_string(&response).unwrap()).into_response()
}

async fn start_mock_server(captured: Captured) -> String {
    let app = Router::new()
        .route("/mcp", post(mock_mcp_handler))
        .with_state(captured);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{}/mcp", addr.port())
}

fn make_config(url: &str) -> HttpConfig {
    HttpConfig {
        server_key: "test".into(),
        server_name: "test".into(),
        url: url.to_string(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        call_timeout: Duration::from_secs(5),
        allow_private_ip: true,
    }
}

#[tokio::test]
async fn initialized_notification_has_no_id_field() {
    let captured = Captured::default();
    let url = start_mock_server(captured.clone()).await;

    let client = HttpMcpClient::new(make_config(&url));
    client.connect().await.expect("connect should succeed");

    let reqs = captured.requests.lock().await;
    assert!(reqs.len() >= 2, "expected >=2 requests, got {}", reqs.len());

    let initialized = reqs
        .iter()
        .find(|r| r.get("method").and_then(|m| m.as_str()) == Some("notifications/initialized"))
        .expect("no notifications/initialized request captured");

    assert!(
        initialized.get("id").is_none(),
        "notifications/initialized MUST NOT contain an id field per JSON-RPC spec, \
         but got: {initialized}"
    );

    assert_eq!(
        initialized.get("jsonrpc").and_then(|v| v.as_str()),
        Some("2.0"),
    );
    assert_eq!(
        initialized.get("method").and_then(|v| v.as_str()),
        Some("notifications/initialized"),
    );
}

#[tokio::test]
async fn agent_runtime_uses_http_mcp_client() {
    let captured = Captured::default();
    let url = start_mock_server(captured.clone()).await;

    let client = HttpMcpClient::new(make_config(&url));
    client.connect().await.expect("connect should succeed");
    let runtime_tools = client
        .list_tools()
        .await
        .expect("list_tools should succeed");

    assert_eq!(runtime_tools.len(), 1);
    assert_eq!(runtime_tools[0].name, "echo");

    let reqs = captured.requests.lock().await;
    let methods: Vec<&str> = reqs
        .iter()
        .filter_map(|r| r.get("method").and_then(|m| m.as_str()))
        .collect();

    let init_count = methods.iter().filter(|m| **m == "initialize").count();
    let notif_count = methods
        .iter()
        .filter(|m| **m == "notifications/initialized")
        .count();
    let list_count = methods.iter().filter(|m| **m == "tools/list").count();

    assert_eq!(init_count, 1);
    assert_eq!(notif_count, 1);
    assert_eq!(list_count, 1);

    for req in reqs.iter() {
        if req.get("method").and_then(|m| m.as_str()) == Some("notifications/initialized") {
            assert!(
                req.get("id").is_none(),
                "notifications/initialized should have no id: {req}"
            );
        }
    }
}

#[tokio::test]
async fn oauth_authorization_header_is_sent_on_http_requests() {
    let captured = Captured::default();
    let url = start_mock_server(captured.clone()).await;
    let mut config = make_config(&url);
    config
        .headers
        .insert("Authorization".into(), "Bearer oauth-token".into());

    let client = HttpMcpClient::new(config);
    client.connect().await.expect("connect should succeed");
    client
        .list_tools()
        .await
        .expect("list_tools should succeed");

    let auth_headers = captured.auth_headers.lock().await;
    assert!(
        auth_headers
            .iter()
            .all(|values| values == &vec!["Bearer oauth-token".to_string()]),
        "all MCP HTTP requests should carry the OAuth bearer token: {auth_headers:?}"
    );
}

#[tokio::test]
async fn manager_injects_valid_oauth_token_for_http_client() {
    let captured = Captured::default();
    let url = start_mock_server(captured.clone()).await;
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let mut config = McpConfig::default();
    config.clients.insert(
        McpServerId::new("remote"),
        McpServerConfig {
            name: "remote".into(),
            description: String::new(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url,
                headers: BTreeMap::new(),
                timeout_secs: 5,
            },
            auto_connect: AutoConnectPolicy::LazyOnFirstUse,
            call_timeout_secs: 5,
            max_concurrency: 4,
            oauth: Some(McpOAuthConfig {
                access_token: "oauth-token".into(),
                expires_at: 4_102_444_800.0,
                ..Default::default()
            }),
            disabled_tools: Vec::new(),
            extra: BTreeMap::from([("allow_private_ip".into(), serde_json::json!(true))]),
        },
    );

    manager.load_config(config).await.expect("load config");
    manager
        .list_tools(ToolFilter {
            server_id: Some(McpServerId::new("remote")),
            name_prefix: None,
            include_disabled: false,
        })
        .await
        .expect("list tools");

    let auth_headers = captured.auth_headers.lock().await;
    assert!(
        auth_headers
            .iter()
            .all(|values| values == &vec!["Bearer oauth-token".to_string()]),
        "manager-created HTTP client should carry OAuth bearer token: {auth_headers:?}"
    );
}

#[tokio::test]
async fn manager_oauth_replaces_legacy_authorization_header_case_insensitively() {
    let captured = Captured::default();
    let url = start_mock_server(captured.clone()).await;
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let mut config = McpConfig::default();
    config.clients.insert(
        McpServerId::new("remote"),
        McpServerConfig {
            name: "remote".into(),
            description: String::new(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url,
                headers: BTreeMap::from([(
                    "authorization".to_string(),
                    "Bearer stale-token".to_string(),
                )]),
                timeout_secs: 5,
            },
            auto_connect: AutoConnectPolicy::LazyOnFirstUse,
            call_timeout_secs: 5,
            max_concurrency: 4,
            oauth: Some(McpOAuthConfig {
                access_token: "oauth-token".into(),
                expires_at: 4_102_444_800.0,
                ..Default::default()
            }),
            disabled_tools: Vec::new(),
            extra: BTreeMap::from([("allow_private_ip".into(), serde_json::json!(true))]),
        },
    );

    manager.load_config(config).await.expect("load config");
    manager
        .list_tools(ToolFilter {
            server_id: Some(McpServerId::new("remote")),
            name_prefix: None,
            include_disabled: false,
        })
        .await
        .expect("list tools");

    let auth_headers = captured.auth_headers.lock().await;
    assert!(
        auth_headers
            .iter()
            .all(|values| values == &vec!["Bearer oauth-token".to_string()]),
        "OAuth token should replace stale configured Authorization headers: {auth_headers:?}"
    );
}

#[tokio::test]
async fn manager_does_not_block_disabled_tools_at_transport_layer() {
    let captured = Captured::default();
    let url = start_mock_server(captured).await;
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let mut config = McpConfig::default();
    config.clients.insert(
        McpServerId::new("remote"),
        McpServerConfig {
            name: "remote".into(),
            description: String::new(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url,
                headers: BTreeMap::new(),
                timeout_secs: 5,
            },
            auto_connect: AutoConnectPolicy::LazyOnFirstUse,
            call_timeout_secs: 5,
            max_concurrency: 4,
            oauth: None,
            disabled_tools: vec!["echo".into()],
            extra: BTreeMap::from([("allow_private_ip".into(), serde_json::json!(true))]),
        },
    );
    manager.load_config(config).await.expect("load config");

    let result = manager
        .call_tool(
            &McpServerId::new("remote"),
            "echo",
            serde_json::json!({"text": "hi"}),
            Default::default(),
        )
        .await
        .expect("manager transport call should still reach the MCP server");
    assert!(
        result.text().contains("ok"),
        "expected mock call result, got {result:?}"
    );
}

#[tokio::test]
async fn strict_server_rejects_notification_with_id() {
    async fn strict_handler(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);

        let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = parsed.get("id").cloned();

        let is_notification = method.starts_with("notifications/");
        if is_notification && parsed.get("id").is_some() {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "notification MUST NOT contain id field",
            )
                .into_response();
        }

        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {"name": "strict-mock", "version": "0.1"},
                    "capabilities": {}
                }
            }),
            "notifications/initialized" => {
                return (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    "{}",
                )
                    .into_response();
            }
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "ping",
                            "description": "pong",
                            "inputSchema": {"type": "object"}
                        }
                    ]
                }
            }),
            _ => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        };

        let mut resp_headers = axum::http::HeaderMap::new();
        resp_headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        if let Some(sid) = headers.get("mcp-session-id") {
            resp_headers.insert("mcp-session-id", sid.clone());
        } else {
            resp_headers.insert("mcp-session-id", "strict-session-1".parse().unwrap());
        }

        (resp_headers, serde_json::to_string(&response).unwrap()).into_response()
    }

    let app = Router::new().route("/mcp", post(strict_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = HttpMcpClient::new(make_config(&url));
    client
        .connect()
        .await
        .expect("connect to strict server should succeed (notification has no id)");

    let tools = client.list_tools().await.expect("list_tools should work");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "ping");
}
