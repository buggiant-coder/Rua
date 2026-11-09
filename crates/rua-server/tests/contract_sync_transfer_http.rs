use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::sync_transfer::router as sync_transfer_router;
use rua_server::{AppState, ServerProfile};

fn build_app(sync_enabled: bool) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    if sync_enabled {
        let sync_dir = tmp.path().join("sync");
        std::fs::create_dir_all(&sync_dir).expect("sync dir");
        std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
        rua_sync::identity::NetworkSecret::generate()
            .save(&sync_dir)
            .expect("network secret");
    }

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", sync_transfer_router())
        .with_state(state.clone());
    (app, tmp, state)
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize body"))
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response JSON")
}

async fn wait_for_job_status(app: &Router, job_id: &str, status: &str) -> Value {
    for _ in 0..20 {
        let resp = request(
            app,
            Method::GET,
            &format!("/api/sync/transfer/jobs/{job_id}"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_json(resp).await;
        if body["status"] == status {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("job {job_id} did not reach status {status}");
}

fn local_device_id(state: &AppState) -> String {
    state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("local sync device")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_directory_returns_local_files_and_directories() {
    let (app, tmp, state) = build_app(true);
    let root = tmp.path().join("downloads");
    std::fs::create_dir_all(root.join("Design")).expect("create dir");
    std::fs::write(root.join("report.pdf"), b"report").expect("write file");
    let device_id = local_device_id(&state);

    let resp = request(
        &app,
        Method::POST,
        "/api/sync/transfer/list",
        Some(json!({
            "device_id": device_id,
            "path": root,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["device_id"], local_device_id(&state));
    assert_eq!(
        body["path"],
        root.canonicalize()
            .expect("canonical root")
            .to_string_lossy()
            .to_string()
    );
    assert_eq!(body["entries"][0]["name"], "Design");
    assert_eq!(body["entries"][0]["kind"], "directory");
    assert_eq!(body["entries"][1]["name"], "report.pdf");
    assert_eq!(body["entries"][1]["kind"], "file");
    assert_eq!(body["entries"][1]["size"], 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_directory_requires_sync_enabled() {
    let (app, tmp, state) = build_app(false);
    let resp = request(
        &app,
        Method::POST,
        "/api/sync/transfer/list",
        Some(json!({
            "device_id": local_device_id(&state),
            "path": tmp.path(),
        })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert_eq!(body["reason"], "not_enabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_transfer_local_source_to_local_target_writes_files_and_tracks_job() {
    let (app, tmp, state) = build_app(true);
    let source_dir = tmp.path().join("source");
    let target_dir = tmp.path().join("target");
    std::fs::create_dir_all(&source_dir).expect("source dir");
    std::fs::create_dir_all(&target_dir).expect("target dir");
    std::fs::write(source_dir.join("report.pdf"), b"report").expect("report");
    let device_id = local_device_id(&state);

    let resp = request(
        &app,
        Method::POST,
        "/api/sync/transfer/send",
        Some(json!({
            "source_device_id": device_id,
            "source_paths": [source_dir.join("report.pdf")],
            "target_device_id": local_device_id(&state),
            "target_dir": target_dir,
            "conflict": "rename"
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let started = read_json(resp).await;
    assert_eq!(started["status"], "running");
    let job_id = started["job_id"].as_str().expect("job id");

    let completed = wait_for_job_status(&app, job_id, "succeeded").await;
    assert_eq!(completed["files_written"], 1);
    assert_eq!(completed["bytes_written"], 6);
    assert_eq!(
        std::fs::read(target_dir.join("report.pdf")).expect("target file"),
        b"report"
    );

    let list = request(&app, Method::GET, "/api/sync/transfer/jobs", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let jobs = read_json(list).await;
    assert_eq!(jobs.as_array().expect("jobs").len(), 1);
    assert_eq!(jobs[0]["job_id"], job_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_job_get_and_cancel_unknown_return_404() {
    let (app, _tmp, _state) = build_app(true);

    let get = request(
        &app,
        Method::GET,
        "/api/sync/transfer/jobs/missing-job",
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::NOT_FOUND);

    let cancel = request(
        &app,
        Method::POST,
        "/api/sync/transfer/jobs/missing-job/cancel",
        None,
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::NOT_FOUND);
}
