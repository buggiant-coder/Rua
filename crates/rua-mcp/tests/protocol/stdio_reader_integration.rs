use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mcp_manager::client::McpClient;
use mcp_manager::error::McpError;
use mcp_manager::types::{McpCallContext, McpContent};

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

fn config_with(scenario: &str, extras: &[(&str, &str)]) -> StdioConfig {
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("MOCK_SCENARIO".into(), scenario.into());
    for (k, v) in extras {
        env.insert((*k).into(), (*v).into());
    }
    StdioConfig {
        server_key: format!("mock-{scenario}"),
        server_name: "mock-mcp".into(),
        command: "python3".into(),
        args: vec![fixture_path().to_string_lossy().into_owned()],
        env,
        cwd: None,
        startup_timeout: Duration::from_secs(5),
        call_timeout: Duration::from_secs(5),
        stderr_log_prefix: None,
    }
}

fn text_of(content: &McpContent) -> &str {
    match content {
        McpContent::Text { text } => text.as_str(),
        _ => panic!("expected text content, got {content:?}"),
    }
}

#[tokio::test]
async fn handshake_succeeds() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping handshake_succeeds");
        return;
    }
    let client = StdioMcpClient::new(config_with("fifo", &[]));
    client.connect().await.expect("connect should succeed");
    let init = client
        .initialize_result()
        .await
        .expect("initialize_result should be cached");
    assert_eq!(init.server_name, "mock-mcp");
    assert_eq!(init.protocol_version, "2024-11-05");
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_requests_run_in_parallel() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping concurrent_requests_run_in_parallel");
        return;
    }
    let client = Arc::new(StdioMcpClient::new(config_with(
        "delay",
        &[("MOCK_DELAY_MS", "500")],
    )));
    client.connect().await.expect("connect");

    let start = Instant::now();
    let c1 = Arc::clone(&client);
    let c2 = Arc::clone(&client);
    let c3 = Arc::clone(&client);
    let r1 = tokio::spawn(async move {
        c1.call_tool(
            "echo",
            serde_json::json!({"msg": "one"}),
            McpCallContext::default(),
        )
        .await
    });
    let r2 = tokio::spawn(async move {
        c2.call_tool(
            "echo",
            serde_json::json!({"msg": "two"}),
            McpCallContext::default(),
        )
        .await
    });
    let r3 = tokio::spawn(async move {
        c3.call_tool(
            "echo",
            serde_json::json!({"msg": "three"}),
            McpCallContext::default(),
        )
        .await
    });

    let (a, b, c) = tokio::join!(r1, r2, r3);
    a.unwrap().expect("call one ok");
    b.unwrap().expect("call two ok");
    c.unwrap().expect("call three ok");

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1000),
        "concurrent calls took {elapsed:?} — expected ~500ms (parallel), got >1s (serial)",
    );

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn handles_out_of_order_responses() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping handles_out_of_order_responses");
        return;
    }
    let client = Arc::new(StdioMcpClient::new(config_with("reverse", &[])));
    client.connect().await.expect("connect");

    let c1 = Arc::clone(&client);
    let r1 = tokio::spawn(async move {
        c1.call_tool(
            "echo",
            serde_json::json!({"msg": "first"}),
            McpCallContext::default(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let c2 = Arc::clone(&client);
    let r2 = tokio::spawn(async move {
        c2.call_tool(
            "echo",
            serde_json::json!({"msg": "second"}),
            McpCallContext::default(),
        )
        .await
    });

    let (a, b) = tokio::join!(r1, r2);
    let resp1 = a.unwrap().expect("call first ok");
    let resp2 = b.unwrap().expect("call second ok");
    assert_eq!(text_of(&resp1.content[0]), "first");
    assert_eq!(text_of(&resp2.content[0]), "second");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn drops_unrequested_notifications() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping drops_unrequested_notifications");
        return;
    }
    let client = StdioMcpClient::new(config_with("interleave", &[]));
    client.connect().await.expect("connect");

    for i in 0..3 {
        let r = client
            .call_tool(
                "echo",
                serde_json::json!({"msg": format!("call-{i}")}),
                McpCallContext::default(),
            )
            .await
            .expect("call should succeed despite surrounding notifications");
        assert_eq!(text_of(&r.content[0]), &format!("call-{i}"));
    }

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_requests_fail_on_server_crash() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping pending_requests_fail_on_server_crash");
        return;
    }
    let client = Arc::new(StdioMcpClient::new(config_with(
        "crash",
        &[("MOCK_CRASH_AFTER", "0")],
    )));

    client.connect().await.expect("connect");

    let result = client
        .call_tool(
            "echo",
            serde_json::json!({"msg": "x"}),
            McpCallContext::default(),
        )
        .await;
    let err = result.expect_err("call should fail when server crashes");
    assert!(
        matches!(err, McpError::TransportClosed { .. }),
        "expected TransportClosed, got {err:?}",
    );

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_removes_from_pending() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping cancellation_removes_from_pending");
        return;
    }
    let client = Arc::new(StdioMcpClient::new(config_with(
        "delay",
        &[("MOCK_DELAY_MS", "1500")],
    )));
    client.connect().await.expect("connect");

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let ctx = McpCallContext {
        cancel: Some(cancel_rx),
        ..Default::default()
    };

    let c = Arc::clone(&client);
    let handle = tokio::spawn(async move {
        c.call_tool("echo", serde_json::json!({"msg": "cancel-me"}), ctx)
            .await
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel_tx.send(true).unwrap();

    let result = handle.await.unwrap();
    let err = result.expect_err("cancelled call should error");
    assert!(
        matches!(err, McpError::Cancelled { .. }),
        "expected Cancelled, got {err:?}",
    );

    let r2 = client
        .call_tool(
            "echo",
            serde_json::json!({"msg": "after-cancel"}),
            McpCallContext::default(),
        )
        .await
        .expect("post-cancel call should succeed");
    assert_eq!(text_of(&r2.content[0]), "after-cancel");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeout_removes_from_pending() {
    if !python3_available() {
        eprintln!("python3 not on PATH — skipping timeout_removes_from_pending");
        return;
    }
    let client = Arc::new(StdioMcpClient::new(config_with(
        "delay",
        &[("MOCK_DELAY_MS", "1500")],
    )));
    client.connect().await.expect("connect");

    let ctx = McpCallContext {
        timeout: Some(Duration::from_millis(150)),
        ..Default::default()
    };
    let result = client
        .call_tool("echo", serde_json::json!({"msg": "to"}), ctx)
        .await;
    let err = result.expect_err("call should time out");
    assert!(
        matches!(err, McpError::CallTimeout { .. }),
        "expected CallTimeout, got {err:?}",
    );

    let ctx2 = McpCallContext {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    };
    let r2 = client
        .call_tool("echo", serde_json::json!({"msg": "after-timeout"}), ctx2)
        .await
        .expect("post-timeout call should succeed");
    assert_eq!(text_of(&r2.content[0]), "after-timeout");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_connect_is_single_flight_and_does_not_restart_ready_child() {
    assert!(python3_available(), "MCP protocol fixture requires Python");
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("starts");
    let client = StdioMcpClient::new(config_with(
        "fifo",
        &[("MOCK_START_LOG", log.to_str().unwrap())],
    ));
    let (a, b) = tokio::join!(client.connect(), client.connect());
    a.unwrap();
    b.unwrap();
    client.connect().await.unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupted_partial_write_requires_reconnect_and_deadline_includes_pipe_wait() {
    let client = StdioMcpClient::new(config_with("blocked_stdin", &[]));
    client.connect().await.unwrap();
    let started = Instant::now();
    let result = client
        .send_request(
            "tools/call",
            serde_json::json!({"payload":"x".repeat(4*1024*1024)}),
            Duration::from_millis(150),
        )
        .await;
    assert!(
        matches!(result, Err(McpError::CallTimeout { .. })),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!client.status().state.is_available());
    let next = client.ping().await;
    assert!(
        matches!(next, Err(McpError::TransportClosed { .. })),
        "partial JSON must never be joined to a new request: {next:?}"
    );
    assert!(client
        .send_notification("notifications/cancelled", serde_json::json!({}))
        .await
        .is_err());
    tokio::time::timeout(Duration::from_secs(3), client.shutdown())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_call_future_does_not_block_or_misroute_later_response() {
    let client = StdioMcpClient::new(config_with("delay", &[("MOCK_DELAY_MS", "150")]));
    client.connect().await.unwrap();
    let timed = tokio::time::timeout(
        Duration::from_millis(30),
        client.call_tool(
            "echo",
            serde_json::json!({"msg":"abandoned"}),
            McpCallContext::default(),
        ),
    )
    .await;
    assert!(timed.is_err());
    let next = client
        .call_tool(
            "echo",
            serde_json::json!({"msg":"next"}),
            McpCallContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(next.text(), "next");
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropped_call_notifies_server_without_disconnect_or_replay() {
    assert!(python3_available());
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("requests");
    let client = Arc::new(StdioMcpClient::new(config_with(
        "delay",
        &[
            ("MOCK_DELAY_MS", "500"),
            ("MOCK_REQUEST_LOG", log.to_str().unwrap()),
        ],
    )));
    client.connect().await.unwrap();
    let running = client.clone();
    let call = tokio::spawn(async move {
        running
            .call_tool(
                "echo",
                serde_json::json!({"msg":"cancelled"}),
                McpCallContext::default(),
            )
            .await
    });
    for _ in 0..100 {
        if std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("tools/call")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("tools/call"));
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    for _ in 0..100 {
        if std::fs::read_to_string(&log)
            .unwrap()
            .contains("notifications/cancelled")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let requests = std::fs::read_to_string(&log).unwrap();
    assert_eq!(requests.matches("tools/call").count(), 1);
    assert_eq!(requests.matches("notifications/cancelled").count(), 1);
    let result = client
        .call_tool(
            "echo",
            serde_json::json!({"msg":"still connected"}),
            McpCallContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(text_of(&result.content[0]), "still connected");
    client.shutdown().await.unwrap();
}
