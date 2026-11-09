use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::Mutex;

use rua_channel::core::channel::{AgentRequest, Channel, Process};
use rua_channel::core::content::{
    ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::streaming::{
    PlatformStreamPolicy, ProcessingIndicator, StreamHandler, StreamType,
};
use rua_core::RuntimeEvent;

use rua_channel::matrix::channel::{
    send_matrix_room_event_with_base_url, upload_matrix_media_with_base_url, MatrixStreamHandler,
    MatrixTypingIndicator,
};
use rua_channel::matrix::{MatrixChannel, MatrixConfig};

struct MockProcess;

#[async_trait]
impl Process for MockProcess {
    async fn handle(&self, _req: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        Box::pin(futures::stream::empty())
    }
}

struct RecordingProcess {
    requests: Arc<Mutex<Vec<AgentRequest>>>,
    notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Process for RecordingProcess {
    async fn handle(&self, req: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        self.requests.lock().await.push(req);
        self.notify.notify_one();
        Box::pin(futures::stream::empty())
    }
}

fn make_target(conversation_id: &str) -> ChannelTarget {
    ChannelTarget {
        agent_id: "default".into(),
        conversation_id: conversation_id.into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn make_outgoing_msg(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "matrix".into(),
        conversation_id: String::new(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text: text.into() }],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn indicator_request() -> AgentRequest {
    AgentRequest {
        agent_id: "default".into(),
        user_id: "@alice:localhost".into(),
        session_id: "!testroom:localhost".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "matrix".into(),
        conversation_id: "!testroom:localhost".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

mod mock_homeserver {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::extract::{Path, State};
    use axum::http::{header, StatusCode};
    use axum::response::Response;
    use axum::routing::{get, post, put};
    use axum::{Json, Router};
    use tokio::sync::Mutex;

    #[derive(Clone)]
    pub struct MockState {
        pub sent_messages: Arc<Mutex<Vec<serde_json::Value>>>,
        pub uploaded_media: Arc<Mutex<Vec<Vec<u8>>>>,
        pub typing_events: Arc<Mutex<Vec<serde_json::Value>>>,
        pub media_auth: Arc<Mutex<Vec<String>>>,
        pub whoami_count: Arc<AtomicUsize>,
        pub sync_queries: Arc<Mutex<Vec<std::collections::HashMap<String, String>>>>,
        pub login_count: Arc<AtomicUsize>,
        pub download_bytes: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
        sync_call_count: Arc<AtomicUsize>,
        sync_response: Arc<serde_json::Value>,
    }

    impl MockState {
        pub fn new(sync_response: serde_json::Value) -> Self {
            Self {
                sent_messages: Arc::new(Mutex::new(Vec::new())),
                uploaded_media: Arc::new(Mutex::new(Vec::new())),
                typing_events: Arc::new(Mutex::new(Vec::new())),
                media_auth: Arc::new(Mutex::new(Vec::new())),
                whoami_count: Arc::new(AtomicUsize::new(0)),
                sync_queries: Arc::default(),
                login_count: Arc::default(),
                download_bytes: Arc::default(),
                sync_call_count: Arc::new(AtomicUsize::new(0)),
                sync_response: Arc::new(sync_response),
            }
        }
    }

    async fn versions() -> Json<serde_json::Value> {
        Json(serde_json::json!({"versions": ["v1.1"]}))
    }

    async fn login(State(state): State<MockState>) -> Json<serde_json::Value> {
        state.login_count.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({
            "user_id": "@bot:localhost",
            "access_token": "syt_test_token",
            "device_id": "TESTDEVICE"
        }))
    }

    async fn whoami(
        State(state): State<MockState>,
        headers: axum::http::HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], "Bearer syt_test_token");
        state.whoami_count.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({"user_id":"@bot:localhost","device_id":"TESTDEVICE"}))
    }

    async fn sync(
        State(state): State<MockState>,
        axum::extract::Query(query): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >,
    ) -> Json<serde_json::Value> {
        state.sync_queries.lock().await.push(query);
        let count = state.sync_call_count.fetch_add(1, Ordering::SeqCst);
        if count > 1 {
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
        let mut response = (*state.sync_response).clone();
        if count == 0 {
            response["next_batch"] = serde_json::json!("catch-up");
            if let Some(rooms) = response
                .pointer_mut("/rooms/join")
                .and_then(|v| v.as_object_mut())
            {
                for room in rooms.values_mut() {
                    room["timeline"]["events"] = serde_json::json!([{
                        "type":"m.room.message","event_id":"$old-history","sender":"@alice:localhost",
                        "origin_server_ts":1710000000000_u64,
                        "content":{"msgtype":"m.text","body":"old history must not be replayed","m.mentions":{"user_ids":["@bot:localhost"]}}
                    }]);
                }
            }
        }
        Json(response)
    }

    async fn send_message(
        State(state): State<MockState>,
        Path((_room_id, _event_type, _txn_id)): Path<(String, String, String)>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        state.sent_messages.lock().await.push(body);
        Json(serde_json::json!({"event_id": "$sent:localhost"}))
    }

    async fn upload_media(
        State(state): State<MockState>,
        headers: axum::http::HeaderMap,
        body: Body,
    ) -> Json<serde_json::Value> {
        state.media_auth.lock().await.push(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
        let bytes = to_bytes(body, 1024 * 1024)
            .await
            .expect("matrix upload body")
            .to_vec();
        state.uploaded_media.lock().await.push(bytes);
        Json(serde_json::json!({"content_uri": "mxc://localhost/uploaded-media"}))
    }

    async fn typing(
        State(state): State<MockState>,
        Path((room_id, user_id)): Path<(String, String)>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        state.typing_events.lock().await.push(serde_json::json!({
            "room_id": room_id,
            "user_id": user_id,
            "body": body,
        }));
        Json(serde_json::json!({}))
    }

    async fn event(Path((room_id, event_id)): Path<(String, String)>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "type":"m.room.message","room_id":room_id,"event_id":event_id,"sender":"@alice:localhost",
            "origin_server_ts":1715000000000_u64,"content":{"msgtype":"m.text","body":"thread root"}
        }))
    }

    async fn members() -> Json<serde_json::Value> {
        Json(serde_json::json!({"chunk": []}))
    }

    async fn media_download(
        State(state): State<MockState>,
        Path((server_name, media_id)): Path<(String, String)>,
    ) -> Response {
        if let Some(bytes) = state.download_bytes.lock().await.get(&media_id).cloned() {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(bytes))
                .unwrap();
        }
        let Some((content_type, body)) = (server_name == "example.com")
            .then_some(match media_id.as_str() {
                "media123" => Some(("image/png", "png-bytes")),
                "audio123" => Some(("audio/mpeg", "mp3-bytes")),
                "video123" => Some(("video/mp4", "mp4-bytes")),
                "file123" => Some(("application/pdf", "pdf-bytes")),
                _ => None,
            })
            .flatten()
        else {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap();
        };
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap()
    }

    async fn fallback(
        req: axum::extract::Request,
    ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
        let path = req.uri().path();
        if path.contains("/state/") {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"errcode": "M_NOT_FOUND", "error": "Event not found."})),
            )
        } else {
            (axum::http::StatusCode::OK, Json(serde_json::json!({})))
        }
    }

    pub async fn start(state: MockState) -> (tokio::task::JoinHandle<()>, String) {
        let app = Router::new()
            .route("/_matrix/client/versions", get(versions))
            .route("/_matrix/client/v3/login", post(login))
            .route("/_matrix/client/v3/account/whoami", get(whoami))
            .route("/_matrix/client/v3/sync", get(sync))
            .route("/_matrix/client/v3/rooms/{room_id}/members", get(members))
            .route(
                "/_matrix/client/v3/rooms/{room_id}/event/{event_id}",
                get(event),
            )
            .route(
                "/_matrix/media/v3/download/{server_name}/{media_id}",
                get(media_download),
            )
            .route(
                "/_matrix/media/v3/config",
                get(|| async { Json(serde_json::json!({"m.upload.size":1048576})) }),
            )
            .route("/_matrix/media/v3/upload", post(upload_media))
            .route(
                "/_matrix/client/v3/rooms/{room_id}/send/{event_type}/{txn_id}",
                put(send_message),
            )
            .route(
                "/_matrix/client/v3/rooms/{room_id}/typing/{user_id}",
                put(typing),
            )
            .fallback(fallback)
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (handle, url)
    }
}

