use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId, McpTransportConfig, ToolFilter};
use rua_mcp::manager::{McpClientStatus, McpManagerImpl};
use rua_mcp::{McpMobilePolicy, PlatformProfile};

fn initialize_response(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mock-mcp", "version": "1.0" }
        }
    })
    .to_string()
}

fn tools_list_response(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": "mock_tool",
                    "description": "A mock tool for testing",
                    "inputSchema": { "type": "object", "properties": {} }
                }
            ]
        }
    })
    .to_string()
}

async fn spawn_mock_server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&call_count);

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                let request = String::from_utf8_lossy(&buf[..n]);

                let body = request.split("\r\n\r\n").nth(1).unwrap_or("");

                let req_id = serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v.get("id")?.as_u64())
                    .unwrap_or(1);

                let method = serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v.get("method")?.as_str().map(String::from))
                    .unwrap_or_default();

                let count = counter.fetch_add(1, Ordering::SeqCst);

                if method == "initialize" && count == 0 {
                    let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if method == "initialize" {
                    let body = initialize_response(req_id);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: test-session\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else if method == "tools/list" {
                    let body = tools_list_response(req_id);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    (port, call_count)
}

#[tokio::test]
async fn retry_after_500_then_list_tools_returns_tools() {
    let (port, _call_count) = spawn_mock_server().await;

    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_retry_interval(Duration::from_secs(1));

    let mut config = McpConfig::default();
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "allow_private_ip".to_string(),
        serde_json::Value::Bool(true),
    );
    config.clients.insert(
        McpServerId::new("retry-test"),
        McpServerConfig {
            name: "retry-test".to_string(),
            description: String::new(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url: format!("http://127.0.0.1:{port}/"),
                headers: Default::default(),
                timeout_secs: 5,
            },
            auto_connect: Default::default(),
            call_timeout_secs: 10,
            max_concurrency: 4,
            oauth: None,
            disabled_tools: Vec::new(),
            extra,
        },
    );

    manager.load_config(config).await.expect("load_config");

    let server_id = McpServerId::new("retry-test");

    let snapshot_status = || {
        let mgr = &manager;
        let id = server_id.clone();
        async move {
            let servers = mgr.servers.read().await;
            let entry = servers.get(&id).unwrap();
            let status = entry.status.read().await;
            status.clone()
        }
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status = snapshot_status().await;
        if matches!(status, McpClientStatus::Failed(_)) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for Failed status after initial 500, got {:?}",
            status
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status = snapshot_status().await;
        if matches!(status, McpClientStatus::Ready) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for Ready status after retry, got {:?}",
            status
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let tools = manager
        .list_tools(ToolFilter::default())
        .await
        .expect("list_tools");
    assert!(
        !tools.is_empty(),
        "expected non-empty tools after successful retry"
    );
    assert_eq!(tools[0].name, "mock_tool");
}
