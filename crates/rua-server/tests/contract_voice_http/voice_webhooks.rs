use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::{router, AppState, ServerProfile};

struct TestApp {
    app: axum::Router,
    state: AppState,
    _tmp: TempDir,
}

fn build_app_with_voice_enabled(enabled: bool) -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    state.voice.set_enabled(enabled);
    let app = router::build_app(state.clone(), None);
    TestApp {
        app,
        state,
        _tmp: tmp,
    }
}

fn build_app() -> TestApp {
    build_app_with_voice_enabled(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_configuration_application_preserves_existing_call_delivery() {
    let app = build_app_with_voice_enabled(true);
    app.state.wait_platform_initialization().await.unwrap();
    let mut outbound = app.state.voice.attach_outbound("existing-call").await;
    let agent_id = app.state.voice.agent_id();
    app.state
        .agent_cells
        .section::<Value>(agent_id, "channels")
        .write(serde_json::json!({"voice":{"enabled":true,"public_url":"https://fixture.example.test"}}))
        .await
        .unwrap();
    rua_server::channels_reload::apply_current(&app.state, agent_id)
        .await
        .unwrap();
    app.state
        .voice
        .send_to_call("existing-call", "existing delivery".to_string())
        .await
        .unwrap();
    assert_eq!(outbound.recv().await.as_deref(), Some("existing delivery"));
    assert!(
        app.state.voice.is_enabled(),
        "successful connection reconfiguration leaves Voice enabled"
    );
}

fn seed_provider(tmp: &TempDir, body: serde_json::Value) {
    let dir = tmp
        .path()
        .join("secret")
        .join("providers")
        .join("providers");
    std::fs::create_dir_all(&dir).expect("providers dir");
    let id = body["id"].as_str().expect("provider id");
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&body).expect("provider json"),
    )
    .expect("write provider json");
}

