#![allow(deprecated)]

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rua_server::endpoints::skills;
use rua_server::endpoints::skills_hub::{hub_search_handler, register_hub_routes};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_test_app(state: AppState) -> Router {
    let hub_router: Router<AppState> = Router::new()
        .route("/skills/hub/search", get(hub_search_handler))
        .merge(register_hub_routes());

    Router::new()
        .nest("/api", hub_router)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

fn build_full_skills_app(state: AppState) -> Router {
    Router::new()
        .nest("/api", skills::shared_router())
        .nest("/api", register_hub_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body is json")
}

fn shorten_retry_env() {
    std::env::set_var("RUA_SKILLS_HUB_HTTP_RETRIES", "1");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE", "0.05");
    std::env::set_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP", "0.2");
}

fn restore_env() {
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_RETRIES");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_BASE");
    std::env::remove_var("RUA_SKILLS_HUB_HTTP_BACKOFF_CAP");
    std::env::remove_var("RUA_SKILLS_HUB_BASE_URL");
    std::env::remove_var("RUA_SKILLS_HUB_MODELSCOPE_BASE");
    std::env::remove_var("RUA_SKILLS_HUB_SKILLHUB_BASE");
    std::env::remove_var("RUA_SKILLS_HUB_ALIYUN_BASE");
}

struct EnvGuard;
impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn search_returns_normalized_results() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "slug": "demo",
                "name": "Demo",
                "description": "desc",
                "version": "1.0.0",
                "url": "https://clawhub.ai/demo"
            }
        ])))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, _tmp) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/skills/hub/search?q=demo&limit=5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body[0]["slug"], "demo");
    assert_eq!(body[0]["name"], "Demo");
    assert_eq!(body[0]["source_url"], "https://clawhub.ai/demo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn search_invalid_json_maps_to_422() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, _tmp) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/skills/hub/search?q=x")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

async fn mount_clawhub_demo(server: &MockServer) {
    let detail = json!({
        "skill": {"slug": "demo", "displayName": "Demo Skill"},
        "version": {
            "version": "1.0.0",
            "files": [
                {"path": "SKILL.md"},
                {"path": "references/notes.md"}
            ]
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail))
        .mount(server)
        .await;

    let skill_md = "---\nname: demo-skill\ndescription: a demo\n---\nbody";
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/demo/file"))
        .respond_with(ResponseTemplate::new(200).set_body_string(skill_md))
        .mount(server)
        .await;
}

async fn mount_modelscope_demo(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/@owner/modelscope-demo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Data": {
                "Name": "modelscope-demo",
                "ReadMeContent": "---\nname: modelscope-demo\ndescription: from modelscope\n---\nbody"
            }
        })))
        .mount(server)
        .await;
}

async fn mount_skillhub_demo(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "skillhub-demo",
            "files": {
                "SKILL.md": "---\nname: skillhub-demo\ndescription: from skillhub\n---\nbody",
                "references": {"notes.md": "reference"}
            }
        })))
        .mount(server)
        .await;
}

async fn mount_aliyun_demo(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/agentexplorer/skills/aliyun-demo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "skillName": "aliyun-demo",
                "files": {
                    "SKILL.md": "---\nname: aliyun-demo\ndescription: from aliyun\n---\nbody",
                    "scripts": {"run.sh": "echo ok"}
                }
            }
        })))
        .mount(server)
        .await;
}

