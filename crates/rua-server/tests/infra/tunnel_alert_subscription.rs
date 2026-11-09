#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_channel::core::content::{ChannelTarget, OutgoingMessage};

use rua_server::endpoints::tunnel_status;
use rua_server::state::TunnelAlertState;
use rua_server::AppState;

struct BroadcastMockChannel {
    id: String,
    call_count: AtomicUsize,
}

impl BroadcastMockChannel {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            call_count: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl rua_channel::core::Channel for BroadcastMockChannel {
    fn id(&self) -> &String {
        &self.id
    }
    async fn start(&self, _process: Arc<dyn rua_channel::core::Process>) -> rua_core::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> rua_core::Result<()> {
        Ok(())
    }
    async fn send(&self, _target: ChannelTarget, message: OutgoingMessage) -> rua_core::Result<()> {
        let _ = message;
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn build_request_from_native(
        &self,
        _native: serde_json::Value,
    ) -> rua_core::Result<rua_channel::core::AgentRequest> {
        Err(rua_core::RuaError::Validation {
            message: "outbound test channel".into(),
        })
    }
}

fn write_fixture_script(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
}

async fn build_app(tmp: &TempDir) -> (Router, AppState) {
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).unwrap();
    let data_dir = tmp.path().to_path_buf();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(data_dir, rua_server::ServerProfile::Test)
    })
    .await
    .unwrap();
    let app = Router::new()
        .nest("/api", tunnel_status::router())
        .with_state(state.clone());
    (app, state)
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

