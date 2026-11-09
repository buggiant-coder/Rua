use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::services::device_command_bus::{
    TerminalDeviceAction, TerminalDeviceCommand, TerminalDeviceResponse,
};
use rua_server::services::terminal::{
    TerminalActivityFrame, TerminalAuthorityConfig, TerminalPalette, TerminalProcessControl,
    TerminalProcessHandle, TerminalProcessSpawner, TerminalProfile, TerminalProfileList,
    TerminalPtyGeometry, TerminalRemoteActivityClient, TerminalRemoteActivityRequest,
    TerminalRemoteCommandClient, TerminalRemoteCommandError, TerminalSessionAuthority,
    TerminalSessionSnapshot, TerminalSnapshot, TerminalStreamCursor, TerminalViewerIdentity,
};
use rua_server::{endpoints, AppState, ServerProfile};
use rua_sync::identity::DeviceIdentity;

#[test]
fn terminal_endpoint_uses_placement_service_instead_of_direct_route_branching() {
    let endpoint_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/endpoints/terminal.rs"),
    )
    .expect("read terminal endpoint source");

    assert!(
        !endpoint_source.contains("route_terminal_execution_device"),
        "terminal endpoints should delegate placement to the terminal service surface"
    );
    assert!(
        !endpoint_source.contains("TerminalPlacementRoute::"),
        "terminal endpoints should not branch on local/remote placement directly"
    );
}

struct TestApp {
    app: Router,
    spawner: Arc<HttpFakeTerminalSpawner>,
    remote_activity_client: Arc<HttpFakeRemoteTerminalActivityClient>,
    _tmp: TempDir,
}

struct HttpFakeTerminalControl {
    inputs: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl TerminalProcessControl for HttpFakeTerminalControl {
    fn kill(&self) {}

    fn write_input(
        &self,
        data: &[u8],
    ) -> Result<(), rua_server::services::terminal::TerminalError> {
        self.inputs.lock().expect("inputs lock").push(data.to_vec());
        Ok(())
    }
}

struct HttpFakeTerminalSpawner {
    inputs: Arc<Mutex<Vec<Vec<u8>>>>,
    output_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>>,
}

impl HttpFakeTerminalSpawner {
    fn new() -> Self {
        Self {
            inputs: Arc::new(Mutex::new(Vec::new())),
            output_tx: Arc::new(Mutex::new(None)),
        }
    }

    fn inputs(&self) -> Vec<Vec<u8>> {
        self.inputs.lock().expect("inputs lock").clone()
    }

    fn emit_output(&self, data: &[u8]) {
        self.output_tx
            .lock()
            .expect("output tx lock")
            .as_ref()
            .expect("terminal output tx")
            .send(data.to_vec())
            .expect("send terminal output");
    }
}

impl TerminalProcessSpawner for HttpFakeTerminalSpawner {
    fn list_profiles(&self) -> TerminalProfileList {
        terminal_profile_list()
    }

    fn spawn(
        &self,
        _request: rua_server::services::terminal::TerminalSpawnRequest,
    ) -> Result<TerminalProcessHandle, rua_server::services::terminal::TerminalError> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(b"terminal-ready\n".to_vec()).expect("send replay");
        *self.output_tx.lock().expect("output tx lock") = Some(tx);
        Ok(TerminalProcessHandle {
            output: rx,
            control: Arc::new(HttpFakeTerminalControl {
                inputs: Arc::clone(&self.inputs),
            }),
        })
    }
}

#[derive(Default)]
struct HttpFakeRemoteTerminalActivityClient {
    requests: Mutex<Vec<TerminalRemoteActivityRequest>>,
    subscribers: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<TerminalActivityFrame>>>,
}

impl HttpFakeRemoteTerminalActivityClient {
    fn requests(&self) -> Vec<TerminalRemoteActivityRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn emit(&self, frame: TerminalActivityFrame) {
        let mut subscribers = self.subscribers.lock().expect("subscribers lock");
        subscribers.retain(|tx| tx.send(frame.clone()).is_ok());
    }
}

