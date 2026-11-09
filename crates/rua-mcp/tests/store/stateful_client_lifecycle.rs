use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mcp_manager::client::McpClient;

use rua_mcp::transports::stdio::{StdioConfig, StdioMcpClient};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp_server.py")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn lifecycle_config() -> StdioConfig {
    StdioConfig {
        server_key: "lifecycle-test".into(),
        server_name: "mock-mcp".into(),
        command: "python3".into(),
        args: vec![fixture_path().to_string_lossy().into_owned()],
        env: {
            let mut m = BTreeMap::new();
            m.insert("MOCK_SCENARIO".into(), "fifo".into());
            m
        },
        cwd: None,
        startup_timeout: Duration::from_secs(5),
        call_timeout: Duration::from_secs(5),
        stderr_log_prefix: None,
    }
}

#[tokio::test]
async fn start_then_shutdown_within_200ms() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping");
        return;
    }

    let client = StdioMcpClient::new(lifecycle_config());
    assert!(!client.is_running());

    client.start().await.expect("start should succeed");
    assert!(client.is_running());

    let t0 = Instant::now();
    client
        .request_shutdown()
        .await
        .expect("request_shutdown should succeed");
    let elapsed = t0.elapsed();

    assert!(!client.is_running());
    assert!(
        elapsed < Duration::from_millis(200),
        "shutdown took {elapsed:?}, expected < 200ms"
    );
}

#[tokio::test]
async fn reconnect_replaces_old_session() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping");
        return;
    }

    let client = StdioMcpClient::new(lifecycle_config());

    client.start().await.expect("first start");
    assert!(client.is_running());

    let init1 = client
        .initialize_result()
        .await
        .expect("init result after first start");

    client.request_shutdown().await.expect("first shutdown");
    assert!(!client.is_running());

    client.start().await.expect("second start");
    assert!(client.is_running());

    let init2 = client
        .initialize_result()
        .await
        .expect("init result after reconnect");

    assert_eq!(init1.server_name, init2.server_name);

    client.request_shutdown().await.expect("final shutdown");
    assert!(!client.is_running());
}

#[tokio::test]
async fn running_watch_reflects_state() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping");
        return;
    }

    let client = StdioMcpClient::new(lifecycle_config());
    let mut rx = client.running_watch();

    assert!(!*rx.borrow());

    client.start().await.expect("start");
    rx.changed().await.expect("should see true");
    assert!(*rx.borrow());

    client.request_shutdown().await.expect("shutdown");
    rx.changed().await.expect("should see false");
    assert!(!*rx.borrow());
}

#[tokio::test]
async fn startup_ready_notifies_waiter() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping");
        return;
    }

    let client = StdioMcpClient::new(lifecycle_config());
    let notify = client.startup_ready();

    let notified = notify.notified();
    tokio::pin!(notified);

    let start_fut = client.start();
    tokio::pin!(start_fut);

    tokio::select! {
        res = &mut start_fut => {
            res.expect("start should succeed");

            tokio::time::timeout(Duration::from_millis(50), notified)
                .await
                .expect("startup_ready should have been notified");
        }
    }

    client.request_shutdown().await.expect("cleanup");
}

#[tokio::test]
async fn shutdown_requested_notifies_listener() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping");
        return;
    }

    let client = StdioMcpClient::new(lifecycle_config());
    client.start().await.expect("start");

    let shutdown_signal = client.shutdown_requested();
    let notified = shutdown_signal.notified();
    tokio::pin!(notified);

    let shutdown_fut = client.request_shutdown();
    tokio::pin!(shutdown_fut);

    tokio::select! {
        res = &mut shutdown_fut => {
            res.expect("shutdown should succeed");
            tokio::time::timeout(Duration::from_millis(50), notified)
                .await
                .expect("shutdown_requested should have been notified");
        }
    }
}
