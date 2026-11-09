use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rua_acp::service::{AcpService, TurnMode};
use rua_config::types::acp::{AcpAgentConfig, AcpConfig};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

struct ProviderRequest {
    request_line: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

struct AcpRuntimeBridgeRequest {
    health_request_line: String,
    turn: rua_acp::remote::AcpRuntimeTurnRequest,
}

fn write_agent_with_model(workspace: &std::path::Path, agent_id: &str) {
    let agent_workspace = workspace.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&agent_workspace).expect("agent workspace");
    std::fs::write(
        agent_workspace.join("agent.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": agent_id,
            "name": "Self Called Agent",
            "workspace_dir": agent_workspace,
            "default_model": {
                "provider_id": "acp-test-provider",
                "model": "acp-test-model"
            },
            "running": {
                "max_iters": 2
            }
        }))
        .unwrap(),
    )
    .expect("agent.json");
    let mut config = rua_config::RootConfig::default();
    config.agents.active_agent = agent_id.to_string();
    config.agents.profiles.insert(
        agent_id.to_string(),
        serde_json::json!({ "id": agent_id, "workspace_dir": agent_workspace }),
    );
    rua_config::save_config(&config, &workspace.join("config.json")).expect("root config");
}

fn write_provider_config(secret_dir: &std::path::Path, base_url: &str) {
    let providers_dir = secret_dir.join("providers").join("providers");
    std::fs::create_dir_all(&providers_dir).expect("provider dir");
    std::fs::write(
        providers_dir.join("acp-test-provider.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "acp-test-provider",
            "name": "ACP Test Provider",
            "base_url": base_url,
            "api_key": "sk-acp-test",
            "chat_model": "OpenAIChatModel",
            "models": [{
                "id": "acp-test-model",
                "name": "ACP Test Model"
            }],
            "support_model_discovery": false,
            "support_connection_check": false,
            "is_custom": true,
            "require_api_key": false
        }))
        .unwrap(),
    )
    .expect("provider config");
    std::fs::write(
        providers_dir.join("acp-override-provider.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "acp-override-provider",
            "name": "ACP Override Provider",
            "base_url": base_url,
            "api_key": "sk-acp-override",
            "chat_model": "OpenAIChatModel",
            "models": [{
                "id": "acp-override-model",
                "name": "ACP Override Model"
            }],
            "support_model_discovery": false,
            "support_connection_check": false,
            "is_custom": true,
            "require_api_key": false
        }))
        .unwrap(),
    )
    .expect("override provider config");
    std::fs::write(
        secret_dir.join("providers").join("default_model.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "provider_id": "acp-test-provider",
            "model": "acp-test-model",
            "source": "acp-process-test"
        }))
        .unwrap(),
    )
    .expect("default model");
}

async fn spawn_openai_sse_server() -> (String, oneshot::Receiver<ProviderRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let base_url = format!(
        "http://{}/v1",
        listener.local_addr().expect("mock provider addr")
    );
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept provider request");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 2048];
        let mut header_end = None;
        let mut content_length = 0_usize;
        loop {
            let read = socket
                .read(&mut buffer)
                .await
                .expect("read provider request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if header_end.is_none() {
                header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n");
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                }
            }
            if header_end.is_some_and(|end| bytes.len() >= end + 4 + content_length) {
                break;
            }
        }

        let request_line = String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let header_end = header_end.expect("provider request headers");
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let authorization = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_string())
        });
        let body_start = header_end + 4;
        let body = serde_json::from_slice(&bytes[body_start..body_start + content_length])
            .expect("provider request JSON body");
        let _ = request_tx.send(ProviderRequest {
            request_line,
            authorization,
            body,
        });

        let sse = concat!(
            "data: {\"id\":\"chatcmpl-acp\",\"object\":\"chat.completion.chunk\",",
            "\"created\":0,\"model\":\"acp-test-model\",\"choices\":[{\"index\":0,",
            "\"delta\":{\"role\":\"assistant\",\"content\":\"hello \"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-acp\",\"object\":\"chat.completion.chunk\",",
            "\"created\":0,\"model\":\"acp-test-model\",\"choices\":[{\"index\":0,",
            "\"delta\":{\"content\":\"from rua\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .await
            .expect("write provider headers");
        socket
            .write_all(format!("{:X}\r\n", sse.len()).as_bytes())
            .await
            .expect("write provider chunk size");
        socket
            .write_all(sse.as_bytes())
            .await
            .expect("write provider body");
        socket
            .write_all(b"\r\n0\r\n\r\n")
            .await
            .expect("finish provider response");
    });

    (base_url, request_rx)
}

