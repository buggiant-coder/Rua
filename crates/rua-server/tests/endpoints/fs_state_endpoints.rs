use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{
    agent_templates, config, config_channels, config_misc, envs, settings, subagent_templates,
    token_usage,
};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_sync_enabled_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for root-config v4 emit regression test"
    );
    (state, tmp)
}

fn router_with_agent_ctx(state: AppState, sub: Router<AppState>) -> Router {
    Router::new()
        .nest("/api", sub)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

fn router_no_agent_ctx(state: AppState, sub: Router<AppState>) -> Router {
    Router::new().nest("/api", sub).with_state(state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON body: {} (body={})",
            e,
            String::from_utf8_lossy(&bytes)
        )
    })
}

fn req(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
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
async fn envs_endpoint_full_cycle() {
    let (state, _tmp) = build_state();
    let app = router_no_agent_ctx(state, envs::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/envs", None))
        .await
        .expect("get empty");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);

    let put_body = json!({ "FOO": "bar", "BAZ": "qux" });
    let resp = app
        .clone()
        .oneshot(req(Method::PUT, "/api/envs", Some(put_body)))
        .await
        .expect("put map");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let arr = body.as_array().expect("array body");
    assert_eq!(arr.len(), 2);

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/envs", None))
        .await
        .expect("get after put");
    let body = read_json(resp).await;
    let keys: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"FOO"));
    assert!(keys.contains(&"BAZ"));

    let resp = app
        .clone()
        .oneshot(req(Method::DELETE, "/api/envs/FOO", None))
        .await
        .expect("delete FOO");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let keys: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["key"].as_str().unwrap())
        .collect();
    assert!(!keys.contains(&"FOO"));
    assert!(keys.contains(&"BAZ"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn envs_update_and_delete_do_not_emit_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_no_agent_ctx(state.clone(), envs::router());

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/envs",
            Some(json!({
                "SYNC_LOCAL_SECRET": "must-not-sync",
                "SYNC_LOCAL_PATH": "/device/local/path"
            })),
        ))
        .await
        .expect("put envs");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(Method::DELETE, "/api/envs/SYNC_LOCAL_SECRET", None))
        .await
        .expect("delete env");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "envs are device-local and PUT/DELETE must not append v4 sync changes; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_language_endpoint() {
    let (state, _tmp) = build_state();
    let app = router_no_agent_ctx(state, settings::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/settings/language", None))
        .await
        .expect("get language");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["language"], "zh");

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/settings/language",
            Some(json!({ "language": "zh-CN" })),
        ))
        .await
        .expect("put language");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["language"], "zh-CN");

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/version", None))
        .await
        .expect("get version");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body["version"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_language_update_emits_root_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_no_agent_ctx(state.clone(), settings::router());

    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/settings/language",
            Some(json!({ "language": "ja" })),
        ))
        .await
        .expect("put language");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let settings_change = changes
        .iter()
        .find(|change| change.domain == "root_config" && change.record_id == "settings");
    assert!(
        settings_change.is_some_and(|change| {
            change.kind == "upsert" && change.payload["language"] == "ja"
        }),
        "settings language PUT must append a root_config/settings v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_usage_endpoint_returns_empty_summary() {
    let (state, _tmp) = build_state();
    let app = router_no_agent_ctx(state, token_usage::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/token-usage", None))
        .await
        .expect("get usage");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total_prompt_tokens"], 0);
    assert_eq!(body["total_completion_tokens"], 0);
    assert_eq!(body["total_calls"], 0);
    assert_eq!(body["total_tokens"], 0);
    assert_eq!(body["count"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_templates_endpoint_reads_startup_builtins_then_create() {
    let (state, _tmp) = build_state();
    let app = router_with_agent_ctx(state, agent_templates::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/agent-templates", None))
        .await
        .expect("list");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let ids: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"default"));
    assert!(ids.contains(&"blank"));
    assert!(ids.contains(&"qa"));

    let create_body = json!({
        "name": "feat-706 test template",
        "description": "from integration test",
        "prompt_files": ["AGENT_SAFETY.md"]
    });
    let resp = app
        .clone()
        .oneshot(req(Method::POST, "/api/agent-templates", Some(create_body)))
        .await
        .expect("create");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = read_json(resp).await;
    let created_id = body["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(req(Method::DELETE, "/api/agent-templates/default", None))
        .await
        .expect("delete builtin");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(req(
            Method::DELETE,
            &format!("/api/agent-templates/{created_id}"),
            None,
        ))
        .await
        .expect("delete custom");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn template_reads_never_rewrite_or_reseed_files() {
    let (state, tmp) = build_state();
    let app = router_with_agent_ctx(state, agent_templates::router());
    let path = tmp.path().join("agent_templates.json");
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    for uri in ["/api/agent-templates", "/api/agent-templates/default"] {
        let response = app
            .clone()
            .oneshot(req(Method::GET, uri, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    std::fs::remove_file(&path).unwrap();
    let response = app
        .oneshot(req(Method::GET, "/api/agent-templates", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_json(response).await, json!([]));
    assert!(!path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_template_create_emits_agent_template_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_with_agent_ctx(state.clone(), agent_templates::router());

    let create_body = json!({
        "name": "sync template v4",
        "description": "from sync regression",
        "prompt_files": ["AGENT_SAFETY.md"]
    });
    let resp = app
        .oneshot(req(Method::POST, "/api/agent-templates", Some(create_body)))
        .await
        .expect("create agent template");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let template_change = changes
        .iter()
        .find(|change| change.domain == "agent_template" && change.record_id == "sync-template-v4");
    assert!(
        template_change.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["name"] == "sync template v4"
                && change.payload["prompt_files"] == json!(["AGENT_SAFETY.md"])
        }),
        "agent-template POST must append an agent_template v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_template_update_and_delete_emit_agent_template_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_with_agent_ctx(state.clone(), agent_templates::router());
    let template_id = "sync-template-life";

    let create_body = json!({
        "name": "sync template life",
        "description": "from sync lifecycle regression",
        "prompt_files": ["AGENT_SAFETY.md"]
    });
    let resp = app
        .clone()
        .oneshot(req(Method::POST, "/api/agent-templates", Some(create_body)))
        .await
        .expect("create agent template");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let update_body = json!({
        "description": "updated through endpoint",
        "prompt_files": ["AGENT_SAFETY.md"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            &format!("/api/agent-templates/{template_id}"),
            Some(update_body),
        ))
        .await
        .expect("update agent template");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated = changes.iter().find(|change| {
        change.domain == "agent_template"
            && change.record_id == template_id
            && change.kind == "upsert"
            && change.payload["description"] == "updated through endpoint"
            && change.payload["prompt_files"] == json!(["AGENT_SAFETY.md"])
    });
    assert!(
        updated.is_some(),
        "agent-template PUT must append an agent_template v4 upsert; got {changes:#?}"
    );

    let resp = app
        .oneshot(req(
            Method::DELETE,
            &format!("/api/agent-templates/{template_id}"),
            None,
        ))
        .await
        .expect("delete agent template");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted = changes.iter().find(|change| {
        change.domain == "agent_template"
            && change.record_id == template_id
            && change.kind == "delete"
    });
    assert!(
        deleted.is_some(),
        "agent-template DELETE must append an agent_template v4 delete; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_templates_endpoint_full_cycle() {
    let (state, _tmp) = build_state();
    let app = router_no_agent_ctx(state, subagent_templates::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/subagent-templates", None))
        .await
        .expect("list empty");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);

    let create_body = json!({
        "name": "researcher",
        "description": "subagent for research tasks",
        "system_prompt": "you are a researcher",
        "tools": ["search"],
        "visible_to": ["console"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            "/api/subagent-templates",
            Some(create_body),
        ))
        .await
        .expect("create");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = read_json(resp).await;
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"], "researcher");

    let resp = app
        .clone()
        .oneshot(req(
            Method::GET,
            &format!("/api/subagent-templates/{id}"),
            None,
        ))
        .await
        .expect("get");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            Method::DELETE,
            &format!("/api/subagent-templates/{id}"),
            None,
        ))
        .await
        .expect("delete");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_template_create_emits_root_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_no_agent_ctx(state.clone(), subagent_templates::router());

    let create_body = json!({
        "name": "sync-subagent-v4",
        "description": "from sync regression",
        "system_prompt": "you are a synced subagent",
        "tools": ["search"],
        "visible_to": ["console"]
    });
    let resp = app
        .oneshot(req(
            Method::POST,
            "/api/subagent-templates",
            Some(create_body),
        ))
        .await
        .expect("create subagent template");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let changes = collect_v4_changes(&state).await;
    let subagent_change = changes
        .iter()
        .find(|change| change.domain == "root_config" && change.record_id == "subagent_templates");
    assert!(
        subagent_change.is_some_and(|change| {
            change.kind == "upsert"
                && change.payload["templates"]
                    .as_array()
                    .is_some_and(|templates| templates.iter().any(|template| {
                        template["name"] == "sync-subagent-v4"
                            && template["tools"] == json!(["search"])
                    }))
        }),
        "subagent-template POST must append a root_config/subagent_templates v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_template_update_and_delete_emit_root_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_no_agent_ctx(state.clone(), subagent_templates::router());

    let create_body = json!({
        "name": "sync-subagent-life",
        "description": "from sync lifecycle regression",
        "system_prompt": "you are a synced subagent",
        "tools": ["search"],
        "visible_to": ["console"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            "/api/subagent-templates",
            Some(create_body),
        ))
        .await
        .expect("create subagent template");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = read_json(resp).await;
    let template_id = body["id"].as_str().expect("template id").to_string();

    let update_body = json!({
        "name": "sync-subagent-life-updated",
        "tools": ["search", "read"]
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            &format!("/api/subagent-templates/{template_id}"),
            Some(update_body),
        ))
        .await
        .expect("update subagent template");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let updated = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "subagent_templates"
            && change.kind == "upsert"
            && change.payload["templates"]
                .as_array()
                .is_some_and(|templates| {
                    templates.iter().any(|template| {
                        template["id"] == template_id
                            && template["name"] == "sync-subagent-life-updated"
                            && template["tools"] == json!(["search", "read"])
                    })
                })
    });
    assert!(
        updated.is_some(),
        "subagent-template PUT must append a root_config/subagent_templates v4 upsert; got {changes:#?}"
    );

    let resp = app
        .oneshot(req(
            Method::DELETE,
            &format!("/api/subagent-templates/{template_id}"),
            None,
        ))
        .await
        .expect("delete subagent template");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let deleted = changes.iter().find(|change| {
        change.domain == "root_config"
            && change.record_id == "subagent_templates"
            && change.kind == "upsert"
            && change.payload["templates"]
                .as_array()
                .is_some_and(|templates| {
                    !templates
                        .iter()
                        .any(|template| template["id"] == template_id)
                })
    });
    assert!(
        deleted.is_some(),
        "subagent-template DELETE must append root_config/subagent_templates v4 without the deleted template; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_misc_user_timezone_round_trip() {
    let (state, _tmp) = build_state();
    let app = router_with_agent_ctx(state, config_misc::router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/user-timezone", None))
        .await
        .expect("get tz");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(
        body["timezone"]
            .as_str()
            .is_some_and(|timezone| !timezone.is_empty()),
        "missing user timezone should default to the host timezone, got {body:?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/user-timezone",
            Some(json!({ "timezone": "Asia/Shanghai" })),
        ))
        .await
        .expect("put tz");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["timezone"], "Asia/Shanghai");

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/user-timezone", None))
        .await
        .expect("get tz round-trip");
    let body = read_json(resp).await;
    assert_eq!(body["timezone"], "Asia/Shanghai");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_misc_user_timezone_emits_root_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_with_agent_ctx(state.clone(), config_misc::router());

    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/config/user-timezone",
            Some(json!({ "timezone": "Asia/Tokyo" })),
        ))
        .await
        .expect("put timezone");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let timezone_change = changes
        .iter()
        .find(|change| change.domain == "root_config" && change.record_id == "user_timezone");
    assert!(
        timezone_change.is_some_and(|change| {
            change.kind == "upsert" && change.payload == json!("Asia/Tokyo")
        }),
        "user-timezone PUT must append a root_config/user_timezone v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_misc_heartbeat_emits_agent_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_with_agent_ctx(state.clone(), config_misc::router());

    let heartbeat = json!({
        "enabled": true,
        "every": "30m",
        "target": "console",
        "active_hours": {
            "start": "09:00",
            "end": "18:00"
        }
    });
    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/config/heartbeat",
            Some(heartbeat.clone()),
        ))
        .await
        .expect("put heartbeat");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let agent_change = changes
        .iter()
        .find(|change| change.domain == "agent_config" && change.record_id == "default");
    assert!(
        agent_change.is_some_and(|change| {
            change.kind == "upsert" && change.payload["heartbeat"] == heartbeat
        }),
        "heartbeat PUT must append an agent_config v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_llm_routing_emits_agent_config_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_no_agent_ctx(state.clone(), config::agent_scoped_config_router());

    let routing = json!({
        "enabled": true,
        "mode": "cloud_first",
        "local": {"provider_id": "ollama", "model": "qwen2.5:7b"},
        "cloud": {"provider_id": "dashscope", "model": "qwen-plus"}
    });
    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/agents/default/config/agents/llm-routing",
            Some(routing.clone()),
        ))
        .await
        .expect("put llm routing");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let agent_change = changes
        .iter()
        .find(|change| change.domain == "agent_config" && change.record_id == "default");
    assert!(
        agent_change.is_some_and(|change| {
            change.kind == "upsert" && change.payload["llm_routing"] == routing
        }),
        "llm-routing PUT must append an agent_config v4 change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_channels_crud_on_agent_json() {
    let (state, _tmp) = build_state();
    let app = router_with_agent_ctx(state, config_channels::root_router());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/channels", None))
        .await
        .expect("list channels");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let map = body.as_object().expect("map");
    assert!(map.contains_key("console"));
    assert!(map["console"]["isBuiltin"].as_bool().unwrap());

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/channels/types", None))
        .await
        .expect("types");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let types: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(types.contains(&"console"));
    assert!(types.contains(&"dingtalk"));

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/console",
            Some(json!({ "enabled": false })),
        ))
        .await
        .expect("put single channel");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/channels/console", None))
        .await
        .expect("get single channel");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], false);

    let resp = app
        .clone()
        .oneshot(req(
            Method::GET,
            "/api/config/channels/console/qrcode",
            None,
        ))
        .await
        .expect("qrcode");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_channels_update_do_not_emit_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router_with_agent_ctx(state.clone(), config_channels::root_router());

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels",
            Some(json!({
                "console": { "enabled": false },
                "dingtalk": {
                    "enabled": false,
                    "webhook": "https://device-local.example.invalid/hook",
                    "secret": "must-not-sync"
                }
            })),
        ))
        .await
        .expect("put all channels");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/console",
            Some(json!({ "enabled": false, "device_note": "local-only" })),
        ))
        .await
        .expect("put channel");
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "channels are device-local and channel PUTs must not append v4 sync changes; got {changes:#?}"
    );
}