#[async_trait]
impl TerminalRemoteActivityClient for HttpFakeRemoteTerminalActivityClient {
    async fn subscribe(
        &self,
        request: TerminalRemoteActivityRequest,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<TerminalActivityFrame>, String> {
        self.requests.lock().expect("requests lock").push(request);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.subscribers.lock().expect("subscribers lock").push(tx);
        Ok(rx)
    }
}

#[derive(Default)]
struct HttpFakeRemoteTerminalClient {
    commands: Mutex<Vec<TerminalDeviceCommand>>,
}

struct FailingRemoteTerminalClient;

#[async_trait]
impl TerminalRemoteCommandClient for FailingRemoteTerminalClient {
    async fn deliver(
        &self,
        _command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalRemoteCommandError> {
        Err(TerminalRemoteCommandError::Unavailable(
            "terminal snapshot unavailable on target authority".to_string(),
        ))
    }
}

struct InputLeaseRejectedRemoteTerminalClient;

#[async_trait]
impl TerminalRemoteCommandClient for InputLeaseRejectedRemoteTerminalClient {
    async fn deliver(
        &self,
        _command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalRemoteCommandError> {
        Err(TerminalRemoteCommandError::Rejected {
            reason: "terminal_input_lease_required".to_string(),
            detail: "terminal input lease is required".to_string(),
        })
    }
}

impl HttpFakeRemoteTerminalClient {
    fn commands(&self) -> Vec<TerminalDeviceCommand> {
        self.commands.lock().expect("commands lock").clone()
    }
}

fn terminal_profile() -> TerminalProfile {
    TerminalProfile {
        id: "proot".to_string(),
        label: "Debian".to_string(),
        kind: "proot".to_string(),
    }
}

fn terminal_profile_list() -> TerminalProfileList {
    TerminalProfileList {
        os: "android".to_string(),
        default_profile_id: "proot".to_string(),
        profiles: vec![terminal_profile()],
    }
}

fn remote_snapshot(
    session_id: impl Into<String>,
    viewer_count: usize,
    input_lease_holder: Option<TerminalViewerIdentity>,
    geometry: TerminalPtyGeometry,
) -> TerminalSessionSnapshot {
    let profile = terminal_profile();
    TerminalSessionSnapshot {
        session_id: session_id.into(),
        cwd: "/remote/home".into(),
        profile_id: profile.id,
        profile_label: profile.label,
        display_name: None,
        viewer_count,
        input_lease_holder,
        pty_geometry: geometry,
    }
}

#[async_trait]
impl TerminalRemoteCommandClient for HttpFakeRemoteTerminalClient {
    async fn deliver(
        &self,
        command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalRemoteCommandError> {
        self.commands
            .lock()
            .expect("commands lock")
            .push(command.clone());
        match command.action {
            TerminalDeviceAction::Open { .. } => Ok(TerminalDeviceResponse::Opened {
                session: remote_snapshot(
                    "remote-session-1",
                    1,
                    Some(TerminalViewerIdentity::paired(
                        command.source_device_id,
                        "browser-a",
                    )),
                    TerminalPtyGeometry { cols: 80, rows: 24 },
                ),
            }),
            TerminalDeviceAction::Attach {
                session_id,
                browser_viewer_id,
                cursor,
                ..
            } => {
                let session = remote_snapshot(
                    session_id,
                    2,
                    Some(TerminalViewerIdentity::paired(
                        command.source_device_id,
                        browser_viewer_id,
                    )),
                    TerminalPtyGeometry { cols: 80, rows: 24 },
                );
                if let Some(cursor) = cursor {
                    let data = b"remote-tail\n".to_vec();
                    let live_cursor = TerminalStreamCursor {
                        next_activity_sequence: cursor.next_activity_sequence + 1,
                        next_output_offset: cursor.next_output_offset + data.len() as u64,
                    };
                    Ok(TerminalDeviceResponse::Attached {
                        session,
                        snapshot: TerminalSnapshot {
                            data: "remote-snapshot\n".to_string(),
                            cols: 80,
                            rows: 24,
                            cursor,
                        },
                        tail: vec![TerminalActivityFrame::Output {
                            activity_sequence: cursor.next_activity_sequence,
                            start_offset: cursor.next_output_offset,
                            end_offset: live_cursor.next_output_offset,
                            data,
                        }],
                        live_cursor,
                    })
                } else {
                    let live_cursor = TerminalStreamCursor {
                        next_activity_sequence: 1,
                        next_output_offset: 14,
                    };
                    Ok(TerminalDeviceResponse::Attached {
                        session,
                        snapshot: TerminalSnapshot {
                            data: "remote-snapshot\n".to_string(),
                            cols: 80,
                            rows: 24,
                            cursor: live_cursor,
                        },
                        tail: Vec::new(),
                        live_cursor,
                    })
                }
            }
            TerminalDeviceAction::AttachmentChunk { .. } => {
                Err(TerminalRemoteCommandError::Rejected {
                    reason: "unexpected_terminal_attachment_chunk".to_string(),
                    detail: "unexpected terminal attachment chunk request".to_string(),
                })
            }
            TerminalDeviceAction::Rename {
                session_id,
                display_name,
            } => {
                let mut session = remote_snapshot(
                    session_id,
                    1,
                    None,
                    TerminalPtyGeometry { cols: 80, rows: 24 },
                );
                session.display_name = display_name;
                Ok(TerminalDeviceResponse::Renamed { session })
            }
            TerminalDeviceAction::SetScrollback { session_id, .. } => {
                Ok(TerminalDeviceResponse::ScrollbackUpdated { session_id })
            }
            TerminalDeviceAction::ListSessions => Ok(TerminalDeviceResponse::Sessions {
                sessions: vec![remote_snapshot(
                    "remote-session-1",
                    1,
                    None,
                    TerminalPtyGeometry { cols: 80, rows: 24 },
                )],
            }),
            TerminalDeviceAction::ListProfiles => Ok(TerminalDeviceResponse::Profiles {
                profiles: terminal_profile_list(),
            }),
            TerminalDeviceAction::TakeInputLease { session_id, .. } => {
                Ok(TerminalDeviceResponse::InputLeaseTaken {
                    session: remote_snapshot(
                        session_id,
                        1,
                        Some(TerminalViewerIdentity::paired(
                            command.source_device_id,
                            "browser-b",
                        )),
                        TerminalPtyGeometry { cols: 80, rows: 24 },
                    ),
                })
            }
            TerminalDeviceAction::AdaptPtyGeometry {
                session_id,
                cols,
                rows,
                ..
            } => Ok(TerminalDeviceResponse::PtyGeometryAdapted {
                session: remote_snapshot(
                    session_id,
                    1,
                    Some(TerminalViewerIdentity::paired(
                        command.source_device_id,
                        "browser-b",
                    )),
                    TerminalPtyGeometry { cols, rows },
                ),
            }),
            TerminalDeviceAction::UpdatePalette { session_id, .. } => {
                Ok(TerminalDeviceResponse::PaletteUpdated { session_id })
            }
            TerminalDeviceAction::SendInput { session_id, .. } => {
                Ok(TerminalDeviceResponse::InputAccepted { session_id })
            }
            TerminalDeviceAction::DetachViewer { session_id, .. } => {
                Ok(TerminalDeviceResponse::ViewerDetached { session_id })
            }
            TerminalDeviceAction::KillSession { session_id, .. } => {
                Ok(TerminalDeviceResponse::SessionKilled { session_id })
            }
        }
    }
}

fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let spawner = Arc::new(HttpFakeTerminalSpawner::new());
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        spawner.clone(),
    ));
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let remote_activity_client = Arc::new(HttpFakeRemoteTerminalActivityClient::default());
    state.terminal_remote_activity_client = remote_activity_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);
    TestApp {
        app,
        spawner,
        remote_activity_client,
        _tmp: tmp,
    }
}

