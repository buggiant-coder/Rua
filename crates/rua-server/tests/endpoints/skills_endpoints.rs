use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::skills::{agent_scoped_router, shared_router};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(tmp.path().join("workspaces/default/skills"))
        .expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", shared_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

#[tokio::test]
async fn identical_skill_save_reports_no_effective_change() {
    let tmp = TempDir::new().unwrap();
    let content = "---\nname: noop-fixture\ndescription: Fixture\n---\n\nOriginal content\n";
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let create = serde_json::from_value(serde_json::json!({
        "name":"noop-fixture", "description":"Fixture", "content":content, "enable":false
    }))
    .unwrap();
    let created = state
        .skill_service
        .create_skill(Some("default"), create)
        .await
        .unwrap();
    let content = created.content;
    let request: rua_server::endpoints::dto::SkillSaveRequest =
        serde_json::from_value(serde_json::json!({
            "name":"noop-fixture", "content":content
        }))
        .unwrap();
    let (_, changed) = state
        .skill_service
        .save_skill(Some("default"), request)
        .await
        .unwrap();
    assert!(
        !changed,
        "unchanged Skill content must not invalidate prepared state"
    );
    let (_, changed) = state
        .skill_service
        .toggle_skill(Some("default"), "noop-fixture", false)
        .await
        .unwrap();
    assert!(
        !changed,
        "already disabled Skill must not invalidate prepared state"
    );
    let (_, changed) = state
        .skill_service
        .toggle_skill(Some("default"), "noop-fixture", true)
        .await
        .unwrap();
    assert!(changed);
    let (_, changed) = state
        .skill_service
        .toggle_skill(Some("default"), "noop-fixture", true)
        .await
        .unwrap();
    assert!(
        !changed,
        "already enabled Skill must not invalidate prepared state"
    );
}

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default/skills"))
        .expect("seed default workspace");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be enabled for skill v4 emit regression test"
    );

    let app = Router::new()
        .nest("/api", shared_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state, tmp)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default");
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
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

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nskill body for {name}\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_list_starts_empty() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(req("GET", "/api/agents/default/skills", None))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.is_array(), "GET /skills should return array");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_skill_lifecycle_create_enable_channels_config_delete() {
    let (app, _tmp) = build_app();

    let create_body = serde_json::json!({
        "name": "demo_skill",
        "description": "demo skill",
        "content": skill_md("demo_skill", "demo skill"),
        "enable": false,
    });
    let resp = app
        .clone()
        .oneshot(req("POST", "/api/agents/default/skills", Some(create_body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/agents/default/skills/demo_skill", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = read_json(resp).await;
    assert_eq!(detail["name"].as_str(), Some("demo_skill"));
    assert_eq!(detail["enabled"].as_bool(), Some(false));

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/agents/default/skills/demo_skill/enable",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/agents/default/skills/demo_skill/channels",
            Some(serde_json::json!({"channels": ["console"]})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/agents/default/skills/demo_skill/config",
            Some(serde_json::json!({"config": {"foo": "bar"}})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/agents/default/skills/demo_skill/config",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cfg = read_json(resp).await;
    let inner = cfg.get("config").unwrap();
    assert_eq!(inner.get("foo").and_then(|v| v.as_str()), Some("bar"));

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/agents/default/skills/demo_skill/disable",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/agents/default/skills/demo_skill", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_skill_lifecycle_emits_skill_blob_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let skill_name = "sync_workspace_skill";

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/agents/default/skills",
            Some(serde_json::json!({
                "name": skill_name,
                "description": "workspace sync skill",
                "content": skill_md(skill_name, "workspace sync skill"),
                "enable": false,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/agents/default/skills/{skill_name}/config"),
            Some(serde_json::json!({"config": {"mode": "v4"}})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let upsert = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == format!("workspace/default/{skill_name}")
            && change.kind == "upsert"
            && change.payload["scope"] == "workspace"
            && change.payload["agent_id"] == "default"
            && change.payload["skill_name"] == skill_name
    });
    assert!(
        upsert.is_some(),
        "workspace skill create/update must expose a current skill_blob_manifest v4 upsert before delete; got {changes:#?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/agents/default/skills/{skill_name}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let delete = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == format!("workspace/default/{skill_name}")
            && change.kind == "delete"
    });
    assert!(
        delete.is_some(),
        "workspace skill DELETE must append a skill_blob_manifest v4 tombstone; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_enabled_skill_returns_409() {
    let (app, tmp) = build_app();
    let create_body = serde_json::json!({
        "name": "guard_skill",
        "description": "guard",
        "content": skill_md("guard_skill", "guard"),
        "enable": true,
    });
    let resp = app
        .clone()
        .oneshot(req("POST", "/api/agents/default/skills", Some(create_body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let manifest_path = tmp.path().join("workspaces/default/skill.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let payload: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        payload["skills"]["guard_skill"]["enabled"].as_bool(),
        Some(true),
        "create_skill with enable=true should mark manifest enabled"
    );

    let svc = rua_skills::SkillService::new(tmp.path().join("workspaces/default")).unwrap();
    let err = svc
        .delete_skill("guard_skill")
        .expect_err("delete enabled skill must error");
    assert!(matches!(err, rua_skills::SkillError::Conflict(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_save_skill_edit_in_place() {
    let (app, _tmp) = build_app();

    let _ = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/agents/default/skills",
            Some(serde_json::json!({
                "name": "to_save",
                "description": "before",
                "content": skill_md("to_save", "before"),
                "enable": false,
            })),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/agents/default/skills/save",
            Some(serde_json::json!({
                "name": "to_save",
                "content": skill_md("to_save", "after"),
                "overwrite": false,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_listing_endpoints_smoke() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/skills/workspaces", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.is_array());

    let resp = app
        .clone()
        .oneshot(req("POST", "/api/agents/default/skills/refresh", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_lifecycle_create_config_delete() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/skills/pool", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let pool = body.as_array().expect("pool list returns array");
    assert!(
        !pool.is_empty(),
        "pool should contain at least one builtin entry, got {}",
        pool.len()
    );

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/skills/pool/create",
            Some(serde_json::json!({
                "name": "pool_demo",
                "description": "pool demo",
                "content": skill_md("pool_demo", "pool demo"),
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/skills/pool/pool_demo/tags",
            Some(serde_json::json!({"tags": ["alpha"]})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/skills/pool/pool_demo/config",
            Some(serde_json::json!({"config": {"k": "v"}})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("GET", "/api/skills/pool/pool_demo/config", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cfg = read_json(resp).await;
    assert_eq!(
        cfg["config"]["k"].as_str(),
        Some("v"),
        "pool_get_config should round-trip the just-written value"
    );

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/skills/pool/pool_demo/config", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req("DELETE", "/api/skills/pool/pool_demo", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_skill_lifecycle_emits_skill_blob_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let skill_name = "sync_pool_skill";

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/skills/pool/create",
            Some(serde_json::json!({
                "name": skill_name,
                "description": "pool sync skill",
                "content": skill_md(skill_name, "pool sync skill"),
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/api/skills/pool/{skill_name}/config"),
            Some(serde_json::json!({"config": {"mode": "v4"}})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let upsert = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == skill_name
            && change.kind == "upsert"
            && change.payload["scope"] == "pool"
            && change.payload["skill_name"] == skill_name
    });
    assert!(
        upsert.is_some(),
        "pool skill create/update must expose a current skill_blob_manifest v4 upsert before delete; got {changes:#?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/skills/pool/{skill_name}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let delete = changes.iter().find(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == skill_name
            && change.kind == "delete"
    });
    assert!(
        delete.is_some(),
        "pool skill DELETE must append a skill_blob_manifest v4 tombstone; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_delete_missing_returns_404() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(req("DELETE", "/api/skills/pool/this_does_not_exist", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_get_config_missing_returns_404() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(req(
            "GET",
            "/api/skills/pool/this_does_not_exist/config",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_builtin_sources_returns_full_set() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(req("GET", "/api/skills/pool/builtin-sources", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let sources = body.as_array().expect("sources is an array");

    assert!(
        !sources.is_empty(),
        "pool/builtin-sources should report packaged builtins, got {}",
        sources.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_builtin_notice_returns_object() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(req("GET", "/api/skills/pool/builtin-notice", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.get("fingerprint").is_some());
    assert!(body.get("has_updates").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_import_builtin_with_explicit_names() {
    let (app, _tmp) = build_app();

    let names = rua_skills::builtin_skill_names();
    if names.is_empty() {
        return;
    }
    let target = &names[0];
    let resp = app
        .oneshot(req(
            "POST",
            "/api/skills/pool/import-builtin",
            Some(serde_json::json!({
                "skill_names": [target],
                "overwrite_conflicts": true,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(skills_pool)]
async fn pool_update_single_builtin_smoke() {
    let (app, _tmp) = build_app();
    let names = rua_skills::builtin_skill_names();
    if names.is_empty() {
        return;
    }
    let target = &names[0];
    let resp = app
        .oneshot(req(
            "POST",
            &format!("/api/skills/pool/{target}/update-builtin"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
