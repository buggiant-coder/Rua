use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{mcp, mcp_oauth};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn seed_agent(tmp: &TempDir, id: &str) {
    let dir = tmp.path().join("workspaces").join(id);
    std::fs::create_dir_all(&dir).expect("seed workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": id,
            "name": id,
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
        }))
        .unwrap(),
    )
    .expect("write agent.json");
}

fn build_state(tmp: &TempDir) -> AppState {
    AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test)
}

#[tokio::test]
async fn applying_saved_mcp_defers_unloaded_agents_and_preserves_loaded_runtime() {
    let tmp = TempDir::new().unwrap();
    seed_agent(&tmp, "mcp-apply-isolated");
    // Linux has no bundled desktop MCP drivers; give this runtime an explicit owner.
    let agent_path = tmp.path().join("workspaces/mcp-apply-isolated/agent.json");
    let mut agent: Value = serde_json::from_slice(&std::fs::read(&agent_path).unwrap()).unwrap();
    agent["mcp"] = json!({"clients": {"disabled-fixture": {
        "name": "Fixture", "enabled": false, "transport": "stdio",
        "command": "no-driver-needed", "auto_connect": "manual"
    }}});
    rua_config::atomic_write_json(&agent_path, &agent).unwrap();
    let state = build_state(&tmp);
    let app = Router::new()
        .nest("/api", mcp::agent_scoped_router())
        .with_state(state.clone());
    let deferred = app
        .clone()
        .oneshot(empty_req(
            "POST",
            "/api/agents/mcp-apply-isolated/mcp/apply",
        ))
        .await
        .unwrap();
    assert_eq!(read_json(deferred).await["status"], "deferred");
    assert!(state.registry.get("mcp-apply-isolated").await.is_none());
    let runtime = state
        .registry
        .get_or_create("mcp-apply-isolated")
        .await
        .unwrap();
    let memory = runtime.memory_manager().await.unwrap();
    let react = runtime.react_runtime().await.unwrap();
    let applied = app
        .oneshot(empty_req(
            "POST",
            "/api/agents/mcp-apply-isolated/mcp/apply",
        ))
        .await
        .unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
    assert_eq!(read_json(applied).await["status"], "applied");
    assert!(std::sync::Arc::ptr_eq(
        &runtime,
        &state.registry.get("mcp-apply-isolated").await.unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &memory,
        &runtime.memory_manager().await.unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &react,
        &runtime.react_runtime().await.unwrap()
    ));
}

fn enable_sync(tmp: &TempDir) {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");
}

