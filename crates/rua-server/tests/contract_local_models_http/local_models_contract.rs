use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_status_exposes_qwenpaw_dashboard_booleans_and_rust_runtime_fields() {
    let test = build_app();

    let resp = request(&test.app, Method::GET, "/api/local-models/server", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    for key in ["available", "installable", "installed", "running"] {
        assert!(
            body[key].as_bool().is_some(),
            "missing boolean status key: {key}; body={body}"
        );
    }
    assert!(
        body.get("port").is_some(),
        "port key should be present even when null"
    );
    assert!(
        body.get("model_name").is_some(),
        "model_name key should be present even when null"
    );
    assert!(
        body.get("message").is_some(),
        "message key should be present even when null"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_status_treats_workspace_llama_binary_as_installed() {
    let test = build_app();
    let server_dir = test.local_model_dir.join("server");
    std::fs::create_dir_all(&server_dir).expect("server dir");
    let binary_name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    std::fs::write(server_dir.join(binary_name), b"fake binary").expect("server binary");

    let resp = request(&test.app, Method::GET, "/api/local-models/server", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["installed"], true);
    assert_eq!(body["installable"], true);
    assert_eq!(body["available"], false);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("not running")),
        "installed-but-stopped runtime should report not-running, got {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_lists_return_arrays_with_stable_recommended_shape() {
    let test = build_app();

    let models_resp = request(&test.app, Method::GET, "/api/local-models/models", None).await;
    assert_eq!(models_resp.status(), StatusCode::OK);
    assert!(read_json(models_resp).await.as_array().is_some());

    let recommended_resp = request(
        &test.app,
        Method::GET,
        "/api/local-models/recommended",
        None,
    )
    .await;
    assert_eq!(recommended_resp.status(), StatusCode::OK);
    let recommended = read_json(recommended_resp).await;
    let recommended = recommended.as_array().expect("recommended models array");
    assert!(
        !recommended.is_empty(),
        "recommended model catalog should not be empty"
    );
    let mut ids = std::collections::BTreeSet::new();
    for model in recommended {
        let id = model["id"].as_str().expect("recommended id");
        assert!(!id.trim().is_empty());
        assert!(
            ids.insert(id.to_string()),
            "recommended ids must be unique: {recommended:?}"
        );
        assert!(model["name"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(model["size_bytes"].as_u64().is_some());
        assert!(model["downloaded"].as_bool().is_some());
        assert!(model["source"]
            .as_str()
            .is_some_and(|source| !source.is_empty()));
    }
    if let Some(first) = recommended.first() {
        assert!(first["id"].as_str().is_some());
        assert!(first["name"].as_str().is_some());
        assert!(first["size_bytes"].as_u64().is_some());
        assert!(first["downloaded"].as_bool().is_some());
        assert!(first["source"].as_str().is_some());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_unknown_model_returns_fastapi_style_404() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::DELETE,
        "/api/local-models/models/contract-unknown-model",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "expected non-empty detail body, got {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_server_without_running_process_returns_conflict_detail() {
    let test = build_app();

    let resp = request(&test.app, Method::DELETE, "/api/local-models/server", None).await;

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("server not running")),
        "expected server-not-running detail, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_server_with_unknown_model_id_returns_not_found_detail() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::POST,
        "/api/local-models/server",
        Some(json!({ "model_id": "missing-model", "port": 0 })),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("local model not found")),
        "expected local-model missing detail, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_progress_and_cancel_without_active_download_return_404_details() {
    let test = build_app();

    for (method, path, expected) in [
        (
            Method::GET,
            "/api/local-models/models/download",
            "model download",
        ),
        (
            Method::DELETE,
            "/api/local-models/models/download",
            "model download",
        ),
        (
            Method::GET,
            "/api/local-models/server/download",
            "server download",
        ),
        (
            Method::DELETE,
            "/api/local-models/server/download",
            "server download",
        ),
    ] {
        let resp = request(&test.app, method, path, None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
        let body = read_json(resp).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected)),
            "expected {expected:?} detail for {path}, got {body:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_download_start_returns_stable_progress_shape_and_can_cancel() {
    let test = build_app();
    let model_id = "http://127.0.0.1:1/rua-contract-model.gguf";

    let start = request(
        &test.app,
        Method::POST,
        "/api/local-models/models/download",
        Some(json!({ "model_id": model_id })),
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    let body = read_json(start).await;
    assert_eq!(body["model_id"], model_id);
    assert_eq!(body["model_name"], model_id);
    assert_eq!(body["status"], "downloading");
    assert_eq!(body["progress"], 0.0);
    assert_eq!(body["downloaded_bytes"], 0);
    assert!(body["total_bytes"].is_null());
    assert_eq!(body["speed_bytes_per_sec"], 0);
    assert_eq!(body["source"], "auto");
    assert!(body["error"].is_null());
    assert!(body["local_path"].is_null());

    let progress = request(
        &test.app,
        Method::GET,
        "/api/local-models/models/download",
        None,
    )
    .await;
    assert_eq!(progress.status(), StatusCode::OK);
    let progress_body = read_json(progress).await;
    assert_eq!(progress_body["model_id"], model_id);
    assert_eq!(progress_body["model_name"], model_id);
    assert!(progress_body["status"].as_str().is_some());
    assert!(progress_body.get("progress").is_some());
    assert!(progress_body["downloaded_bytes"].as_u64().is_some());
    assert!(progress_body.get("total_bytes").is_some());
    assert!(progress_body["speed_bytes_per_sec"].as_u64().is_some());
    assert_eq!(progress_body["source"], "auto");
    assert!(progress_body.get("error").is_some());
    assert!(progress_body.get("local_path").is_some());

    let cancel = request(
        &test.app,
        Method::DELETE,
        "/api/local-models/models/download",
        None,
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(read_json(cancel).await["success"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_download_respects_explicit_source_field_in_start_response() {
    let test = build_app();
    let model_id = "modelscope:contract/repo";

    let start = request(
        &test.app,
        Method::POST,
        "/api/local-models/models/download",
        Some(json!({ "model_id": model_id, "source": "modelscope" })),
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    let body = read_json(start).await;
    assert_eq!(body["model_id"], model_id);
    assert_eq!(body["model_name"], model_id);
    assert_eq!(body["status"], "downloading");
    assert_eq!(body["source"], "modelscope");

    let cancel = request(
        &test.app,
        Method::DELETE,
        "/api/local-models/models/download",
        None,
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_download_start_returns_same_progress_dto_shape_and_can_cancel() {
    let test = build_app();

    let start = request(
        &test.app,
        Method::POST,
        "/api/local-models/server/download",
        Some(json!({ "model_id": "llama-server" })),
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    let body = read_json(start).await;
    assert_eq!(body["model_id"], "llama-server");
    assert_eq!(body["model_name"], "llama-server");
    assert_eq!(body["status"], "downloading");
    assert_eq!(body["progress"], 0.0);
    assert_eq!(body["downloaded_bytes"], 0);
    assert!(body["total_bytes"].is_null());
    assert_eq!(body["speed_bytes_per_sec"], 0);
    assert_eq!(body["source"], "auto");
    assert!(body["error"].is_null());
    assert!(body["local_path"].is_null());

    let progress = request(
        &test.app,
        Method::GET,
        "/api/local-models/server/download",
        None,
    )
    .await;
    assert_eq!(progress.status(), StatusCode::OK);
    let progress_body = read_json(progress).await;
    assert_eq!(progress_body["model_id"], "llama-server");
    assert_eq!(progress_body["model_name"], "llama-server");
    assert!(progress_body["downloaded_bytes"].as_u64().is_some());
    assert!(progress_body.get("total_bytes").is_some());
    assert!(progress_body["speed_bytes_per_sec"].as_u64().is_some());
    assert_eq!(progress_body["source"], "auto");
    assert!(progress_body.get("local_path").is_some());

    let cancel = request(
        &test.app,
        Method::DELETE,
        "/api/local-models/server/download",
        None,
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(read_json(cancel).await["success"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_model_route_treats_path_like_model_ids_as_ids_not_filesystem_paths() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::DELETE,
        "/api/local-models/models/..%2Foutside",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("local model not found")),
        "expected local-model missing detail, got {body:?}"
    );
}