#[tokio::test(flavor = "multi_thread")]
async fn all_retries_failed_sets_state_failed() {
    let tmp = TempDir::new().unwrap();
    let tunnel_dir = tmp.path().join("tunnel");
    let state_file = tmp.path().join("crash_state");

    write_fixture_script(
        &tunnel_dir,
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE='{state}'\n\
             if [ ! -f \"$STATE\" ]; then\n\
                 touch \"$STATE\"\n\
                 echo 'https://alert-test.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'no url here' >&2\n\
                 exit 1\n\
             fi\n",
            state = state_file.display(),
        ),
    );

    let tunnel_driver: Arc<tokio::sync::Mutex<Option<rua_tunnel::CloudflareTunnelDriver>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let alert_state = Arc::new(tokio::sync::RwLock::new(TunnelAlertState::default()));
    let channel_manager = Arc::new(rua_channel::core::ChannelManager::new());

    let mgr = rua_tunnel::BinaryManager::new(tunnel_dir.clone());
    let driver = rua_tunnel::CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![
        Duration::from_millis(50),
        Duration::from_millis(50),
        Duration::from_millis(50),
    ]);
    {
        let mut guard = tunnel_driver.lock().await;
        *guard = Some(driver);
    }

    {
        let td = Arc::clone(&tunnel_driver);
        let als = Arc::clone(&alert_state);
        let cm = Arc::clone(&channel_manager);
        tokio::spawn(async move {
            rua_server::state::tunnel_event_subscriber_loop(td, als, cm).await;
        });
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let script_path = tunnel_dir.join("cloudflared");
    {
        let guard = tunnel_driver.lock().await;
        let driver = guard.as_ref().unwrap();
        driver.start_tunnel(&script_path, 8080).await.unwrap();
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    let alert = alert_state.read().await;
    assert_eq!(alert.state, "failed");
    assert!(alert.last_failure_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn restarted_event_sets_state_connected() {
    let tmp = TempDir::new().unwrap();
    let tunnel_dir = tmp.path().join("tunnel");
    let state_file = tmp.path().join("restart_state");

    write_fixture_script(
        &tunnel_dir,
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE='{state}'\n\
             if [ ! -f \"$STATE\" ]; then\n\
                 touch \"$STATE\"\n\
                 echo 'https://initial.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'https://restarted.trycloudflare.com' >&2\n\
                 sleep 60\n\
             fi\n",
            state = state_file.display(),
        ),
    );

    let tunnel_driver: Arc<tokio::sync::Mutex<Option<rua_tunnel::CloudflareTunnelDriver>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let alert_state = Arc::new(tokio::sync::RwLock::new(TunnelAlertState::default()));
    let channel_manager = Arc::new(rua_channel::core::ChannelManager::new());

    let mgr = rua_tunnel::BinaryManager::new(tunnel_dir.clone());
    let driver = rua_tunnel::CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(150),
    ]);
    {
        let mut guard = tunnel_driver.lock().await;
        *guard = Some(driver);
    }

    {
        let td = Arc::clone(&tunnel_driver);
        let als = Arc::clone(&alert_state);
        let cm = Arc::clone(&channel_manager);
        tokio::spawn(async move {
            rua_server::state::tunnel_event_subscriber_loop(td, als, cm).await;
        });
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let script_path = tunnel_dir.join("cloudflared");
    {
        let guard = tunnel_driver.lock().await;
        let driver = guard.as_ref().unwrap();
        driver.start_tunnel(&script_path, 8080).await.unwrap();
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    let alert = alert_state.read().await;
    assert_eq!(alert.state, "connected");
    drop(alert);

    let guard = tunnel_driver.lock().await;
    if let Some(ref d) = *guard {
        d.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_endpoint_includes_state_field() {
    let tmp = TempDir::new().unwrap();
    let (app, _state) = build_app(&tmp).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/tunnel/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "disconnected");
    assert!(body.get("state").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn all_retries_failed_broadcasts_to_all_channels() {
    let tmp = TempDir::new().unwrap();
    let tunnel_dir = tmp.path().join("tunnel");
    let state_file = tmp.path().join("bcast_state");

    write_fixture_script(
        &tunnel_dir,
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE='{state}'\n\
             if [ ! -f \"$STATE\" ]; then\n\
                 touch \"$STATE\"\n\
                 echo 'https://bcast-test.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'no url here' >&2\n\
                 exit 1\n\
             fi\n",
            state = state_file.display(),
        ),
    );

    let ch_dingtalk = Arc::new(BroadcastMockChannel::new("dingtalk"));
    let ch_wecom = Arc::new(BroadcastMockChannel::new("wecom"));
    let ch_feishu = Arc::new(BroadcastMockChannel::new("feishu"));

    let channel_manager = Arc::new(rua_channel::core::ChannelManager::new());
    channel_manager.register(ch_dingtalk.clone()).await.unwrap();
    channel_manager.register(ch_wecom.clone()).await.unwrap();
    channel_manager.register(ch_feishu.clone()).await.unwrap();

    let tunnel_driver: Arc<tokio::sync::Mutex<Option<rua_tunnel::CloudflareTunnelDriver>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let alert_state = Arc::new(tokio::sync::RwLock::new(TunnelAlertState::default()));

    let mgr = rua_tunnel::BinaryManager::new(tunnel_dir.clone());
    let driver = rua_tunnel::CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![
        Duration::from_millis(50),
        Duration::from_millis(50),
        Duration::from_millis(50),
    ]);
    {
        let mut guard = tunnel_driver.lock().await;
        *guard = Some(driver);
    }

    {
        let td = Arc::clone(&tunnel_driver);
        let als = Arc::clone(&alert_state);
        let cm = Arc::clone(&channel_manager);
        tokio::spawn(async move {
            rua_server::state::tunnel_event_subscriber_loop(td, als, cm).await;
        });
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let script_path = tunnel_dir.join("cloudflared");
    {
        let guard = tunnel_driver.lock().await;
        let driver = guard.as_ref().unwrap();
        driver.start_tunnel(&script_path, 8080).await.unwrap();
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(
        ch_dingtalk.calls() >= 1,
        "dingtalk should receive broadcast"
    );
    assert!(ch_wecom.calls() >= 1, "wecom should receive broadcast");
    assert!(ch_feishu.calls() >= 1, "feishu should receive broadcast");

    let alert = alert_state.read().await;
    assert_eq!(alert.state, "failed");
}