async fn spawn_acp_runtime_bridge_server() -> (u16, oneshot::Receiver<AcpRuntimeBridgeRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ACP runtime health server");
    let port = listener.local_addr().expect("ACP runtime address").port();
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut health_socket, _) = listener.accept().await.expect("accept health request");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = health_socket
                .read(&mut buffer)
                .await
                .expect("read health request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let request_line = String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        health_socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
            )
            .await
            .expect("write health response");

        let (mut turn_socket, _) = listener.accept().await.expect("accept turn request");
        let mut bytes = Vec::new();
        let mut header_end = None;
        let mut content_length = 0_usize;
        loop {
            let read = turn_socket
                .read(&mut buffer)
                .await
                .expect("read turn request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if header_end.is_none() {
                header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n");
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                }
            }
            if header_end.is_some_and(|end| bytes.len() >= end + 4 + content_length) {
                break;
            }
        }
        let body_start = header_end.expect("turn request headers") + 4;
        let turn = serde_json::from_slice::<rua_acp::remote::AcpRuntimeTurnRequest>(
            &bytes[body_start..body_start + content_length],
        )
        .expect("turn request JSON");
        let _ = request_tx.send(AcpRuntimeBridgeRequest {
            health_request_line: request_line,
            turn,
        });

        let stream_body = [
            rua_acp::remote::AcpRuntimeStreamItem::Event {
                event: Box::new(RuntimeEvent::TextDelta {
                    delta: "hello from resident server".to_string(),
                    message_id: None,
                }),
            },
            rua_acp::remote::AcpRuntimeStreamItem::Event {
                event: Box::new(RuntimeEvent::End {
                    finish_reason: Some("stop".to_string()),
                }),
            },
        ]
        .into_iter()
        .map(|item| serde_json::to_string(&item).expect("runtime stream item"))
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        turn_socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    stream_body.len(),
                    stream_body
                )
                .as_bytes(),
            )
            .await
            .expect("write turn response");
    });

    (port, request_rx)
}

fn send_json(stdin: &mut std::process::ChildStdin, value: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    stdin.flush().unwrap();
}

fn recv_response(rx: &mpsc::Receiver<serde_json::Value>, id: i64) -> serde_json::Value {
    recv_response_with_messages(rx, id).0
}

fn recv_response_with_messages(
    rx: &mpsc::Receiver<serde_json::Value>,
    id: i64,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut messages = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let message = rx
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("timed out waiting for ACP response {id}: {error}"));
        if message.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
            return (message, messages);
        }
        messages.push(message);
    }
}

#[test]
fn acp_subcommand_exists_in_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_rua"))
        .arg("--help")
        .output()
        .expect("failed to run rua --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("acp"),
        "rua --help should list the 'acp' subcommand, got:\n{stdout}"
    );
}

#[test]
fn acp_help_shows_agent_and_model_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_rua"))
        .args(["acp", "--help"])
        .output()
        .expect("failed to run rua acp --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--agent"),
        "rua acp --help should list --agent flag, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--provider-id"),
        "rua acp --help should list --provider-id flag, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--model-id"),
        "rua acp --help should list --model-id flag, got:\n{stdout}"
    );
}