async fn request_empty(app: &axum::Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn request_raw(
    app: &axum::Router,
    method: Method,
    uri: &str,
    content_type: &'static str,
    body: &'static [u8],
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn request_json(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn request_form(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: &'static str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn request_multipart(
    app: &axum::Router,
    uri: &str,
    boundary: &'static str,
    body: String,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn read_body(resp: axum::response::Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

async fn read_json(resp: axum::response::Response) -> Value {
    serde_json::from_str(&read_body(resp).await).expect("json body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_without_enabled_voice_channel_returns_error_twiml() {
    let app = build_app();

    let resp = request_form(
        &app.app,
        Method::POST,
        "/voice/incoming",
        "CallSid=CA-test-voice-001&From=%2B15550000001&To=%2B15550000002",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        content_type.contains("xml"),
        "voice incoming should return TwiML XML, got {content_type}"
    );

    let body = read_body(resp).await;
    assert!(body.contains("<Response>"));
    assert!(body.contains("Voice channel is not available"));
    assert!(body.contains("<Reject"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_callback_acknowledges_without_enabled_voice_channel() {
    let app = build_app();

    let resp = request_form(
        &app.app,
        Method::POST,
        "/voice/status-callback",
        "CallSid=CA-test-voice-002&CallStatus=completed",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let body = read_body(resp).await;
    assert!(body.is_empty(), "204 callback body should be empty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_route_rejects_plain_http_before_token_validation() {
    let app = build_app_with_voice_enabled(true);

    let resp = request_empty(&app.app, Method::GET, "/voice/ws").await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_config_defaults_to_disabled_and_unsupported() {
    let app = build_app();

    let resp = request_empty(&app.app, Method::GET, "/voice/asr/config").await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["transcription_provider_type"], "disabled");
    assert_eq!(body["transcription_provider_id"], "");
    assert_eq!(body["transcription_model"], "whisper-1");
    assert_eq!(body["supported"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_transcribe_raw_audio_without_provider_returns_unsupported_payload() {
    let app = build_app();

    let resp = request_raw(
        &app.app,
        Method::POST,
        "/voice/asr/transcribe",
        "audio/webm",
        b"\x00\x01\x02\x03",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({
            "text": "",
            "supported": false,
            "detail": "audio transcription is not configured"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_transcribe_api_compat_route_matches_root_disabled_contract() {
    let app = build_app();

    let resp = request_raw(
        &app.app,
        Method::POST,
        "/api/voice/asr",
        "audio/wav",
        b"\x00\x01\x02\x03",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["text"], "");
    assert_eq!(body["supported"], false);
    assert_eq!(body["detail"], "audio transcription is not configured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_transcribe_multipart_requires_file_field() {
    let app = build_app();
    let boundary = "rua-voice-asr-boundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"model\"\r\n\r\n\
         whisper-1\r\n\
         --{boundary}--\r\n"
    );

    let resp = request_multipart(&app.app, "/voice/asr/transcribe", boundary, body).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(resp).await,
        serde_json::json!({"detail": "file field is required"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_transcribe_multipart_rejects_unsupported_file_extension_before_provider_dispatch() {
    let app = build_app();
    let provider_type = request_json(
        &app.app,
        Method::PUT,
        "/api/agent/transcription-provider-type",
        serde_json::json!({"transcription_provider_type": "whisper_api"}),
    )
    .await;
    assert_eq!(provider_type.status(), StatusCode::OK);

    let boundary = "rua-voice-asr-txt-boundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\
         Content-Type: text/plain\r\n\r\n\
         hello\r\n\
         --{boundary}--\r\n"
    );

    let resp = request_multipart(&app.app, "/voice/asr/transcribe", boundary, body).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = read_json(resp).await;
    assert_eq!(body["detail"]["code"], "UNSUPPORTED_FILE_TYPE");
    assert!(
        body["detail"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(".txt") && message.contains(".webm")),
        "expected unsupported extension detail with allowed extensions, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcription_provider_type_roundtrips_on_rust_root_and_scoped_routes() {
    let app = build_app();

    let initial = request_empty(
        &app.app,
        Method::GET,
        "/api/agent/transcription-provider-type",
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(
        read_json(initial).await,
        serde_json::json!({"transcription_provider_type": "disabled"})
    );

    let update = request_json(
        &app.app,
        Method::PUT,
        "/api/agents/default/transcription-provider-type",
        serde_json::json!({"transcription_provider_type": "whisper_api"}),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(
        read_json(update).await,
        serde_json::json!({"transcription_provider_type": "whisper_api"})
    );

    let root_after = request_empty(
        &app.app,
        Method::GET,
        "/api/agent/transcription-provider-type",
    )
    .await;
    assert_eq!(root_after.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_after).await,
        serde_json::json!({"transcription_provider_type": "whisper_api"})
    );

    let local = request_json(
        &app.app,
        Method::PUT,
        "/api/agent/transcription-provider-type",
        serde_json::json!({"transcription_provider_type": "local_whisper"}),
    )
    .await;
    assert_eq!(local.status(), StatusCode::OK);
    assert_eq!(
        read_json(local).await,
        serde_json::json!({"transcription_provider_type": "local_whisper"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_whisper_status_uses_qwenpaw_and_console_field_names() {
    let app = build_app();

    for path in [
        "/api/agent/local-whisper-status",
        "/api/agents/default/local-whisper-status",
    ] {
        let resp = request_empty(&app.app, Method::GET, path).await;
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let body = read_json(resp).await;
        assert_eq!(
            body,
            serde_json::json!({
                "available": false,
                "ffmpeg_installed": false,
                "whisper_installed": false,
            }),
            "{path}"
        );
        assert!(
            body.get("ffmpeg_available").is_none() && body.get("whisper_available").is_none(),
            "Rust backend must match QwenPaw and console API field names: {body:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcription_providers_list_projects_rust_provider_cells() {
    let app = build_app();
    seed_provider(
        &app._tmp,
        serde_json::json!({
            "id": "dashscope",
            "name": "DashScope",
            "base_url": "https://dashscope.example/v1",
            "api_key": "sk-dashscope-secret",
            "chat_model": "DashScopeChatModel",
            "models": [{"id": "qwen-audio", "name": "Qwen Audio"}],
            "require_api_key": true
        }),
    );
    seed_provider(
        &app._tmp,
        serde_json::json!({
            "id": "openai-asr",
            "name": "OpenAI ASR",
            "base_url": "https://openai.example/v1",
            "api_key": "",
            "chat_model": "OpenAIChatModel",
            "models": [{"id": "whisper-1", "name": "Whisper"}],
            "require_api_key": false
        }),
    );
    seed_provider(
        &app._tmp,
        serde_json::json!({
            "id": "needs-key",
            "name": "Needs API Key",
            "base_url": "https://needs-key.example/v1",
            "api_key": "",
            "chat_model": "OpenAIChatModel",
            "models": [{"id": "whisper-1", "name": "Whisper"}],
            "require_api_key": true
        }),
    );
    seed_provider(
        &app._tmp,
        serde_json::json!({
            "id": "unconfigured",
            "name": "Unconfigured",
            "base_url": "",
            "api_key": "sk-unused",
            "chat_model": "OpenAIChatModel"
        }),
    );

    let configured = request_json(
        &app.app,
        Method::PUT,
        "/api/agent/transcription-provider",
        serde_json::json!({"provider_id": "openai-asr"}),
    )
    .await;
    assert_eq!(configured.status(), StatusCode::OK);

    for path in [
        "/api/agent/transcription-providers",
        "/api/agents/default/transcription-providers",
    ] {
        let resp = request_empty(&app.app, Method::GET, path).await;
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let body = read_json(resp).await;
        assert_eq!(body["configured_provider_id"], "openai-asr", "{path}");
        let providers = body["providers"].as_array().expect("providers array");
        assert_eq!(
            providers
                .iter()
                .map(|p| p["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["dashscope", "openai-asr"],
            "only provider configs usable by Rust voice HTTP calls should be listed: {body:?}"
        );
        assert_eq!(providers[0]["type"], "dashscope");
        assert_eq!(providers[1]["type"], "openai");
        assert!(providers
            .iter()
            .all(|provider| provider["available"] == true));
        assert!(
            !body.to_string().contains("sk-dashscope-secret"),
            "voice provider list must not expose provider secrets: {body:?}"
        );
        assert!(
            !providers.iter().any(|provider| provider["id"] == "needs-key"),
            "providers that require an API key must not be offered while the key is empty: {body:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcription_provider_type_rejects_unknown_value_without_mutating_state() {
    let app = build_app();

    let seed = request_json(
        &app.app,
        Method::PUT,
        "/api/agent/transcription-provider-type",
        serde_json::json!({"transcription_provider_type": "whisper_api"}),
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);

    let invalid = request_json(
        &app.app,
        Method::PUT,
        "/api/agents/default/transcription-provider-type",
        serde_json::json!({"transcription_provider_type": "foobar"}),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = read_json(invalid).await;
    assert!(
        invalid_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Invalid transcription_provider_type")),
        "expected invalid provider detail, got {invalid_body:?}"
    );

    let after = request_empty(
        &app.app,
        Method::GET,
        "/api/agent/transcription-provider-type",
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(
        read_json(after).await,
        serde_json::json!({"transcription_provider_type": "whisper_api"})
    );
}

async fn configured_voice_app() -> TestApp {
    let app = build_app_with_voice_enabled(true);
    app.state
        .agent_cells
        .section::<Value>(app.state.voice.agent_id(), "channels")
        .write(serde_json::json!({"voice": {
            "enabled": true,
            "twilio_account_sid": "AC-test",
            "twilio_auth_token": "test-auth",
            "phone_number": "+200",
            "public_url": "https://calls.example.test",
            "language": "zh-CN",
            "tts_provider": "Google",
            "tts_voice": "voice-1",
            "stt_provider": "Deepgram",
            "welcome_greeting": "你好 & welcome",
            "dm_policy": "allowlist",
            "allow_from": ["+100"]
        }}))
        .await
        .unwrap();
    app
}

async fn signed_incoming(app: &TestApp, from: &str) -> axum::response::Response {
    app.app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/voice/incoming")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                // Independently computed HMAC-SHA1 over this URL and the sorted form.
                .header("x-twilio-signature", "VS4Ax8AiTLlPhtoHEO/ySSaEeDY=")
                .body(Body::from(format!(
                    "AccountSid=AC-test&CallSid=CA-signed&From=%2B{from}&To=%2B200"
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_incoming_uses_agent_config_and_binds_token_to_call_sid() {
    let app = configured_voice_app().await;
    let response = signed_incoming(&app, "100").await;
    assert_eq!(response.status(), StatusCode::OK);
    let xml = read_body(response).await;
    assert!(
        xml.contains("wss://calls.example.test/voice/ws?token="),
        "{xml}"
    );
    assert!(
        xml.contains("welcomeGreeting=\"你好 &amp; welcome\""),
        "{xml}"
    );
    assert!(xml.contains("ttsProvider=\"Google\""), "{xml}");
    assert!(xml.contains("transcriptionProvider=\"Deepgram\""), "{xml}");
    assert!(xml.contains("voice=\"voice-1\""), "{xml}");
    assert!(xml.contains("language=\"zh-CN\""), "{xml}");
    let token = xml
        .split("token=")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(
        app.state.voice.validate_ws_token(token).await.unwrap(),
        "CA-signed"
    );
    assert!(app.state.voice.validate_ws_token(token).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_incoming_rejects_modified_form_without_issuing_token() {
    let app = configured_voice_app().await;
    let response = signed_incoming(&app, "101").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(response).await["detail"],
        "invalid Twilio signature"
    );
    assert_eq!(app.state.voice.token_count().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_voice_config_closes_calls_and_revokes_pending_tokens() {
    let app = configured_voice_app().await;
    assert_eq!(signed_incoming(&app, "100").await.status(), StatusCode::OK);
    assert_eq!(app.state.voice.token_count().await, 1);
    let mut outbound = app.state.voice.attach_outbound("CA-signed").await;
    let path = format!(
        "/api/agents/{}/config/channels/voice",
        app.state.voice.agent_id()
    );
    let response = request_json(
        &app.app,
        Method::PUT,
        &path,
        serde_json::json!({"enabled": false}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!app.state.voice.is_enabled());
    assert_eq!(app.state.voice.token_count().await, 0);
    assert!(outbound.recv().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_number_sid_configures_callbacks_and_reload_is_idempotent() {
    use axum::{extract::Path, routing::post, Form, Json};
    use std::{collections::HashMap, sync::Arc};
    let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = calls.clone();
    let api = axum::Router::new().route(
        "/Accounts/{account}/IncomingPhoneNumbers/{phone}",
        post(
            move |Path((account, phone)): Path<(String, String)>,
                  headers: axum::http::HeaderMap,
                  Form(form): Form<HashMap<String, String>>| {
                let captured = captured.clone();
                async move {
                    assert_eq!(account, "AC-test");
                    assert_eq!(phone, "PN-test.json");
                    assert_eq!(headers["authorization"], "Basic QUMtdGVzdDp0ZXN0LWF1dGg=");
                    captured.lock().await.push(form);
                    Json(serde_json::json!({"sid":"PN-test"}))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, api).await.unwrap();
    });
    let app = configured_voice_app().await;
    let cell = app
        .state
        .agent_cells
        .section::<Value>(app.state.voice.agent_id(), "channels");
    cell.update(|existing| {
        let mut v = existing.unwrap();
        v["voice"]["phone_number_sid"] = serde_json::json!("PN-test");
        v["voice"]["twilio_api_base"] = serde_json::json!(api_base);
        v
    })
    .await
    .unwrap();
    for _ in 0..2 {
        rua_server::channels_reload::reload_from_disk(&app.state, app.state.voice.agent_id()).await;
    }
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["VoiceUrl"],
        "https://calls.example.test/voice/incoming"
    );
    assert_eq!(
        calls[0]["StatusCallback"],
        "https://calls.example.test/voice/status-callback"
    );
    assert_eq!(calls[0]["VoiceMethod"], "POST");
    assert_eq!(calls[0]["StatusCallbackMethod"], "POST");
    drop(calls);
    assert!(app.state.voice.is_enabled());
    assert_eq!(signed_incoming(&app, "100").await.status(), StatusCode::OK);
    cell.update(|existing| {
        let mut v = existing.unwrap();
        v["voice"]["enabled"] = serde_json::json!(false);
        v
    })
    .await
    .unwrap();
    rua_server::channels_reload::reload_from_disk(&app.state, app.state.voice.agent_id()).await;
    assert!(!app.state.voice.is_enabled());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_twilio_webhook_setup_disables_voice_and_reports_health() {
    let api = axum::Router::new().fallback(|| async { StatusCode::UNAUTHORIZED });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, api).await.unwrap();
    });
    let app = configured_voice_app().await;
    app.state
        .agent_cells
        .section::<Value>(app.state.voice.agent_id(), "channels")
        .update(|existing| {
            let mut v = existing.unwrap();
            v["voice"]["phone_number_sid"] = serde_json::json!("PN-test");
            v["voice"]["twilio_api_base"] = serde_json::json!(api_base);
            v
        })
        .await
        .unwrap();
    rua_server::channels_reload::reload_from_disk(&app.state, app.state.voice.agent_id()).await;
    assert!(!app.state.voice.is_enabled());
    let response = request_empty(
        &app.app,
        Method::GET,
        &format!(
            "/api/agents/{}/config/channels/voice/health",
            app.state.voice.agent_id()
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["status"], "degraded");
    assert!(body["detail"].as_str().unwrap().contains("401"));
    server.abort();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_voice_url_reuses_tunnel_and_preserves_it_when_disabled() {
    use axum::{routing::post, Form, Json};
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let app = configured_voice_app().await;
    let script = app._tmp.path().join("fake-cloudflared");
    std::fs::write(
        &script,
        b"#!/bin/sh\nprintf 'https://calls-fixture.trycloudflare.com\\n' >&2\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let driver = rua_tunnel::CloudflareTunnelDriver::new(rua_tunnel::BinaryManager::new(
        app._tmp.path().join("unused"),
    ));
    driver.start_tunnel(&script, 8765).await.unwrap();
    *app.state.tunnel_driver.lock().await = Some(driver);
    let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = calls.clone();
    let api = axum::Router::new().route(
        "/Accounts/{*path}",
        post(
            move |Form(form): Form<std::collections::HashMap<String, String>>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.push(form);
                    Json(serde_json::json!({}))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, api).await.unwrap();
    });
    let cell = app
        .state
        .agent_cells
        .section::<Value>(app.state.voice.agent_id(), "channels");
    cell.update(|existing| {
        let mut v = existing.unwrap();
        v["voice"]["public_url"] = serde_json::json!("");
        v["voice"]["phone_number_sid"] = serde_json::json!("PN-test");
        v["voice"]["twilio_api_base"] = serde_json::json!(base);
        v
    })
    .await
    .unwrap();
    rua_server::channels_reload::reload_from_disk(&app.state, app.state.voice.agent_id()).await;
    assert!(app.state.voice.is_enabled());
    assert_eq!(
        calls.lock().await[0]["VoiceUrl"],
        "https://calls-fixture.trycloudflare.com/voice/incoming"
    );
    assert_eq!(
        cell.read().await.unwrap().unwrap()["voice"]["public_url"],
        "",
        "generated URL must not overwrite saved config"
    );
    cell.update(|existing| {
        let mut v = existing.unwrap();
        v["voice"]["enabled"] = serde_json::json!(false);
        v
    })
    .await
    .unwrap();
    rua_server::channels_reload::reload_from_disk(&app.state, app.state.voice.agent_id()).await;
    assert!(!app.state.voice.is_enabled());
    let mut guard = app.state.tunnel_driver.lock().await;
    assert!(guard.as_ref().unwrap().is_running().await);
    guard.as_ref().unwrap().stop().await;
    *guard = None;
    server.abort();
}