#[test]
fn channel_id_is_matrix() {
    let ch = MatrixChannel::new(MatrixConfig::default());
    assert_eq!(ch.id(), "matrix");
}

#[test]
fn build_request_from_native_parses_body() {
    let ch = MatrixChannel::new(MatrixConfig {
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });

    let native = serde_json::json!({
        "body": "hello world",
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event123"
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "matrix");
    assert_eq!(req.user_id, "@alice:example.com");
    assert_eq!(req.conversation_id, "!room:example.com");
    assert_eq!(req.message_id, Some("$event123".into()));
    assert_eq!(req.metadata["sender_id"], "@alice:example.com");
    assert_eq!(req.metadata["room_id"], "!room:example.com");
    assert_eq!(req.metadata["event_id"], "$event123");
    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello world"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn build_request_defaults_to_fallback_room() {
    let ch = MatrixChannel::new(MatrixConfig {
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });

    let native = serde_json::json!({
        "body": "hi",
        "sender": "@bob:example.com"
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.conversation_id, "!fallback:example.com");
    assert_eq!(req.metadata["room_id"], "!fallback:example.com");
    assert_eq!(req.metadata["sender_id"], "@bob:example.com");
    assert_eq!(req.message_id, None);
}

#[test]
fn build_request_rejects_duplicate_event_id() {
    let ch = MatrixChannel::new(MatrixConfig {
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });
    let native = serde_json::json!({
        "body": "hello",
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-dup"
    });

    ch.build_request_from_native(native.clone())
        .expect("first event should parse");
    let err = ch
        .build_request_from_native(native)
        .expect_err("duplicate event should be rejected");

    assert!(
        err.to_string().contains("duplicate matrix event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_bot_mention_sets_gate_metadata_and_strips_user_id() {
    let ch = MatrixChannel::new(MatrixConfig {
        user_id: "@rua:example.com".into(),
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });

    let native = serde_json::json!({
        "body": "@rua:example.com deploy now",
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-mention"
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["mentioned"], true);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "deploy now"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn build_request_from_native_non_bot_mention_preserves_text() {
    let ch = MatrixChannel::new(MatrixConfig {
        user_id: "@rua:example.com".into(),
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });

    let native = serde_json::json!({
        "body": "@other:example.com keep this",
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-other-mention"
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(req.metadata["bot_mentioned"], false);
    assert_eq!(req.metadata["mentioned"], false);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "@other:example.com keep this"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn build_request_from_native_thread_relation_uses_root_event_session() {
    let ch = MatrixChannel::new(MatrixConfig {
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });

    let native = serde_json::json!({
        "body": "thread reply",
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-thread-reply",
        "m.relates_to": {
            "rel_type": "m.thread",
            "event_id": "$root-event"
        }
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(
        req.session_id,
        "matrix_thread:!room:example.com:$root-event"
    );
    assert_eq!(req.conversation_id, "!room:example.com");
    assert_eq!(req.metadata["matrix_thread_root_event_id"], "$root-event");
    assert_eq!(req.metadata["matrix_rel_type"], "m.thread");
}

#[tokio::test]
async fn download_inbound_mxc_injects_local_path_for_typed_image() {
    let tmp = tempfile::tempdir().unwrap();
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state).await;
    let ch = MatrixChannel::new(MatrixConfig {
        homeserver_url,
        access_token: "syt_test".into(),
        media_dir: tmp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "body": "photo.png",
        "msgtype": "m.image",
        "url": "mxc://example.com/media123",
        "info": {"mimetype": "image/png"},
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-media"
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("matrix media download");

    let local_path = native["local_path"]
        .as_str()
        .expect("local path")
        .to_string();
    assert!(local_path.starts_with(tmp.path().to_string_lossy().as_ref()));
    assert!(local_path.ends_with("photo.png"));
    assert_eq!(native["mime"], "image/png");
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), "png-bytes");

    let req = ch
        .build_request_from_native(native)
        .expect("typed matrix media request");
    match &req.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "mxc://example.com/media123");
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/png"));
        }
        other => panic!("expected image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_mxc_injects_local_paths_for_file_audio_and_video() {
    let tmp = tempfile::tempdir().unwrap();
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state).await;
    let cases = [
        (
            "m.file",
            "mxc://example.com/file123",
            "report.pdf",
            "application/pdf",
            "pdf-bytes",
            "file",
        ),
        (
            "m.audio",
            "mxc://example.com/audio123",
            "voice.mp3",
            "audio/mpeg",
            "mp3-bytes",
            "audio",
        ),
        (
            "m.video",
            "mxc://example.com/video123",
            "clip.mp4",
            "video/mp4",
            "mp4-bytes",
            "video",
        ),
    ];

    for (idx, (msgtype, url, body, mime, bytes, expected)) in cases.into_iter().enumerate() {
        let ch = MatrixChannel::new(MatrixConfig {
            homeserver_url: homeserver_url.clone(),
            access_token: "syt_test".into(),
            media_dir: tmp.path().to_string_lossy().to_string(),
            ..Default::default()
        });
        let mut native = serde_json::json!({
            "body": body,
            "msgtype": msgtype,
            "url": url,
            "info": {"mimetype": mime},
            "sender": "@alice:example.com",
            "room_id": "!room:example.com",
            "event_id": format!("$event-media-{idx}")
        });

        ch.download_inbound_media(&mut native)
            .await
            .expect("matrix media download");
        let local_path = native["local_path"]
            .as_str()
            .expect("local path")
            .to_string();
        assert_eq!(native["mime"], mime);
        assert_eq!(std::fs::read_to_string(&local_path).unwrap(), bytes);

        let req = ch
            .build_request_from_native(native)
            .expect("typed matrix media request");
        match (&req.input[0], expected) {
            (
                IncomingContentPart::File {
                    file_id,
                    path,
                    mime: part_mime,
                    name,
                },
                "file",
            ) => {
                assert_eq!(file_id, url);
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(part_mime.as_deref(), Some(mime));
                assert_eq!(name.as_deref(), Some(body));
            }
            (
                IncomingContentPart::Audio {
                    file_id,
                    path,
                    mime: part_mime,
                    name,
                },
                "audio",
            ) => {
                assert_eq!(file_id, url);
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(part_mime.as_deref(), Some(mime));
                assert_eq!(name.as_deref(), Some(body));
            }
            (
                IncomingContentPart::Video {
                    file_id,
                    path,
                    mime: part_mime,
                    name,
                },
                "video",
            ) => {
                assert_eq!(file_id, url);
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(part_mime.as_deref(), Some(mime));
                assert_eq!(name.as_deref(), Some(body));
            }
            (other, expected) => panic!("expected {expected} part, got {other:?}"),
        }
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_mxc_failure_keeps_text_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state).await;
    let ch = MatrixChannel::new(MatrixConfig {
        homeserver_url,
        access_token: "syt_test".into(),
        media_dir: tmp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "body": "missing.png",
        "msgtype": "m.image",
        "url": "mxc://example.com/missing",
        "info": {"mimetype": "image/png"},
        "sender": "@alice:example.com",
        "room_id": "!room:example.com",
        "event_id": "$event-media"
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("download failure should not abort fallback parsing");

    let req = ch
        .build_request_from_native(native)
        .expect("fallback matrix media request");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "missing.png"),
        other => panic!("expected text fallback, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn upload_and_send_local_image_as_matrix_media_event() {
    let tmp = tempfile::tempdir().unwrap();
    let image_path = tmp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state.clone()).await;

    let mxc = upload_matrix_media_with_base_url(
        &homeserver_url,
        "syt_test",
        &image_path,
        Some("chart.png"),
    )
    .await
    .expect("upload matrix media");
    assert_eq!(mxc, "mxc://localhost/uploaded-media");
    assert_eq!(
        state.uploaded_media.lock().await.as_slice(),
        &[b"png-bytes".to_vec()]
    );

    let event = MatrixChannel::matrix_media_event_content(
        &OutgoingContentPart::Image {
            url: None,
            path: Some(image_path),
            alt: Some("chart.png".into()),
        },
        &mxc,
        tmp.path().join("chart.png").as_path(),
    )
    .expect("matrix event content");
    assert_eq!(event["msgtype"], "m.image");
    assert_eq!(event["body"], "chart.png");
    assert_eq!(event["url"], "mxc://localhost/uploaded-media");
    assert_eq!(event["info"]["mimetype"], "image/png");
    assert_eq!(event["info"]["size"], 9);

    send_matrix_room_event_with_base_url(&homeserver_url, "syt_test", "!room:localhost", event)
        .await
        .expect("send matrix media event");
    let messages = state.sent_messages.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msgtype"], "m.image");
    assert_eq!(messages[0]["url"], "mxc://localhost/uploaded-media");

    server.abort();
}

#[tokio::test]
async fn upload_and_send_local_file_audio_and_video_as_matrix_media_events() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("report.pdf");
    let audio_path = tmp.path().join("voice.mp3");
    let video_path = tmp.path().join("clip.mp4");
    std::fs::write(&file_path, "pdf-bytes").unwrap();
    std::fs::write(&audio_path, "mp3-bytes").unwrap();
    std::fs::write(&video_path, "mp4-bytes").unwrap();
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state.clone()).await;
    let cases = vec![
        (
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path.clone()),
                name: Some("report.pdf".into()),
            },
            file_path,
            "m.file",
            "application/pdf",
        ),
        (
            OutgoingContentPart::Audio {
                url: None,
                path: Some(audio_path.clone()),
                name: Some("voice.mp3".into()),
            },
            audio_path,
            "m.audio",
            "audio/mpeg",
        ),
        (
            OutgoingContentPart::Video {
                url: None,
                path: Some(video_path.clone()),
                name: Some("clip.mp4".into()),
            },
            video_path,
            "m.video",
            "video/mp4",
        ),
    ];

    for (part, path, msgtype, mime) in cases {
        let mxc = upload_matrix_media_with_base_url(&homeserver_url, "syt_test", &path, None)
            .await
            .expect("upload matrix media");
        let event = MatrixChannel::matrix_media_event_content(&part, &mxc, &path)
            .expect("matrix event content");
        assert_eq!(event["msgtype"], msgtype);
        assert_eq!(event["url"], "mxc://localhost/uploaded-media");
        assert_eq!(event["info"]["mimetype"], mime);
        send_matrix_room_event_with_base_url(&homeserver_url, "syt_test", "!room:localhost", event)
            .await
            .expect("send matrix media event");
    }

    let uploaded = state.uploaded_media.lock().await;
    assert_eq!(uploaded.len(), 3);
    assert_eq!(uploaded[0], b"pdf-bytes".to_vec());
    assert_eq!(uploaded[1], b"mp3-bytes".to_vec());
    assert_eq!(uploaded[2], b"mp4-bytes".to_vec());
    let messages = state.sent_messages.lock().await;
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["msgtype"], "m.file");
    assert_eq!(messages[1]["msgtype"], "m.audio");
    assert_eq!(messages[2]["msgtype"], "m.video");

    server.abort();
}

#[tokio::test]
async fn stream_handler_sends_placeholder_and_m_replace_edits() {
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state.clone()).await;
    let handler = MatrixStreamHandler::new(
        homeserver_url,
        "syt_test".into(),
        "!room:localhost".into(),
        PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 3000),
    );

    handler
        .on_delta(StreamType::Message, "hello")
        .await
        .expect("send placeholder");
    handler
        .on_delta(StreamType::Message, "hello there")
        .await
        .expect("send delta edit");
    handler
        .on_end(StreamType::Message, "hello matrix")
        .await
        .expect("send final edit");

    let messages = state.sent_messages.lock().await;
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["msgtype"], "m.text");
    assert_eq!(messages[0]["body"], "hello");

    assert_eq!(messages[1]["msgtype"], "m.text");
    assert_eq!(messages[1]["body"], "* hello there");
    assert_eq!(messages[1]["m.new_content"]["msgtype"], "m.text");
    assert_eq!(messages[1]["m.new_content"]["body"], "hello there");
    assert_eq!(messages[1]["m.relates_to"]["rel_type"], "m.replace");
    assert_eq!(messages[1]["m.relates_to"]["event_id"], "$sent:localhost");

    assert_eq!(messages[2]["msgtype"], "m.text");
    assert_eq!(messages[2]["body"], "* hello matrix");
    assert_eq!(messages[2]["m.new_content"]["msgtype"], "m.text");
    assert_eq!(messages[2]["m.new_content"]["body"], "hello matrix");
    assert_eq!(messages[2]["m.relates_to"]["rel_type"], "m.replace");
    assert_eq!(messages[2]["m.relates_to"]["event_id"], "$sent:localhost");

    server.abort();
}

#[tokio::test]
async fn typing_indicator_sends_start_keepalive_and_stop() {
    let state = mock_homeserver::MockState::new(serde_json::json!({}));
    let (server, homeserver_url) = mock_homeserver::start(state.clone()).await;
    let indicator = MatrixTypingIndicator::with_keepalive_interval(
        homeserver_url,
        "syt_test".into(),
        "!testroom:localhost".into(),
        "@bot:localhost".into(),
        Duration::from_millis(25),
    );
    let req = indicator_request();

    indicator.start(&req).await.expect("start typing");
    tokio::time::sleep(Duration::from_millis(65)).await;
    indicator.stop(&req).await.expect("stop typing");
    let count_after_stop = state.typing_events.lock().await.len();
    tokio::time::sleep(Duration::from_millis(65)).await;

    let typing = state.typing_events.lock().await;
    assert!(
        count_after_stop >= 3,
        "expected start plus keepalive typing events, got {count_after_stop}"
    );
    assert_eq!(
        typing.len(),
        count_after_stop,
        "stop should abort typing keepalive"
    );
    assert_eq!(typing[0]["room_id"], "!testroom:localhost");
    assert_eq!(typing[0]["user_id"], "@bot:localhost");
    assert_eq!(typing[0]["body"]["typing"], true);
    assert_eq!(typing[0]["body"]["timeout"], 30000);
    assert_eq!(typing.last().unwrap()["body"]["typing"], false);

    server.abort();
}

#[tokio::test]
async fn send_fails_without_start() {
    let ch = MatrixChannel::new(MatrixConfig::default());
    let target = make_target("");
    let msg = make_outgoing_msg("test");

    let result = ch.send(target, msg).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("not initialized"), "got: {err_msg}");
}