fn set_local_sync_device(state: &mut AppState) -> String {
    let device = Arc::new(DeviceIdentity::generate());
    let device_id = hex::encode(device.device_id_short());
    *state.sync_device.write().expect("sync device lock") = Some(device);
    device_id
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
            Body::from(serde_json::to_vec(&value).expect("serialize request"))
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
    serde_json::from_slice(&bytes).expect("response json")
}

async fn next_sse_json(body: &mut axum::body::Body) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(1), body.frame())
        .await
        .expect("terminal activity should yield frame")
        .expect("terminal activity ended")
        .expect("terminal activity frame");
    let data = frame.data_ref().expect("terminal activity data");
    let text = std::str::from_utf8(data).expect("terminal activity utf8");
    let data_line = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("sse data line");
    serde_json::from_str(data_line).expect("terminal activity json")
}

fn assert_canonical_terminal_attachment(body: &Value) {
    assert!(body["session"].is_object());
    assert!(body["snapshot"]["data"].is_string());
    assert!(body["snapshot"]["cols"].is_number());
    assert!(body["snapshot"]["rows"].is_number());
    assert!(body["snapshot"]["cursor"]["next_activity_sequence"].is_number());
    assert!(body["snapshot"]["cursor"]["next_output_offset"].is_number());
    assert!(body["tail"].is_array());
    assert!(body["live_cursor"]["next_activity_sequence"].is_number());
    assert!(body["live_cursor"]["next_output_offset"].is_number());
    let keys = body
        .as_object()
        .expect("terminal attachment object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        ["live_cursor", "session", "snapshot", "tail"]
            .into_iter()
            .collect()
    );
}

