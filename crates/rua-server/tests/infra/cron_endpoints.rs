use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::cron;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", cron::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_endpoints_full_crud_cycle() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/cron/jobs", None))
        .await
        .expect("list empty");
    assert_eq!(resp.status(), StatusCode::OK, "GET /cron/jobs (empty)");
    let body = read_json(resp).await;
    assert!(body.is_array(), "list response must be JSON array");
    assert_eq!(
        body.as_array().unwrap().len(),
        0,
        "expected 0 jobs initially"
    );

    let create_body = json!({
        "name": "feat-704 smoke",
        "schedule": { "type": "cron", "cron": "0 * * * *", "timezone": "UTC" },
        "task_type": "text",
        "text": "hello from feat-704",
        "dispatch": { "type": "channel", "channel": "console", "mode": "stream" }
    });
    let resp = app
        .clone()
        .oneshot(req(Method::POST, "/api/cron/jobs", Some(create_body)))
        .await
        .expect("create");
    assert_eq!(resp.status(), StatusCode::CREATED, "POST /cron/jobs");
    let created = read_json(resp).await;
    let job_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["name"], "feat-704 smoke");
    assert_eq!(created["schedule"]["cron"], "0 * * * *");

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/cron/jobs", None))
        .await
        .expect("list after create");
    let body = read_json(resp).await;
    assert_eq!(
        body.as_array().unwrap().len(),
        1,
        "expected 1 job after create"
    );

    let update_body = json!({
        "name": "feat-704 smoke renamed",
        "schedule": { "type": "cron", "cron": "*/15 * * * *", "timezone": "Asia/Shanghai" },
        "enabled": false
    });
    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            &format!("/api/cron/jobs/{job_id}"),
            Some(update_body),
        ))
        .await
        .expect("update");
    assert_eq!(resp.status(), StatusCode::OK, "PUT /cron/jobs/{{id}}");
    let updated = read_json(resp).await;
    assert_eq!(updated["name"], "feat-704 smoke renamed");
    assert_eq!(updated["schedule"]["cron"], "*/15 * * * *");
    assert_eq!(updated["schedule"]["timezone"], "Asia/Shanghai");
    assert_eq!(updated["enabled"], false);

    let resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            &format!("/api/cron/jobs/{job_id}/run"),
            None,
        ))
        .await
        .expect("run");
    let status = resp.status();
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "run must not 404 — job exists"
    );
    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "run must not 400 — job has a text body"
    );
    assert!(
        status == StatusCode::OK || status.is_server_error(),
        "run returned unexpected status {status}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            Method::DELETE,
            &format!("/api/cron/jobs/{job_id}"),
            None,
        ))
        .await
        .expect("delete");
    assert_eq!(resp.status(), StatusCode::OK, "DELETE /cron/jobs/{{id}}");
    let body = read_json(resp).await;
    assert_eq!(body["success"], true);

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/cron/jobs", None))
        .await
        .expect("list final");
    let body = read_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0, "list empty after delete");

    let resp = app
        .clone()
        .oneshot(req(
            Method::DELETE,
            &format!("/api/cron/jobs/{job_id}"),
            None,
        ))
        .await
        .expect("delete twice");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "second delete is 404");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_create_writes_jobs_json_in_workspace() {
    let (app, tmp) = build_app();

    let create_body = json!({
        "name": "persist-check",
        "schedule": { "type": "cron", "cron": "@daily", "timezone": "UTC" },
        "task_type": "text",
        "text": "hi"
    });
    let resp = app
        .clone()
        .oneshot(req(Method::POST, "/api/cron/jobs", Some(create_body)))
        .await
        .expect("create");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let jobs_path = tmp.path().join("workspaces/default/jobs.json");
    assert!(
        jobs_path.exists(),
        "expected jobs.json at {} after create",
        jobs_path.display()
    );
    let raw = std::fs::read_to_string(&jobs_path).expect("read jobs.json");
    let parsed: Value = serde_json::from_str(&raw).expect("parse jobs.json");
    assert_eq!(parsed["version"], 1, "jobs.json keeps version=1");
    let jobs = parsed["jobs"]
        .as_object()
        .expect("jobs.json stores jobs as an id-keyed object");
    let job = jobs.values().next().expect("created job persisted");
    assert_eq!(
        job["name"], "persist-check",
        "job persisted to disk via CronCells"
    );
}