#[tokio::test]
async fn start_fails_without_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let ch = MatrixChannel::new(MatrixConfig {
        homeserver_url: "https://matrix.example.com".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        ..Default::default()
    });

    let process: Arc<dyn Process> = Arc::new(MockProcess);
    let result = ch.start(process).await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("neither password nor access_token"),
        "got: {err_msg}"
    );
}

#[tokio::test]
async fn stop_is_noop_when_not_started() {
    let ch = MatrixChannel::new(MatrixConfig::default());
    assert!(ch.stop().await.is_ok());
}

#[test]
fn config_deserializes_from_json() {
    let json = serde_json::json!({
        "homeserver_url": "https://matrix.example.com",
        "user_id": "@bot:example.com",
        "password": "secret",
        "default_room_id": "!room:example.com"
    });

    let config: MatrixConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.homeserver_url, "https://matrix.example.com");
    assert_eq!(config.user_id, "@bot:example.com");
    assert_eq!(config.password, "secret");
    assert_eq!(config.default_room_id, "!room:example.com");
    assert!(config.access_token.is_empty());
    assert!(!config.encryption_enabled);
    assert!(config.store_path.is_none());
}

#[test]
fn config_defaults_are_empty() {
    let config = MatrixConfig::default();
    assert!(config.homeserver_url.is_empty());
    assert!(config.user_id.is_empty());
    assert!(config.password.is_empty());
    assert!(config.access_token.is_empty());
    assert!(config.default_room_id.is_empty());
    assert!(!config.encryption_enabled);
    assert!(config.store_path.is_none());
}