fn build_mcp_app(tmp: &TempDir) -> (Router, AppState) {
    let state = build_state(tmp);
    let app = Router::new()
        .nest("/api", mcp::root_router().merge(mcp_oauth::root_router()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state)
}

fn req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn empty_req(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap()
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let (sync_doc, blobs_store) = {
        let guard = state.sync_node.lock().await;
        let node = guard.as_ref().expect("sync node");
        (node.doc().clone(), node.blobs_store().clone())
    };
    let network_secret = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .clone();
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("device")
        .clone();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        sync_doc.clone(),
        device.to_iroh_author(),
        network_secret.derive_sync_key(),
        blobs_store,
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut changes = Vec::new();
    for entry in entries {
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_section_write_persists_to_agent_json() {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "default");

    let state = build_state(&tmp);

    let mcp_cfg = mcp_manager::types::McpConfig {
        clients: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                mcp_manager::types::McpServerId::new("test-srv"),
                mcp_manager::types::McpServerConfig {
                    name: "test-srv".to_string(),
                    description: "e2e test server".to_string(),
                    enabled: true,
                    transport: mcp_manager::types::McpTransportConfig::Stdio {
                        command: "/bin/echo".to_string(),
                        args: vec![],
                        env: std::collections::BTreeMap::new(),
                        cwd: None,
                        use_proot: false,
                    },
                    auto_connect: Default::default(),
                    call_timeout_secs: 60,
                    max_concurrency: 4,
                    oauth: None,
                    disabled_tools: Vec::new(),
                    extra: std::collections::BTreeMap::new(),
                },
            );
            m
        },
    };

    state
        .agent_mcp_cells
        .cell("default")
        .write(mcp_cfg)
        .await
        .expect("write mcp");

    let path = tmp.path().join("workspaces/default/agent.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");

    assert!(
        raw.get("mcp").is_some(),
        "agent.json must have $.mcp after cell write, got {raw:#}"
    );
    assert_eq!(
        raw["mcp"]["clients"]["test-srv"]["name"].as_str(),
        Some("test-srv"),
    );
    assert_eq!(
        raw["name"].as_str(),
        Some("default"),
        "other sections must be preserved"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_external_edit_mcp_section_takes_effect() {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "default");

    let state = build_state(&tmp);

    let before = state
        .agent_mcp_cells
        .cell("default")
        .read()
        .await
        .expect("read");
    assert!(before.is_none(), "no mcp section initially");

    let path = tmp.path().join("workspaces/default/agent.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    raw["mcp"] = json!({
        "clients": {
            "external-srv": {
                "name": "external-srv",
                "transport": "stdio",
                "command": "/bin/true",
                "args": [],
                "env": {}
            }
        }
    });
    std::fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).expect("external write");

    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for mtime");
    f.set_modified(later).expect("set_modified");

    let after = state
        .agent_mcp_cells
        .cell("default")
        .read()
        .await
        .expect("read after external edit");
    assert!(
        after.is_some(),
        "mcp section must be visible after external edit"
    );
    let cfg = after.unwrap();
    assert!(
        cfg.clients
            .contains_key(&mcp_manager::types::McpServerId::new("external-srv")),
        "external-srv must appear, got {:?}",
        cfg.clients.keys().collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_agents_mcp_updates_isolated() {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "agent-a");
    seed_agent(&tmp, "agent-b");

    let state = build_state(&tmp);

    let mcp_a = mcp_manager::types::McpConfig {
        clients: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                mcp_manager::types::McpServerId::new("srv-a"),
                mcp_manager::types::McpServerConfig {
                    name: "srv-a".to_string(),
                    description: String::new(),
                    enabled: true,
                    transport: mcp_manager::types::McpTransportConfig::Stdio {
                        command: "/bin/echo".to_string(),
                        args: vec!["a".to_string()],
                        env: std::collections::BTreeMap::new(),
                        cwd: None,
                        use_proot: false,
                    },
                    auto_connect: Default::default(),
                    call_timeout_secs: 30,
                    max_concurrency: 2,
                    oauth: None,
                    disabled_tools: Vec::new(),
                    extra: std::collections::BTreeMap::new(),
                },
            );
            m
        },
    };

    let mcp_b = mcp_manager::types::McpConfig {
        clients: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                mcp_manager::types::McpServerId::new("srv-b"),
                mcp_manager::types::McpServerConfig {
                    name: "srv-b".to_string(),
                    description: String::new(),
                    enabled: true,
                    transport: mcp_manager::types::McpTransportConfig::Stdio {
                        command: "/bin/echo".to_string(),
                        args: vec!["b".to_string()],
                        env: std::collections::BTreeMap::new(),
                        cwd: None,
                        use_proot: false,
                    },
                    auto_connect: Default::default(),
                    call_timeout_secs: 30,
                    max_concurrency: 2,
                    oauth: None,
                    disabled_tools: Vec::new(),
                    extra: std::collections::BTreeMap::new(),
                },
            );
            m
        },
    };

    state
        .agent_mcp_cells
        .cell("agent-a")
        .write(mcp_a)
        .await
        .expect("write a");
    state
        .agent_mcp_cells
        .cell("agent-b")
        .write(mcp_b)
        .await
        .expect("write b");

    let a_cfg = state
        .agent_mcp_cells
        .cell("agent-a")
        .read()
        .await
        .expect("read a")
        .expect("a present");
    let b_cfg = state
        .agent_mcp_cells
        .cell("agent-b")
        .read()
        .await
        .expect("read b")
        .expect("b present");

    assert!(a_cfg
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("srv-a")));
    assert!(!a_cfg
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("srv-b")));
    assert!(b_cfg
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("srv-b")));
    assert!(!b_cfg
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("srv-a")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_agent_mcp_client_emits_agent_config_v4_when_sync_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "default");
    enable_sync(&tmp);
    let (app, state) = build_mcp_app(&tmp);

    let resp = app
        .oneshot(req(
            "POST",
            "/api/mcp",
            json!({
                "key": "sync-agent-mcp",
                "name": "sync-agent-mcp",
                "enabled": true,
                "transport": "stdio",
                "command": "/bin/echo",
                "args": ["ok"],
                "env": {"TOKEN": "agent-mcp-secret"},
                "headers": {"Authorization": "Bearer agent-mcp-auth"}
            }),
        ))
        .await
        .expect("send POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let raw_agent_json =
        std::fs::read_to_string(tmp.path().join("workspaces/default/agent.json")).expect("read");
    assert!(
        !raw_agent_json.contains("agent-mcp-secret")
            && !raw_agent_json.contains("Bearer agent-mcp-auth"),
        "agent MCP secrets must be encrypted at rest in agent.json"
    );

    let changes = collect_v4_changes(&state).await;
    let agent_config_change = changes
        .iter()
        .find(|change| change.domain == "agent_config" && change.record_id == "default");
    assert!(
        agent_config_change.is_some_and(|change| {
            change.payload["mcp"]["clients"]["sync-agent-mcp"]["env"]["TOKEN"]
                == json!("agent-mcp-secret")
                && change.payload["mcp"]["clients"]["sync-agent-mcp"]["headers"]["Authorization"]
                    == json!("Bearer agent-mcp-auth")
        }),
        "per-agent MCP POST must append an agent_config v4 change with logical MCP secret payload; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_oauth_callback_emits_agent_config_v4_with_logical_tokens() {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "default");
    enable_sync(&tmp);
    let (app, state) = build_mcp_app(&tmp);

    let token_app = Router::new().route(
        "/token",
        post(|| async {
            axum::Json(json!({
                "access_token": "mcp-oauth-access-token",
                "refresh_token": "mcp-oauth-refresh-token",
                "expires_in": 1800,
                "scope": "tools:read"
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind token server");
    let token_addr = listener.local_addr().expect("token addr");
    let token_server = tokio::spawn(async move {
        axum::serve(listener, token_app)
            .await
            .expect("token server");
    });

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/mcp",
            json!({
                "key": "oauth-agent-mcp",
                "name": "oauth-agent-mcp",
                "enabled": true,
                "transport": "streamable_http",
                "url": "https://mcp.example.test/mcp"
            }),
        ))
        .await
        .expect("send MCP create");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let create_change = changes
        .iter()
        .find(|change| {
            change.domain == "agent_config"
                && change.record_id == "default"
                && change.payload["mcp"]["clients"]["oauth-agent-mcp"].is_object()
                && change.payload["mcp"]["clients"]["oauth-agent-mcp"]["oauth"].is_null()
        })
        .cloned()
        .expect("MCP create should emit initial agent_config change");

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/mcp/oauth/start/oauth-agent-mcp",
            json!({
                "url": "https://mcp.example.test/mcp",
                "auth_endpoint": "https://auth.example.test/authorize",
                "token_endpoint": format!("http://{token_addr}/token"),
                "client_id": "mcp-oauth-client",
                "scope": "tools:read"
            }),
        ))
        .await
        .expect("send OAuth start");
    assert_eq!(resp.status(), StatusCode::OK);
    let started = read_json(resp).await;
    let session_id = started["session_id"].as_str().expect("session id");

    let resp = app
        .oneshot(empty_req(
            "GET",
            &format!("/api/mcp/oauth/callback?code=oauth-code&state={session_id}"),
        ))
        .await
        .expect("send OAuth callback");
    assert_eq!(resp.status(), StatusCode::OK);
    token_server.abort();

    let raw_agent_json =
        std::fs::read_to_string(tmp.path().join("workspaces/default/agent.json")).expect("read");
    assert!(
        !raw_agent_json.contains("mcp-oauth-access-token")
            && !raw_agent_json.contains("mcp-oauth-refresh-token"),
        "MCP OAuth tokens must be encrypted at rest in agent.json"
    );

    let changes = collect_v4_changes(&state).await;
    let oauth_change = changes.iter().find(|change| {
        change.domain == "agent_config"
            && change.record_id == "default"
            && change.payload["mcp"]["clients"]["oauth-agent-mcp"]["oauth"]["access_token"]
                == json!("mcp-oauth-access-token")
    });
    assert!(
        oauth_change.is_some_and(|change| {
            let oauth = &change.payload["mcp"]["clients"]["oauth-agent-mcp"]["oauth"];
            oauth["refresh_token"] == json!("mcp-oauth-refresh-token")
                && oauth["client_id"] == json!("mcp-oauth-client")
                && oauth["scope"] == json!("tools:read")
        }),
        "MCP OAuth callback must append an agent_config v4 change with logical OAuth tokens; got {changes:#?}"
    );
    let oauth_change = oauth_change.expect("MCP OAuth callback should emit agent_config change");
    assert!(
        oauth_change.hlc > create_change.hlc,
        "back-to-back v4 emits must have monotonic HLCs so OAuth callback wins on receivers; create={:?}, oauth={:?}",
        create_change.hlc,
        oauth_change.hlc
    );
}

#[tokio::test]
async fn mcp_section_write_reports_application_and_preserves_the_runtime() {
    let tmp = TempDir::new().unwrap();
    seed_agent(&tmp, "default");
    let state = build_state(&tmp);
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let app = Router::new()
        .nest("/api", mcp::agent_scoped_router())
        .with_state(state.clone());
    let response = app
        .oneshot(req(
            "POST",
            "/api/agents/default/mcp",
            json!({
                "key":"application-fixture", "name":"Application fixture", "enabled":false,
                "transport":"stdio", "command":"fixture"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        read_json(response).await["application"]["status"],
        "applied"
    );
    assert!(std::sync::Arc::ptr_eq(
        &runtime,
        &state.registry.get("default").await.unwrap()
    ));
    assert!(runtime
        .mcp_manager()
        .await
        .unwrap()
        .configuration()
        .await
        .unwrap()
        .clients
        .contains_key(&mcp_manager::types::McpServerId::new("application-fixture")));
}
