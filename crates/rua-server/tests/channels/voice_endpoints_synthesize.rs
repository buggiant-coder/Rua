use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::gap_fixes::agent_config_router;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", agent_config_router())
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
    serde_json::from_slice(&bytes).expect("response is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_audio_mode_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agent/audio-mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.get("audio_mode").is_some(), "missing audio_mode key");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_audio_mode_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/agent/audio-mode")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"audio_mode":"voice"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["audio_mode"].as_str(), Some("voice"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_tts_config_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agent/tts-config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(
        body.get("tts_provider_type").is_some(),
        "missing tts_provider_type"
    );
    assert!(body.get("tts_model").is_some(), "missing tts_model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_tts_config_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/agent/tts-config")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"tts_provider_type":"openai","tts_model":"tts-1-hd","tts_voice":"nova"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["tts_model"].as_str(), Some("tts-1-hd"));
    assert_eq!(body["tts_voice"].as_str(), Some("nova"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_tts_synthesize_not_501() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agent/tts/synthesize")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"text":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::NOT_IMPLEMENTED,
        "synthesize must not return 501"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_transcription_provider_type_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agent/transcription-provider-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(
        body.get("transcription_provider_type").is_some(),
        "missing field"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_transcription_provider_type_accepts_supported_rust_values() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/agent/transcription-provider-type")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"transcription_provider_type":"whisper_api"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(
        body["transcription_provider_type"].as_str(),
        Some("whisper_api")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_transcription_provider_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/agent/transcription-provider")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"provider_id":"my-whisper"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["provider_id"].as_str(), Some("my-whisper"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_transcription_providers_returns_200() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/agent/transcription-providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(body.get("providers").is_some(), "missing providers");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_endpoint_returns_501() {
    let (app, _tmp) = build_app();
    let endpoints = vec![
        ("PUT", "/api/agent/audio-mode", r#"{"audio_mode":"auto"}"#),
        ("PUT", "/api/agent/tts-config", r#"{"tts_model":"tts-1"}"#),
        ("POST", "/api/agent/tts/synthesize", r#"{"text":"hi"}"#),
        (
            "PUT",
            "/api/agent/transcription-provider-type",
            r#"{"transcription_provider_type":"disabled"}"#,
        ),
        (
            "PUT",
            "/api/agent/transcription-provider",
            r#"{"provider_id":""}"#,
        ),
    ];
    for (method, uri, body_str) in endpoints {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("Content-Type", "application/json")
                    .body(Body::from(body_str))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_IMPLEMENTED,
            "{method} {uri} must not return 501"
        );
    }
}