async fn assert_installs_to_workspace_and_pool(
    app: Router,
    tmp: &TempDir,
    source_url: &str,
    target_name: &str,
) {
    let workspace_body = json!({
        "url": source_url,
        "target_name": target_name,
        "enable": true
    });
    let workspace_req = Request::builder()
        .method("POST")
        .uri("/api/agents/default/skills/hub/install")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(workspace_body.to_string()))
        .unwrap();
    let workspace_resp = app.clone().oneshot(workspace_req).await.expect("oneshot");
    let workspace_status = workspace_resp.status();
    let workspace_json = read_json(workspace_resp).await;
    assert_eq!(
        workspace_status,
        StatusCode::OK,
        "body was {workspace_json}"
    );
    assert_eq!(workspace_json["installed"], true);
    assert_eq!(workspace_json["enabled"], true);
    assert_eq!(workspace_json["name"], target_name);
    assert!(tmp
        .path()
        .join("workspaces/default/skills")
        .join(target_name)
        .join("SKILL.md")
        .exists());

    let pool_name = format!("{target_name}-pool");
    let pool_body = json!({
        "bundle_url": source_url,
        "target_name": pool_name,
    });
    let pool_req = Request::builder()
        .method("POST")
        .uri("/api/skills/pool/import")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(pool_body.to_string()))
        .unwrap();
    let pool_resp = app.oneshot(pool_req).await.expect("oneshot");
    let pool_status = pool_resp.status();
    let pool_json = read_json(pool_resp).await;
    assert_eq!(pool_status, StatusCode::OK, "body was {pool_json}");
    assert_eq!(pool_json["installed"], true);
    assert_eq!(pool_json["enabled"], false);
    assert_eq!(pool_json["name"], pool_name);
    assert!(tmp
        .path()
        .join("skill_pool")
        .join(pool_name)
        .join("SKILL.md")
        .exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn install_creates_workspace_skill_and_returns_metadata() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    mount_clawhub_demo(&server).await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, tmp) = build_state();
    let app = build_test_app(state);

    let body = json!({"url": "https://clawhub.ai/demo", "version": "1.0.0", "enable": false});
    let req = Request::builder()
        .method("POST")
        .uri("/api/agents/default/skills/hub/install")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_json(resp).await;
    assert_eq!(json["installed"], true);
    assert_eq!(json["enabled"], false);

    let source_url = json["source_url"].as_str().expect("source_url");
    assert!(
        source_url.contains("/api/v1/skills/demo"),
        "expected fetched URL, got {source_url}"
    );
    let installed_name = json["name"].as_str().expect("name").to_string();

    let workspace_root = tmp.path().join("workspaces").join("default");
    let candidates = [
        workspace_root.join("skills").join(&installed_name),
        workspace_root.join("skill").join(&installed_name),
    ];
    let exists = candidates.iter().any(|p| p.join("SKILL.md").exists());
    assert!(
        exists,
        "expected SKILL.md under workspace skill dir; checked {candidates:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn install_pool_writes_into_pool_dir() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    mount_clawhub_demo(&server).await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, _tmp) = build_state();
    let pool_root = state.rua_root.join("skill_pool");
    let app = build_test_app(state);

    let body = json!({"url": "https://clawhub.ai/demo", "version": "1.0.0"});
    let req = Request::builder()
        .method("POST")
        .uri("/api/skills/hub/install_pool")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let json = read_json(resp).await;
    assert_eq!(status, StatusCode::OK, "body was {json}");
    assert_eq!(json["installed"], true);
    let installed_name = json["name"].as_str().expect("name").to_string();

    let pool_skill_md = pool_root.join(&installed_name).join("SKILL.md");
    assert!(
        pool_skill_md.exists(),
        "missing {}",
        pool_skill_md.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn pool_import_accepts_frontend_bundle_url_payload() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    mount_clawhub_demo(&server).await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, tmp) = build_state();
    let app = build_full_skills_app(state);

    let body = json!({
        "bundle_url": "https://clawhub.ai/demo",
        "version": "1.0.0",
        "target_name": "skill-creator"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/skills/pool/import")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let json = read_json(resp).await;

    assert_eq!(status, StatusCode::OK, "body was {json}");
    assert_eq!(json["installed"], true);
    assert_eq!(json["name"], "skill-creator");
    assert_eq!(json["enabled"], false);
    assert!(json["source_url"]
        .as_str()
        .unwrap_or_default()
        .contains("/api/v1/skills/demo"));

    let pool_skill_md = tmp
        .path()
        .join("skill_pool")
        .join("skill-creator")
        .join("SKILL.md");
    assert!(
        pool_skill_md.exists(),
        "missing {}",
        pool_skill_md.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn market_sources_install_to_workspace_and_pool() {
    shorten_retry_env();
    let _guard = EnvGuard;
    let _router = rua_skills_hub::lib_ext::install_default_router();

    let clawhub = MockServer::start().await;
    mount_clawhub_demo(&clawhub).await;
    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", clawhub.uri());

    let modelscope = MockServer::start().await;
    mount_modelscope_demo(&modelscope).await;
    std::env::set_var("RUA_SKILLS_HUB_MODELSCOPE_BASE", modelscope.uri());

    let skillhub = MockServer::start().await;
    mount_skillhub_demo(&skillhub).await;
    std::env::set_var("RUA_SKILLS_HUB_SKILLHUB_BASE", skillhub.uri());

    let aliyun = MockServer::start().await;
    mount_aliyun_demo(&aliyun).await;
    std::env::set_var("RUA_SKILLS_HUB_ALIYUN_BASE", aliyun.uri());

    let (state, tmp) = build_state();
    let app = build_full_skills_app(state);

    assert_installs_to_workspace_and_pool(
        app.clone(),
        &tmp,
        "https://clawhub.ai/demo",
        "clawhub-installed",
    )
    .await;
    assert_installs_to_workspace_and_pool(
        app.clone(),
        &tmp,
        "https://modelscope.cn/skills/@owner/modelscope-demo",
        "modelscope-installed",
    )
    .await;
    assert_installs_to_workspace_and_pool(
        app.clone(),
        &tmp,
        "https://api.skillhub.cn/owner/skillhub-demo",
        "skillhub-installed",
    )
    .await;
    assert_installs_to_workspace_and_pool(
        app,
        &tmp,
        "https://api.aliyun.com/agentexplorer/skills/aliyun-demo",
        "aliyun-installed",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn install_unsupported_source_maps_to_400() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let (state, _tmp) = build_state();
    let app = build_test_app(state);

    let body = json!({"url": "https://skills.sh/owner/repo/skill"});
    let req = Request::builder()
        .method("POST")
        .uri("/api/agents/default/skills/hub/install")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn install_http_error_passes_through() {
    shorten_retry_env();
    let _guard = EnvGuard;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    std::env::set_var("RUA_SKILLS_HUB_BASE_URL", server.uri());

    let (state, _tmp) = build_state();
    let app = build_test_app(state);

    let body = json!({"url": "https://clawhub.ai/missing"});
    let req = Request::builder()
        .method("POST")
        .uri("/api/agents/default/skills/hub/install")
        .header("content-type", "application/json")
        .header("X-Agent-Id", "default")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn register_hub_routes_omits_search() {
    let _guard = EnvGuard;

    let (state, _tmp) = build_state();

    let app = Router::new()
        .nest("/api", register_hub_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/skills/hub/search?q=anything")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