#[test]
fn config_serde_roundtrip_with_new_fields() {
    let config = MatrixConfig {
        homeserver_url: "https://matrix.example.com".into(),
        user_id: "@bot:example.com".into(),
        password: String::new(),
        access_token: "tok123".into(),
        default_room_id: "!room:example.com".into(),
        encryption_enabled: true,
        store_path: Some("/tmp/matrix_store".into()),
        media_dir: "/tmp/matrix_media".into(),
    };
    let json = serde_json::to_value(&config).unwrap();
    let roundtripped: MatrixConfig = serde_json::from_value(json).unwrap();
    assert!(roundtripped.encryption_enabled);
    assert_eq!(roundtripped.store_path, Some("/tmp/matrix_store".into()));
    assert_eq!(roundtripped.access_token, "tok123");
}

#[test]
fn config_encryption_defaults_false() {
    let json = serde_json::json!({
        "homeserver_url": "https://matrix.example.com",
        "user_id": "@bot:example.com",
        "password": "secret"
    });
    let config: MatrixConfig = serde_json::from_value(json).unwrap();
    assert!(!config.encryption_enabled);
    assert!(config.store_path.is_none());
}

#[tokio::test]
async fn send_happy_path_via_mock_homeserver() {
    let sync_response = serde_json::json!({
        "next_batch": "s1",
        "rooms": {
            "join": {
                "!testroom:localhost": {
                    "timeline": {"events": [], "limited": false, "prev_batch": "t1"},
                    "state": {"events": []},
                    "ephemeral": {"events": []}
                }
            }
        }
    });

    let state = mock_homeserver::MockState::new(sync_response);
    let (_server, url) = mock_homeserver::start(state.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let config = MatrixConfig {
        homeserver_url: url,
        user_id: "@bot:localhost".into(),
        password: "testpass".into(),
        default_room_id: "!testroom:localhost".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        ..Default::default()
    };

    let ch = MatrixChannel::new(config);
    let process: Arc<dyn Process> = Arc::new(MockProcess);
    ch.start(process).await.expect("start should succeed");

    let target = make_target("!testroom:localhost");
    let msg = make_outgoing_msg("hello matrix");

    let mut last_err = None;
    for _ in 0..50 {
        match ch.send(target.clone(), msg.clone()).await {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    if let Some(e) = last_err {
        panic!("send failed after retries: {e}");
    }

    let messages = state.sent_messages.lock().await;
    assert_eq!(
        messages.len(),
        1,
        "mock should have received exactly 1 message"
    );
    assert_eq!(messages[0]["body"], "hello matrix");
    assert_eq!(messages[0]["msgtype"], "m.text");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn receive_happy_path_via_mock_homeserver() {
    let sync_response = serde_json::json!({
        "next_batch": "s1",
        "rooms": {
            "join": {
                "!testroom:localhost": {
                    "timeline": {
                        "events": [{
                            "type": "m.room.message",
                            "event_id": "$msg1:localhost",
                            "sender": "@alice:localhost",
                            "origin_server_ts": 1715000000000_u64,
                            "content": {
                                "msgtype": "m.text",
                                "body": "hello from alice",
                                "m.mentions": {"user_ids": ["@bot:localhost"]}
                            }
                        }],
                        "limited": false,
                        "prev_batch": "t1"
                    },
                    "state": {"events": []},
                    "ephemeral": {"events": []}
                }
            }
        }
    });

    let state = mock_homeserver::MockState::new(sync_response);
    let (_server, url) = mock_homeserver::start(state).await;

    let tmp = tempfile::tempdir().unwrap();
    let config = MatrixConfig {
        homeserver_url: url,
        user_id: "@bot:localhost".into(),
        password: "testpass".into(),
        default_room_id: "!testroom:localhost".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        ..Default::default()
    };

    let requests: Arc<Mutex<Vec<AgentRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(tokio::sync::Notify::new());
    let recording = Arc::new(RecordingProcess {
        requests: requests.clone(),
        notify: notify.clone(),
    });

    let ch = MatrixChannel::new(config);
    ch.start(recording).await.expect("start should succeed");

    tokio::time::timeout(Duration::from_secs(10), notify.notified())
        .await
        .expect("timeout waiting for sync message delivery");

    let reqs = requests.lock().await;
    assert_eq!(reqs.len(), 1, "process should receive exactly 1 request");
    assert_eq!(reqs[0].channel_id, "matrix");
    assert_eq!(reqs[0].user_id, "@alice:localhost");
    assert_eq!(reqs[0].input.len(), 1);
    match &reqs[0].input[0] {
        IncomingContentPart::Text { text } => {
            assert_eq!(text, "@alice:localhost: hello from alice")
        }
        other => panic!("expected Text, got {other:?}"),
    }

    ch.stop().await.unwrap();
}

#[path = "matrix_matrix_channel/runtime_auth.rs"]
mod runtime_auth;

#[path = "matrix_matrix_channel/runtime_content.rs"]
mod runtime_content;
