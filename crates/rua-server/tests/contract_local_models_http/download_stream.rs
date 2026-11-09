use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use rua_local_model::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
use tokio::sync::broadcast;
use tower::ServiceExt;

use super::support::{build_app, read_json};

async fn read_json_lines(response: axum::response::Response, expected_lines: usize) -> Vec<String> {
    let mut body = response.into_body();
    let mut buf = Vec::new();
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
            .await
            .expect("stream should yield a frame")
            .expect("stream ended before yielding expected frames")
            .expect("frame should be readable");
        if let Some(data) = frame.data_ref() {
            buf.extend_from_slice(data);
        }
        let text = String::from_utf8_lossy(&buf);
        if text.lines().count() >= expected_lines {
            return text.lines().map(str::to_string).collect();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_stream_returns_chunked_json_lines() {
    let test = build_app();
    let (tx, _) = broadcast::channel::<IpcMessage>(16);
    test.state
        .local_model
        .register_download_sender("test-task-1".into(), tx.clone())
        .await;

    let app = test.app.clone();
    let handle = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/download/stream?task_id=test-task-1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response")
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.send(IpcMessage::Progress(ProgressUpdate {
        id: "model-abc".into(),
        bytes_downloaded: 512,
        bytes_total: 1024,
        eta_secs: None,
    }))
    .expect("send progress");
    drop(tx);

    let response = handle.await.expect("join stream request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );

    let lines = read_json_lines(response, 1).await;
    assert_eq!(lines.len(), 1);
    let msg: IpcMessage = serde_json::from_str(&lines[0]).expect("valid IpcMessage JSON");
    match msg {
        IpcMessage::Progress(progress) => {
            assert_eq!(progress.id, "model-abc");
            assert_eq!(progress.bytes_downloaded, 512);
            assert_eq!(progress.bytes_total, 1024);
        }
        other => panic!("expected Progress variant, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_stream_forwards_status_and_error_json_lines() {
    let test = build_app();
    let (tx, _) = broadcast::channel::<IpcMessage>(16);
    test.state
        .local_model
        .register_download_sender("test-task-terminal".into(), tx.clone())
        .await;

    let app = test.app.clone();
    let handle = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/download/stream?task_id=test-task-terminal")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response")
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.send(IpcMessage::Status(StatusUpdate {
        id: "model-terminal".into(),
        status: DownloadStatus::Started,
        detail: None,
    }))
    .expect("send started");
    tx.send(IpcMessage::Error(ErrorUpdate {
        id: "model-terminal".into(),
        error: "network timeout".into(),
        retryable: Some(true),
    }))
    .expect("send error");
    drop(tx);

    let response = handle.await.expect("join stream request");
    assert_eq!(response.status(), StatusCode::OK);

    let lines = read_json_lines(response, 2).await;
    assert_eq!(lines.len(), 2);

    let status: IpcMessage = serde_json::from_str(&lines[0]).expect("status json");
    assert!(matches!(
        status,
        IpcMessage::Status(StatusUpdate {
            status: DownloadStatus::Started,
            ..
        })
    ));

    let error: IpcMessage = serde_json::from_str(&lines[1]).expect("error json");
    match error {
        IpcMessage::Error(error) => {
            assert_eq!(error.id, "model-terminal");
            assert_eq!(error.error, "network timeout");
            assert_eq!(error.retryable, Some(true));
        }
        other => panic!("expected Error variant, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_stream_unknown_task_returns_404_detail() {
    let test = build_app();

    let response = test
        .app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/download/stream?task_id=nonexistent")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = read_json(response).await;
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("nonexistent")),
        "expected missing task error, got {body}"
    );
}