fn files_containing(root: &std::path::Path, needle: &str) -> Vec<std::path::PathBuf> {
    fn visit(path: &std::path::Path, needle: &str, matches: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, needle, matches);
            } else if path.is_file() {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                if String::from_utf8_lossy(&bytes).contains(needle) {
                    matches.push(path);
                }
            }
        }
    }

    let mut matches = Vec::new();
    visit(root, needle, &mut matches);
    matches
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_profiles_expose_only_debian_on_android() {
    let app = build_app();

    let response = request(
        &app.app,
        Method::GET,
        "/api/terminal/profiles?execution_device=local",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["os"], "android");
    assert_eq!(body["default_profile_id"], "proot");
    assert_eq!(body["profiles"][0]["id"], "proot");
    assert_eq!(body["profiles"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_profiles_use_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let response = request(
        &app,
        Method::GET,
        "/api/terminal/profiles?execution_device=remote-device",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(body["profiles"][0]["id"], "proot");
    assert_eq!(body["profiles"][0]["label"], "Debian");
    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::ListProfiles
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_sessions_can_be_created_listed_and_killed_without_sync() {
    let app = build_app();

    let initial = request(
        &app.app,
        Method::GET,
        "/api/terminal/sessions?execution_device=local",
        None,
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(read_json(initial).await["sessions"], json!([]));

    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000,
            "terminal_profile_id": "proot"
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    assert_eq!(opened_body["session"]["viewer_count"], 1);
    assert_eq!(opened_body["session"]["profile_id"], "proot");
    assert_eq!(opened_body["session"]["profile_label"], "Debian");

    let listed = request(
        &app.app,
        Method::GET,
        "/api/terminal/sessions?execution_device=local",
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body = read_json(listed).await;
    assert_eq!(
        listed_body["sessions"].as_array().expect("sessions").len(),
        1
    );
    assert_eq!(listed_body["sessions"][0]["session_id"], session_id);

    let updated = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/scrollback"),
        Some(json!({ "scrollback_lines": 24000 })),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);

    let killed = request(
        &app.app,
        Method::DELETE,
        &format!("/api/terminal/sessions/{session_id}?browser_viewer_id=browser-a"),
        None,
    )
    .await;
    assert_eq!(killed.status(), StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_output_and_input_are_not_persisted_as_conversation_or_memory_state() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let attached = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::OK);
    let attached = read_json(attached).await;
    assert!(
        attached["snapshot"]["data"]
            .as_str()
            .is_some_and(|snapshot| snapshot.contains("terminal-ready")),
        "test must exercise terminal snapshot restoration"
    );
    assert_eq!(attached["snapshot"]["cursor"], attached["live_cursor"]);

    let input = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/input"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "data": "echo terminal-secret-marker\n"
        })),
    )
    .await;
    assert_eq!(input.status(), StatusCode::NO_CONTENT);

    for marker in ["terminal-ready", "terminal-secret-marker"] {
        let matches = files_containing(app._tmp.path(), marker);
        assert!(
            matches.is_empty(),
            "terminal marker `{marker}` must stay out of persisted conversation/memory state, found in {matches:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_detach_removes_viewer_without_killing_live_session() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let attached = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::OK);
    assert_eq!(read_json(attached).await["session"]["viewer_count"], 2);

    let detached = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/detach"),
        Some(json!({ "browser_viewer_id": "browser-b" })),
    )
    .await;
    assert_eq!(detached.status(), StatusCode::NO_CONTENT);

    let listed = request(
        &app.app,
        Method::GET,
        "/api/terminal/sessions?execution_device=local",
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = read_json(listed).await;
    assert_eq!(body["sessions"][0]["session_id"], session_id);
    assert_eq!(body["sessions"][0]["viewer_count"], 1);

    let accepted_input = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/input"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "data": "echo still-live\n"
        })),
    )
    .await;
    assert_eq!(accepted_input.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.spawner.inputs(), vec![b"echo still-live\n".to_vec()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_open_without_source_identity_does_not_fallback_to_local() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    *state.sync_device.write().expect("sync device lock") = None;
    let local_spawner = Arc::new(HttpFakeTerminalSpawner::new());
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        local_spawner,
    ));
    state.terminal_remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let opened = request(
        &app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000,
            "terminal_profile_id": "proot"
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::SERVICE_UNAVAILABLE);

    let listed = request(
        &app,
        Method::GET,
        "/api/terminal/sessions?execution_device=local",
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        read_json(listed).await["sessions"],
        json!([]),
        "remote failure must not create a local fallback session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_attach_takeover_and_input_follow_input_lease() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let attached = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::OK);
    let attached_body = read_json(attached).await;
    assert_eq!(
        attached_body["session"]["input_lease_holder"]["browser_id"],
        "browser-a"
    );
    assert_eq!(
        attached_body["snapshot"]["cursor"]["next_activity_sequence"],
        1
    );
    assert_eq!(
        attached_body["snapshot"]["cursor"]["next_output_offset"],
        15
    );
    assert_eq!(
        attached_body["live_cursor"],
        attached_body["snapshot"]["cursor"]
    );
    assert_eq!(attached_body["snapshot"]["cols"], 80);
    assert_eq!(attached_body["snapshot"]["rows"], 24);
    assert!(
        attached_body["snapshot"]["data"]
            .as_str()
            .is_some_and(|snapshot| snapshot.contains("terminal-ready")),
        "attach should include one coherent snapshot"
    );
    assert_eq!(attached_body["tail"], json!([]));

    let stale_input = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/input"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "data": "echo stale\n"
        })),
    )
    .await;
    assert_eq!(stale_input.status(), StatusCode::FORBIDDEN);

    let takeover = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/take-input-lease"),
        Some(json!({ "browser_viewer_id": "browser-b" })),
    )
    .await;
    assert_eq!(takeover.status(), StatusCode::OK);
    assert_eq!(
        read_json(takeover).await["session"]["input_lease_holder"]["browser_id"],
        "browser-b"
    );

    let accepted_input = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/input"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "data": "echo second\n"
        })),
    )
    .await;
    assert_eq!(accepted_input.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.spawner.inputs(), vec![b"echo second\n".to_vec()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_reattach_from_cursor_returns_mandatory_snapshot() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let session_id = read_json(opened).await["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let initial = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let initial = read_json(initial).await;
    let cursor = initial["live_cursor"].clone();
    app.spawner.emit_output(b"retained-tail\n");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resumed = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 5000,
            "cursor": cursor
        })),
    )
    .await;

    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed = read_json(resumed).await;
    assert!(resumed["snapshot"].is_object());
    assert_eq!(
        resumed["snapshot"]["cursor"]["next_output_offset"],
        resumed["live_cursor"]["next_output_offset"]
    );
    assert!(
        resumed["snapshot"]["data"]
            .as_str()
            .is_some_and(|snapshot| snapshot.contains("retained-tail")),
        "reattach must restore current authority state instead of returning a visible tail-only replay"
    );
    assert_eq!(resumed["tail"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_activity_stream_reports_input_lease_takeover() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let attached = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/attach"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::OK);
    let attached = read_json(attached).await;
    let next_activity_sequence = attached["live_cursor"]["next_activity_sequence"]
        .as_u64()
        .expect("next activity sequence");
    let next_output_offset = attached["live_cursor"]["next_output_offset"]
        .as_u64()
        .expect("next output offset");

    let activity = request(
        &app.app,
        Method::GET,
        &format!(
            "/api/terminal/sessions/{session_id}/activity?browser_viewer_id=browser-a&next_activity_sequence={next_activity_sequence}&next_output_offset={next_output_offset}"
        ),
        None,
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let mut body = activity.into_body();

    let takeover = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/take-input-lease"),
        Some(json!({ "browser_viewer_id": "browser-b" })),
    )
    .await;
    assert_eq!(takeover.status(), StatusCode::OK);

    let frame = next_sse_json(&mut body).await;
    assert_eq!(frame["type"], "input_lease_changed");
    assert_eq!(frame["holder"]["browser_id"], "browser-b");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_activity_stream_rejects_an_unavailable_cursor() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let session_id = read_json(opened).await["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let response = request(
        &app.app,
        Method::GET,
        &format!(
            "/api/terminal/sessions/{session_id}/activity?browser_viewer_id=browser-a&next_activity_sequence=99&next_output_offset=99"
        ),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = read_json(response).await;
    assert!(body["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("cursor unavailable")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_activity_stream_reports_process_exit() {
    let app = build_app();
    let activity = request(
        &app.app,
        Method::GET,
        "/api/terminal/sessions/remote-session-1/activity?execution_device=remote-device&browser_viewer_id=browser-a",
        None,
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let mut body = activity.into_body();

    tokio::time::timeout(Duration::from_secs(1), async {
        while app.remote_activity_client.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote activity subscribe request");
    app.remote_activity_client
        .emit(TerminalActivityFrame::Exited {
            activity_sequence: 1,
        });

    let frame = next_sse_json(&mut body).await;
    assert_eq!(frame["type"], "exited");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_activity_stream_uses_remote_activity_bridge() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_activity_client = Arc::new(HttpFakeRemoteTerminalActivityClient::default());
    state.terminal_remote_activity_client = remote_activity_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let activity = request(
        &app,
        Method::GET,
        "/api/terminal/sessions/remote-session-1/activity?execution_device=remote-device&browser_viewer_id=browser-a",
        None,
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let mut body = activity.into_body();

    tokio::time::timeout(Duration::from_secs(1), async {
        while remote_activity_client.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote activity subscribe request");
    remote_activity_client.emit(TerminalActivityFrame::Output {
        activity_sequence: 0,
        start_offset: 0,
        end_offset: 12,
        data: b"remote-live\n".to_vec(),
    });

    let frame = next_sse_json(&mut body).await;
    assert_eq!(frame["type"], "output");
    assert_eq!(frame["activity_sequence"], 0);
    assert_eq!(frame["start_offset"], 0);
    assert_eq!(frame["end_offset"], 12);
    assert_eq!(
        BASE64_STANDARD
            .decode(frame["data_base64"].as_str().expect("output base64"))
            .expect("decode output"),
        b"remote-live\n"
    );

    let requests = remote_activity_client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source_device_id, source_device_id);
    assert_eq!(requests[0].target_device_id, "remote-device");
    assert_eq!(requests[0].session_id, "remote-session-1");
    assert_eq!(requests[0].viewer_id, "browser-a");
    assert_eq!(requests[0].cursor, TerminalStreamCursor::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_open_uses_remote_command_client_without_local_fallback() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let local_spawner = Arc::new(HttpFakeTerminalSpawner::new());
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        local_spawner.clone(),
    ));
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let opened = request(
        &app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000,
            "terminal_profile_id": "proot"
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    assert_eq!(
        read_json(opened).await["session"]["session_id"],
        "remote-session-1"
    );
    assert!(
        local_spawner.inputs().is_empty(),
        "remote open must not touch local terminal authority"
    );
    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    match &commands[0].action {
        TerminalDeviceAction::Open {
            terminal_profile_id,
            scrollback_lines,
            ..
        } => {
            assert_eq!(terminal_profile_id.as_deref(), Some("proot"));
            assert_eq!(*scrollback_lines, 12000);
        }
        other => panic!("expected terminal open command, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_attach_uses_remote_command_client_and_returns_canonical_state() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let local_spawner = Arc::new(HttpFakeTerminalSpawner::new());
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        local_spawner.clone(),
    ));
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let attached = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/attach",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 12000,
            "cursor": {
                "next_activity_sequence": 4,
                "next_output_offset": 32
            }
        })),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::OK);
    let body = read_json(attached).await;
    assert_eq!(body["session"]["session_id"], "remote-session-1");
    assert_eq!(body["snapshot"]["cursor"]["next_activity_sequence"], 4);
    assert_eq!(body["snapshot"]["cursor"]["next_output_offset"], 32);
    assert_eq!(body["live_cursor"]["next_activity_sequence"], 5);
    assert_eq!(body["live_cursor"]["next_output_offset"], 44);
    assert_eq!(body["tail"][0]["activity_sequence"], 4);
    assert_eq!(body["tail"][0]["start_offset"], 32);
    assert!(
        local_spawner.inputs().is_empty(),
        "remote attach must not touch local terminal authority"
    );

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::Attach {
            scrollback_lines: 12000,
            cursor: Some(TerminalStreamCursor {
                next_activity_sequence: 4,
                next_output_offset: 32,
            }),
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_and_remote_attach_share_one_canonical_contract() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    set_local_sync_device(&mut state);
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(HttpFakeTerminalSpawner::new()),
    ));
    state.terminal_remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let opened = request(
        &app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let local_session_id = read_json(opened).await["session"]["session_id"]
        .as_str()
        .expect("local session id")
        .to_string();
    let local = request(
        &app,
        Method::POST,
        &format!("/api/terminal/sessions/{local_session_id}/attach"),
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let remote = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/attach",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 5000
        })),
    )
    .await;

    assert_eq!(local.status(), StatusCode::OK);
    assert_eq!(remote.status(), StatusCode::OK);
    assert_canonical_terminal_attachment(&read_json(local).await);
    assert_canonical_terminal_attachment(&read_json(remote).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_snapshot_failure_never_falls_back_to_local_replay() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    set_local_sync_device(&mut state);
    state.terminal_remote_client = Arc::new(FailingRemoteTerminalClient);
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let response = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-missing-snapshot/attach",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "scrollback_lines": 5000
        })),
    )
    .await;
    let local_sessions = request(
        &app,
        Method::GET,
        "/api/terminal/sessions?execution_device=local",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(read_json(response).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("snapshot unavailable")));
    assert_eq!(read_json(local_sessions).await["sessions"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_scrollback_update_uses_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let updated = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/scrollback",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "scrollback_lines": 24000
        })),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::SetScrollback {
            scrollback_lines: 24000,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_rename_uses_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let renamed = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/rename",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "display_name": "Shared shell"
        })),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let body = read_json(renamed).await;
    assert_eq!(body["session"]["session_id"], "remote-session-1");
    assert_eq!(body["session"]["display_name"], "Shared shell");

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::Rename { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_list_uses_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let listed = request(
        &app,
        Method::GET,
        "/api/terminal/sessions?execution_device=remote-device",
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = read_json(listed).await;
    assert_eq!(body["sessions"][0]["session_id"], "remote-session-1");

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::ListSessions
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_takeover_and_input_use_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let takeover = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/take-input-lease",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b"
        })),
    )
    .await;
    assert_eq!(takeover.status(), StatusCode::OK);
    assert_eq!(
        read_json(takeover).await["session"]["input_lease_holder"]["browser_id"],
        "browser-b"
    );

    let input = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/input",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "data": "echo remote\n"
        })),
    )
    .await;
    assert_eq!(input.status(), StatusCode::NO_CONTENT);

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|command| {
        command.source_device_id == source_device_id && command.target_device_id == "remote-device"
    }));
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::TakeInputLease { .. }
    ));
    assert!(matches!(
        commands[1].action,
        TerminalDeviceAction::SendInput { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_input_lease_rejection_is_not_device_unavailable() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    set_local_sync_device(&mut state);
    state.terminal_remote_client = Arc::new(InputLeaseRejectedRemoteTerminalClient);
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let response = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/input",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "data": "echo remote\n"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        read_json(response).await["detail"],
        "terminal input lease is required"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_geometry_adapt_requires_input_lease() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_body = read_json(opened).await;
    let session_id = opened_body["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let stale = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/adapt-pty-geometry"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "cols": 100,
            "rows": 30
        })),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::FORBIDDEN);

    let adapted = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/adapt-pty-geometry"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "cols": 100,
            "rows": 30
        })),
    )
    .await;
    assert_eq!(adapted.status(), StatusCode::OK);
    let body = read_json(adapted).await;
    assert_eq!(body["session"]["pty_geometry"]["cols"], 100);
    assert_eq!(body["session"]["pty_geometry"]["rows"], 30);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_geometry_adapt_uses_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let adapted = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/adapt-pty-geometry",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "cols": 100,
            "rows": 30
        })),
    )
    .await;
    assert_eq!(adapted.status(), StatusCode::OK);
    let body = read_json(adapted).await;
    assert_eq!(body["session"]["pty_geometry"]["cols"], 100);
    assert_eq!(body["session"]["pty_geometry"]["rows"], 30);

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::AdaptPtyGeometry {
            cols: 100,
            rows: 30,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_terminal_palette_update_requires_input_lease() {
    let app = build_app();
    let opened = request(
        &app.app,
        Method::POST,
        "/api/terminal/sessions",
        Some(json!({
            "execution_device": { "kind": "local" },
            "browser_viewer_id": "browser-a",
            "scrollback_lines": 12000
        })),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let session_id = read_json(opened).await["session"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let palette = json!({
        "foreground": "#101418",
        "background": "#f8f9fa",
        "cursor": "#228be6"
    });

    let passive = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/palette"),
        Some(json!({
            "browser_viewer_id": "browser-b",
            "palette": palette
        })),
    )
    .await;
    assert_eq!(passive.status(), StatusCode::FORBIDDEN);

    let holder = request(
        &app.app,
        Method::POST,
        &format!("/api/terminal/sessions/{session_id}/palette"),
        Some(json!({
            "browser_viewer_id": "browser-a",
            "palette": palette
        })),
    )
    .await;
    assert_eq!(holder.status(), StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_palette_update_uses_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let updated = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/palette",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b",
            "palette": {
                "foreground": "#101418",
                "background": "#f8f9fa",
                "cursor": "#228be6"
            }
        })),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].source_device_id, source_device_id);
    assert_eq!(commands[0].target_device_id, "remote-device");
    assert!(matches!(
        &commands[0].action,
        TerminalDeviceAction::UpdatePalette {
            session_id,
            browser_viewer_id,
            palette: TerminalPalette {
                foreground,
                background,
                cursor,
            },
        } if session_id == "remote-session-1"
            && browser_viewer_id == "browser-b"
            && foreground == "#101418"
            && background == "#f8f9fa"
            && cursor == "#228be6"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_terminal_detach_and_kill_use_remote_command_client() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let source_device_id = set_local_sync_device(&mut state);
    let local_spawner = Arc::new(HttpFakeTerminalSpawner::new());
    state.terminal_authority = Arc::new(TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: tmp.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        local_spawner.clone(),
    ));
    let remote_client = Arc::new(HttpFakeRemoteTerminalClient::default());
    state.terminal_remote_client = remote_client.clone();
    let app = Router::new()
        .nest("/api", endpoints::terminal::router())
        .with_state(state);

    let detached = request(
        &app,
        Method::POST,
        "/api/terminal/sessions/remote-session-1/detach",
        Some(json!({
            "execution_device": { "kind": "remote", "device_id": "remote-device" },
            "browser_viewer_id": "browser-b"
        })),
    )
    .await;
    assert_eq!(detached.status(), StatusCode::NO_CONTENT);

    let killed = request(
        &app,
        Method::DELETE,
        "/api/terminal/sessions/remote-session-1?browser_viewer_id=browser-b&execution_device=remote-device",
        None,
    )
    .await;
    assert_eq!(killed.status(), StatusCode::NO_CONTENT);
    assert!(
        local_spawner.inputs().is_empty(),
        "remote lifecycle commands must not touch local terminal authority"
    );

    let commands = remote_client.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|command| {
        command.source_device_id == source_device_id && command.target_device_id == "remote-device"
    }));
    assert!(matches!(
        commands[0].action,
        TerminalDeviceAction::DetachViewer { .. }
    ));
    assert!(matches!(
        commands[1].action,
        TerminalDeviceAction::KillSession { .. }
    ));
}