#[test]
fn acp_provider_and_model_flags_must_be_passed_together() {
    for args in [
        ["acp", "--provider-id", "acp-override-provider"],
        ["acp", "--model-id", "acp-override-model"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rua"))
            .args(args)
            .output()
            .expect("run rua acp with incomplete model selection");

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("required"),
            "incomplete ACP model selection should be rejected by clap, got:\n{stderr}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_uses_running_server_for_initialize_and_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let (port, bridge_request) = spawn_acp_runtime_bridge_server().await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_rua"))
        .args([
            "--rua-root",
            temp.path().to_str().expect("workspace path"),
            "acp",
            "--agent",
            "default",
            "--provider-id",
            "idealab-responses",
            "--model-id",
            "gpt-5.6-sol",
        ])
        .env("RUA_HOST", "127.0.0.1")
        .env("RUA_PORT", port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn remote ACP CLI");
    let mut stdin = child.stdin.take().expect("ACP stdin");
    let stdout = child.stdout.take().expect("ACP stdout");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read ACP stdout");
            let value: serde_json::Value =
                serde_json::from_str(&line).expect("ACP stdout must contain JSON-RPC only");
            if tx.send(value).is_err() {
                break;
            }
        }
    });

    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientInfo": {"name": "rua-remote-test", "version": "0.0.0"}
            }
        }),
    );
    let initialize = recv_response(&rx, 1);
    assert_eq!(initialize["result"]["protocolVersion"], 1);

    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": {
                "cwd": project,
                "mcpServers": [{
                    "name": "filesystem",
                    "command": "/usr/bin/example-mcp",
                    "args": ["--stdio"],
                    "env": [{
                        "name": "TOKEN",
                        "value": "secret"
                    }]
                }]
            }
        }),
    );
    let session = recv_response(&rx, 2);
    let session_id = session["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string();
    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{
                    "type": "text",
                    "text": "reply through the resident server"
                }]
            }
        }),
    );
    let (prompt, messages) = recv_response_with_messages(&rx, 3);
    assert_eq!(prompt["result"]["stopReason"], "end_turn");
    assert!(messages
        .iter()
        .any(|message| message.to_string().contains("hello from resident server")));

    let bridge_request = bridge_request.await.expect("ACP runtime bridge request");
    assert_eq!(
        bridge_request.health_request_line,
        "GET /api/agents/default/acp/runtime/health HTTP/1.1"
    );
    assert_eq!(bridge_request.turn.workspace_dir, project);
    assert_eq!(
        bridge_request.turn.request.metadata["chat_model_slot"],
        serde_json::json!({
            "provider_id": "idealab-responses",
            "model": "gpt-5.6-sol",
        })
    );
    let mcp_config = bridge_request
        .turn
        .mcp_config
        .expect("remote ACP MCP config");
    assert_eq!(
        mcp_config["clients"]["acp_0_filesystem"]["transport"],
        "stdio"
    );
    assert_eq!(
        mcp_config["clients"]["acp_0_filesystem"]["command"],
        "/usr/bin/example-mcp"
    );
    assert_eq!(
        mcp_config["clients"]["acp_0_filesystem"]["env"]["TOKEN"],
        "secret"
    );

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    reader.join().expect("ACP reader");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_acp_client_can_prompt_rua_agent_over_stdio() {
    let workspace = tempfile::tempdir().expect("workspace");
    let project = workspace.path().join("project");
    let secret_dir = workspace.path().join("secret");
    let agent_id = "external-agent";
    std::fs::create_dir_all(&project).expect("project dir");
    let (base_url, provider_request) = spawn_openai_sse_server().await;
    write_agent_with_model(workspace.path(), agent_id);
    write_provider_config(&secret_dir, &base_url);
    let mut child = Command::new(env!("CARGO_BIN_EXE_rua"))
        .args([
            "--rua-root",
            workspace.path().to_str().unwrap(),
            "acp",
            "--agent",
            agent_id,
            "--provider-id",
            "acp-override-provider",
            "--model-id",
            "acp-override-model",
        ])
        .env("RUA_SECRET_DIR", &secret_dir)
        .env("RUA_HOST", "127.0.0.1")
        .env("RUA_PORT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rua acp");
    let mut stdin = child.stdin.take().expect("ACP stdin");
    let stdout = child.stdout.take().expect("ACP stdout");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read ACP stdout");
            let value: serde_json::Value =
                serde_json::from_str(&line).expect("ACP stdout must contain JSON-RPC only");
            if tx.send(value).is_err() {
                break;
            }
        }
    });

    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientInfo": {"name": "rua-test", "version": "0.0.0"}
            }
        }),
    );
    let initialize = recv_response(&rx, 1);
    assert_eq!(initialize["result"]["protocolVersion"], 1);
    assert_eq!(initialize["result"]["agentInfo"]["name"], "rua");
    assert_eq!(
        initialize["result"]["agentCapabilities"]["loadSession"],
        false
    );

    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": {
                "cwd": project,
                "mcpServers": []
            }
        }),
    );
    let session = recv_response(&rx, 2);
    assert!(session["result"]["sessionId"].as_str().is_some());
    assert_eq!(
        session["result"]["_meta"]["rua.agent"],
        serde_json::Value::String(agent_id.to_string())
    );
    let session_id = session["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string();

    send_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{
                    "type": "text",
                    "text": "reply with the configured test response"
                }]
            }
        }),
    );
    let (prompt, messages) = recv_response_with_messages(&rx, 3);
    assert_eq!(prompt["result"]["stopReason"], "end_turn");
    let updates = messages
        .iter()
        .filter(|message| message["method"] == "session/update")
        .map(serde_json::Value::to_string)
        .collect::<String>();
    assert!(
        updates.contains("hello "),
        "missing first text update: {updates}"
    );
    assert!(
        updates.contains("from rua"),
        "missing second text update: {updates}"
    );
    let provider_request = provider_request
        .await
        .expect("mock provider should receive one request");
    assert_eq!(
        provider_request.request_line,
        "POST /v1/chat/completions HTTP/1.1"
    );
    assert_eq!(
        provider_request.authorization.as_deref(),
        Some("Bearer sk-acp-override")
    );
    assert_eq!(provider_request.body["model"], "acp-override-model");

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    reader.join().expect("ACP reader");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rua_acp_client_can_run_a_turn_against_rua_agent_server() {
    let workspace = tempfile::tempdir().expect("workspace");
    let project = workspace.path().join("project");
    let secret_dir = workspace.path().join("secret");
    let agent_id = "self-called-agent";
    std::fs::create_dir_all(&project).expect("project dir");
    let (base_url, provider_request) = spawn_openai_sse_server().await;
    write_agent_with_model(workspace.path(), agent_id);
    write_provider_config(&secret_dir, &base_url);

    let runner_name = "rua-self";
    let runner = AcpAgentConfig {
        enabled: true,
        command: env!("CARGO_BIN_EXE_rua").to_string(),
        args: vec![
            "--rua-root".to_string(),
            workspace.path().to_string_lossy().into_owned(),
            "acp".to_string(),
            "--agent".to_string(),
            agent_id.to_string(),
        ],
        env: HashMap::from([
            (
                "RUA_SECRET_DIR".to_string(),
                secret_dir.to_string_lossy().into_owned(),
            ),
            ("RUA_HOST".to_string(), "127.0.0.1".to_string()),
            ("RUA_PORT".to_string(), "1".to_string()),
        ]),
        trusted: true,
        tool_parse_mode: Default::default(),
        stdio_buffer_limit_bytes: Some(50 * 1024 * 1024),
        extra: serde_json::Map::new(),
    };
    let cache_dir = workspace.path().join("cache");
    let service = AcpService::with_runtime_cache_dir(
        std::sync::Arc::new(AcpConfig {
            agents: HashMap::from([(runner_name.to_string(), runner)]),
            extra: serde_json::Map::new(),
        }),
        project.to_string_lossy().into_owned(),
        cache_dir,
        None,
        "caller-agent".to_string(),
    );
    let chat_id = SessionId::new("rua-acp-self-call");

    let mut events = tokio::time::timeout(
        Duration::from_secs(60),
        service.run_turn(
            chat_id,
            runner_name,
            "reply with the configured test response",
            TurnMode::Start,
            Some(project.to_string_lossy().into_owned()),
        ),
    )
    .await
    .expect("Rua ACP self-call setup timeout")
    .expect("Rua ACP client should initialize, create a session, and start a turn");

    let mut text = String::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("Rua ACP self-call event timeout")
            .expect("Rua ACP self-call stream closed before end");
        match event {
            RuntimeEvent::TextDelta { delta, .. } => text.push_str(&delta),
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("Rua ACP self-call failed: {error}"),
            _ => {}
        }
    }

    assert_eq!(text, "hello from rua");
    let provider_request = provider_request
        .await
        .expect("mock provider should receive one request");
    assert_eq!(
        provider_request.request_line,
        "POST /v1/chat/completions HTTP/1.1"
    );
    assert_eq!(
        provider_request.authorization.as_deref(),
        Some("Bearer sk-acp-test")
    );
    assert_eq!(provider_request.body["model"], "acp-test-model");
    service.close_all().await;
}
