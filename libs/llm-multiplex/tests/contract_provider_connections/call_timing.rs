use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{AgentEvent, ChatRequest, ModelDialogueItem, Role};
use futures::StreamExt;
use llm_multiplex::debug_capture::{
    self, ActiveCapture, CaptureTrace, CHAT_ADDRESS_METADATA_KEY, MODEL_CALL_ID_METADATA_KEY,
};
use llm_multiplex::{
    ChatModel, LlmError, LlmRateLimiter, ModelInfo, OpenAiCompatibleConfig,
    OpenAiResponsesProvider, Provider, RateLimitChatModel, RateLimitConfig,
};
use rua_core::ChatAddress;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

struct Capture {
    base: PathBuf,
    root: PathBuf,
    address: ChatAddress,
}

impl Capture {
    fn new(enabled: bool) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let base = std::env::temp_dir().join(format!("rua-call-timing-{id}"));
        let root = base.join("capture");
        let address = ChatAddress::project(format!("project-{id}"), format!("chat-{id}"));
        if enabled {
            debug_capture::register_capture(
                address.clone(),
                ActiveCapture {
                    capture_id: id,
                    capture_path: root.clone(),
                },
            );
        }
        Self {
            base,
            root,
            address,
        }
    }

    fn request(&self) -> ChatRequest {
        let mut request = ChatRequest::streaming(vec![ModelDialogueItem::text(
            Role::User,
            "Keep the complete long context. ".repeat(4096),
        )]);
        request
            .metadata
            .insert(CHAT_ADDRESS_METADATA_KEY.into(), json!(self.address));
        request
            .metadata
            .insert(MODEL_CALL_ID_METADATA_KEY.into(), json!("call-1"));
        request
    }

    fn traces(&self, stage: &str) -> Vec<Value> {
        debug_capture::flush_capture(&self.root, Duration::from_secs(5)).unwrap();
        std::fs::read_dir(self.root.join("calls/call-1"))
            .expect("call capture directory")
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{stage}-timing-"))
            })
            .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
            .collect()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        debug_capture::unregister_capture(&self.address);
        let _ = debug_capture::flush_capture(&self.root, Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn process_shutdown_drains_capture_and_cancels_samplers_without_cancelling_http() {
    // Shutdown closes the process registry permanently. Isolate it from the
    // other capture tests, just as a real server process would be isolated.
    const CHILD: &str = "RUA_TEST_CAPTURE_SHUTDOWN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "call_timing::process_shutdown_drains_capture_and_cancels_samplers_without_cancelling_http", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let capture = Capture::new(true);
            let (release, gate) = tokio::sync::oneshot::channel();
            let mut response = successful_response(None);
            response.headers_gate = Some(gate);
            let server = Server::start(vec![response]).await;
            let mut stream = server.model().chat_stream(capture.request());
            stream.next().await.unwrap().unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), stream.next())
                    .await
                    .is_err()
            );
            debug_capture::shutdown_capture_writers(Duration::from_secs(5))
                .await
                .unwrap();
            assert!(debug_capture::active_capture(&capture.address).is_none());
            assert_eq!(capture.traces("network")[0]["outcome"], "capture_disabled");
            let status_path = std::fs::read_dir(&capture.root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("capture-writer-")
                })
                .unwrap();
            let status: Value =
                serde_json::from_slice(&std::fs::read(status_path).unwrap()).unwrap();
            assert_eq!(status["status"]["closed"], true);
            assert_eq!(status["status"]["pending_artifacts"], 0);
            assert_eq!(status["status"]["failed_artifacts"], 0);
            release.send(()).unwrap();
            assert!(stream.collect::<Vec<_>>().await.iter().all(Result::is_ok));
            let late = Capture::new(true);
            assert!(debug_capture::active_capture(&late.address).is_none());
        });
}

struct Response {
    status: u16,
    chunks: Vec<String>,
    keep_open: bool,
    first_chunk_gate: Option<tokio::sync::oneshot::Receiver<()>>,
    headers_gate: Option<tokio::sync::oneshot::Receiver<()>>,
    keep_connection: bool,
}

impl Response {
    fn sse(chunks: Vec<String>) -> Self {
        Self {
            status: 200,
            chunks,
            keep_open: false,
            first_chunk_gate: None,
            headers_gate: None,
            keep_connection: false,
        }
    }
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    raw_requests: Arc<Mutex<Vec<Vec<u8>>>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let raw_requests = Arc::new(Mutex::new(Vec::new()));
        let raw_captured = raw_requests.clone();
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = stopped.cancelled() => {}
                _ = async move {
                    let mut reused = None;
                    for response in responses {
                        let mut socket = match reused.take() {
                            Some(socket) => socket,
                            None => listener.accept().await.unwrap().0,
                        };
                        let mut bytes = Vec::new();
                        let mut buffer = [0_u8; 8192];
                        loop {
                            let count = socket.read(&mut buffer).await.unwrap();
                            assert_ne!(count, 0);
                            bytes.extend_from_slice(&buffer[..count]);
                            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                                let headers = String::from_utf8_lossy(&bytes[..end]);
                                let length: usize = headers.lines().find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                                }).unwrap();
                                if bytes.len() >= end + 4 + length {
                                    captured.lock().unwrap().push(serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap());
                                    raw_captured.lock().unwrap().push(bytes[end + 4..end + 4 + length].to_vec());
                                    break;
                                }
                            }
                        }
                        if let Some(gate) = response.headers_gate { let _ = gate.await; }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        let connection = if response.keep_connection { "keep-alive" } else { "close" };
                        let location = if response.status == 307 { "location: /redirect-target\r\n" } else { "" };
                        let headers = format!(
                            "HTTP/1.1 {} OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nx-request-id: upstream-123\r\nretry-after: 1\r\nset-cookie: secret-cookie\r\nauthorization: secret-response-auth\r\nconnection: {connection}\r\n{location}\r\n", response.status
                        );
                        socket.write_all(headers.as_bytes()).await.unwrap();
                        if let Some(gate) = response.first_chunk_gate { let _ = gate.await; }
                        for chunk in response.chunks {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            socket.write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes()).await.unwrap();
                            socket.flush().await.unwrap();
                        }
                        if response.keep_open {
                            std::future::pending::<()>().await;
                        }
                        socket.write_all(b"0\r\n\r\n").await.unwrap();
                        if response.keep_connection { reused = Some(socket); }
                    }
                } => {}
            }
        });
        Self {
            url,
            requests,
            raw_requests,
            cancel,
            task,
        }
    }

    fn model(&self) -> Box<dyn ChatModel> {
        llm_multiplex::dashscope::create_dashscope_provider_with_url(
            &self.url,
            "secret-api-key",
            vec![ModelInfo {
                id: "qwen-test".into(),
                name: "Qwen test".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            }],
            serde_json::Map::from_iter([("enable_thinking".into(), json!(true))]),
        )
        .unwrap()
        .create_chat_model("qwen-test")
        .unwrap()
    }

    fn responses_model(&self) -> Box<dyn ChatModel> {
        OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
            provider_id: "dashscope".into(),
            provider_name: "DashScope".into(),
            base_url: self.url.clone(),
            api_key: Some("secret-api-key".into()),
            api_keys: Vec::new(),
            headers: serde_json::Map::from_iter([(
                "x-dashscope-session-cache".into(),
                json!("enable"),
            )]),
            models: vec![ModelInfo {
                id: "qwen3.8-max".into(),
                name: "Qwen 3.8 Max".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            }],
            timeout_secs: 30,
            generate_kwargs: serde_json::Map::new(),
            support_model_discovery: false,
            support_connection_check: false,
            is_local: false,
        })
        .unwrap()
        .create_chat_model("qwen3.8-max")
        .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn successful_response(cached_tokens: Option<u64>) -> Response {
    let mut usage = json!({"prompt_tokens": 550000, "completion_tokens": 12, "completion_tokens_details": {"reasoning_tokens": 4}});
    if let Some(cached_tokens) = cached_tokens {
        usage["prompt_tokens_details"] = json!({"cached_tokens": cached_tokens});
    }
    Response::sse(vec![
        ": keepalive\n\n".into(),
        sse(
            json!({"id":"response-123", "choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}),
        ),
        sse(json!({"choices":[{"index":0,"delta":{"reasoning_content":"thinking"}}]})),
        sse(json!({"choices":[{"index":0,"delta":{"content":"answer"}}]})),
        sse(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"tool-1","function":{"name":"Read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        sse(json!({"choices":[],"usage":usage})),
        "data: [DONE]\n\n".into(),
    ])
}

#[tokio::test]
async fn temporarily_unavailable_dashscope_response_session_retries_incrementally() {
    let server = Server::start(vec![
        Response {
            status: 400,
            chunks: vec![json!({"error":{"message":"previous_response_id not found"}}).to_string()],
            keep_open: false,
            first_chunk_gate: None,
            headers_gate: None,
            keep_connection: false,
        },
        Response::sse(vec![sse(json!({
            "type": "response.completed",
            "response": {
                "id": "response-new",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 1,
                    "total_tokens": 11
                }
            }
        }))]),
    ])
    .await;
    let model = server.responses_model();
    let request = dashscope_session_request("response-pending");

    let events = model.chat_stream(request).collect::<Vec<_>>().await;
    assert!(events.into_iter().all(|event| event.is_ok()));

    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["previous_response_id"], "response-pending");
    assert_eq!(requests[1]["previous_response_id"], "response-pending");
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn expired_dashscope_response_session_replays_full_history_after_retries() {
    let mut responses = (0..5)
        .map(|_| Response {
            status: 400,
            chunks: vec![json!({"error":{"message":"previous_response_id expired"}}).to_string()],
            keep_open: false,
            first_chunk_gate: None,
            headers_gate: None,
            keep_connection: false,
        })
        .collect::<Vec<_>>();
    responses.push(Response::sse(vec![sse(json!({
        "type": "response.completed",
        "response": {
            "id": "response-new",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 1,
                "total_tokens": 11
            }
        }
    }))]));
    let server = Server::start(responses).await;
    let model = server.responses_model();
    let request = dashscope_session_request("response-expired");

    let events = model.chat_stream(request).collect::<Vec<_>>().await;
    assert!(events.into_iter().all(|event| event.is_ok()));

    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests[..5]
        .iter()
        .all(|request| request["previous_response_id"] == "response-expired"));
    assert!(requests[5].get("previous_response_id").is_none());
    assert_eq!(requests[5]["input"].as_array().unwrap().len(), 3);
}

fn dashscope_session_request(response_id: &str) -> ChatRequest {
    let first_user = ModelDialogueItem::text(Role::User, "first");
    let input = json!([{"role":"user","content":"first"}]);
    let input_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&input).unwrap()));
    let mut assistant = ModelDialogueItem::text(Role::Assistant, "answer");
    assistant.metadata.insert(
        "rua_responses_session".into(),
        json!({
            "provider_id": "dashscope",
            "model": "qwen3.8-max",
            "response_id": response_id,
            "request_input_sha256": input_hash,
            "created_at_unix_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }),
    );
    ChatRequest::streaming(vec![
        first_user,
        assistant,
        ModelDialogueItem::text(Role::User, "second"),
    ])
    .with_model("qwen3.8-max")
}

#[tokio::test]
async fn responses_session_fingerprint_matches_full_input_at_tool_and_override_boundaries() {
    use agent_types::{ContentPart, ToolCall, ToolResult};

    for variant in [
        "text",
        "tool",
        "mixed_result_anchor",
        "input_override",
        "messages_override",
        "older_anchor",
        "omni",
    ] {
        let server = Server::start(vec![Response::sse(vec![sse(json!({
            "type": "response.completed", "response": {"id": "response-next", "output": []},
        }))])])
        .await;
        let provider = OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
            provider_id: "dashscope".into(),
            provider_name: "test".into(),
            base_url: server.url.clone(),
            api_key: None,
            api_keys: Vec::new(),
            headers: Default::default(),
            models: Vec::new(),
            timeout_secs: 5,
            generate_kwargs: Default::default(),
            support_model_discovery: false,
            support_connection_check: false,
            is_local: true,
        })
        .unwrap();
        let mut request = dashscope_session_request("response-ready");
        if variant == "tool" || variant == "mixed_result_anchor" {
            let call = ToolCall::new("tool-call", "read", json!({"路径":"文件.md"}));
            request.messages[1]
                .content
                .push(ContentPart::ToolUse { call });
            let result = ContentPart::ToolResult {
                result: ToolResult::text("tool-call", "完整结果\n"),
            };
            if variant == "tool" {
                request.messages[2].content.insert(0, result);
            } else {
                request.messages[1].content.push(result);
            }
        }
        if variant == "input_override" {
            request.provider_options.insert(
                "input".into(),
                json!([{"role":"user","content":"override"}]),
            );
        }
        if variant == "omni" {
            request.model = Some("qwen-omni".into());
        }
        if variant == "messages_override" {
            request.provider_options.insert(
                "messages".into(),
                json!([{"role":"user","content":"override messages"}]),
            );
        }
        let mut prefix = request.clone();
        prefix.messages.truncate(1);
        let prefix_input = provider.build_request_body(&prefix)["input"].clone();
        request.messages[1].metadata["rua_responses_session"]["request_input_sha256"] =
            json!(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&prefix_input).unwrap())
            ));
        request.messages[1].metadata["rua_responses_session"]["model"] = json!(request.model);
        if variant == "older_anchor" {
            let mut stale = request.messages[1].clone();
            stale.metadata["rua_responses_session"]["response_id"] = json!("stale-response");
            stale.metadata["rua_responses_session"]["request_input_sha256"] = json!("stale");
            request.messages.push(stale);
            request
                .messages
                .push(ModelDialogueItem::text(Role::User, "last question"));
        }
        let expected_wire = provider.build_request_body(&request);
        let mut full = request.clone();
        for message in &mut full.messages {
            message.metadata.remove("rua_responses_session");
        }
        let full_body = provider.build_request_body(&full);
        let expected_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&full_body["input"]).unwrap())
        );
        let model = provider
            .create_chat_model(request.model.as_deref().unwrap())
            .unwrap();
        let events = model.chat_stream(request).collect::<Vec<_>>().await;
        let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();
        let metadata = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::End { metadata, .. } => Some(metadata),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            metadata["rua_responses_session"]["request_input_sha256"], expected_hash,
            "{variant}"
        );
        assert_eq!(
            server.requests.lock().unwrap()[0],
            expected_wire,
            "{variant}"
        );
    }
}

#[test]
fn unpolled_responses_stream_does_not_prepare_a_full_fallback_request() {
    let capture = Capture::new(true);
    let provider = OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: "dashscope".into(),
        provider_name: "DashScope".into(),
        base_url: "http://127.0.0.1:1".into(),
        api_key: None,
        api_keys: Vec::new(),
        headers: Default::default(),
        models: Vec::new(),
        timeout_secs: 1,
        generate_kwargs: Default::default(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: true,
    })
    .unwrap();
    let model = provider.create_chat_model("qwen3.8-max").unwrap();
    let mut request = dashscope_session_request("response-ready");
    request.metadata = capture.request().metadata;
    let stream = model.chat_stream(request);
    drop(stream);

    let trace = capture.traces("provider").remove(0);
    assert_eq!(trace["outcome"], "dropped");
    assert_eq!(trace["data"]["responses_session_reused"], true);
    assert_eq!(trace["data"]["responses_input_items"], 1);
    assert_eq!(trace["data"]["responses_full_fallback_built"], false);
    assert!(trace["milestones_ms"].get("http_send_started").is_none());
}

#[tokio::test]
async fn capture_wait_checkpoints_coalesce_adjacent_writes_but_finish_flushes_all_marks() {
    let capture = Capture::new(true);
    let mut trace = CaptureTrace::new(&capture.request().metadata, "iteration");
    trace
        .measure_local("preparation_started", "preparation_finished", async {})
        .await;
    trace
        .measure_local("before_hook_started", "before_hook_finished", async {})
        .await;
    trace
        .measure_local("projection_started", "projection_finished", async {})
        .await;
    trace.finish("completed");

    let saved = capture.traces("iteration").remove(0);
    assert_eq!(saved["outcome"], "completed");
    for milestone in [
        "preparation_started",
        "before_hook_started",
        "before_hook_finished",
        "projection_started",
    ] {
        assert!(saved["milestones_ms"][milestone].is_number(), "{milestone}");
    }
    assert!(
        saved["data"]["coalesced_trace_checkpoints"]
            .as_u64()
            .unwrap()
            >= 2
    );
}

#[tokio::test(start_paused = true)]
async fn slow_local_phase_is_checkpointed_while_still_pending() {
    let capture = Capture::new(true);
    let mut trace = CaptureTrace::new(&capture.request().metadata, "iteration");
    {
        let pending = trace.measure_local(
            "hook_started",
            "hook_finished",
            std::future::pending::<()>(),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("pending phase completed"),
            _ = tokio::time::sleep(Duration::from_millis(150)) => {}
        }
        let saved = capture.traces("iteration").remove(0);
        assert_eq!(saved["outcome"], "in_progress");
        assert!(saved["milestones_ms"]["hook_started"].is_number());
        assert!(saved["milestones_ms"].get("hook_finished").is_none());
    }
    drop(trace);
    assert_eq!(capture.traces("iteration")[0]["outcome"], "dropped");
}

#[tokio::test]
async fn captures_stream_phases_usage_and_consumer_pauses_without_changing_context() {
    let capture = Capture::new(true);
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(Some(480000));
    response.first_chunk_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let original = "Keep the complete long context. ".repeat(65_536);
    let mut request = capture.request();
    request.messages = vec![ModelDialogueItem::text(Role::User, original.clone())];
    let mut stream = server.model().chat_stream(request);
    let mut events = Vec::new();
    events.push(stream.next().await.unwrap().unwrap());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.next())
                    .await
                    .is_err()
            );
            if !capture.traces("provider")[0]["milestones_ms"]["response_headers"].is_null() {
                break;
            }
        }
    })
    .await
    .unwrap();
    // The stream is now waiting for a body that the server cannot send yet.
    assert!(
        tokio::time::timeout(Duration::from_millis(60), stream.next())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    while let Some(event) = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
    {
        let event = event.unwrap();
        if matches!(event, AgentEvent::ThinkingDelta { .. }) {
            let traces = capture.traces("provider");
            assert_eq!(
                traces.len(),
                1,
                "a pending call must already have timing evidence"
            );
            assert_eq!(traces[0]["outcome"], "in_progress");
            assert!(traces[0]["milestones_ms"]["first_text_delta"].is_null());
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        let terminal = matches!(event, AgentEvent::End { .. });
        events.push(event);
        if terminal {
            break;
        }
    }
    drop(stream);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "answer")));
    let traces = capture.traces("provider");
    assert_eq!(traces.len(), 1);
    let trace = &traces[0];
    assert_eq!(trace["outcome"], "completed");
    assert_eq!(trace["model_call_id"], "call-1");
    let phases = [
        "body_built",
        "request_capture_enqueued",
        "request_serialized",
        "http_send_started",
        "response_headers",
        "first_response_byte",
        "first_sse_data",
        "first_thinking_delta",
        "first_text_delta",
        "first_tool_call_delta",
        "first_usage",
        "provider_finished",
    ];
    let times: Vec<_> = phases
        .iter()
        .map(|name| {
            trace["milestones_ms"][name]
                .as_f64()
                .unwrap_or_else(|| panic!("missing {name}: {trace}"))
        })
        .collect();
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(times[4] - times[3] >= 20.0);
    assert!(times[5] - times[4] >= 20.0);
    assert!(trace["data"]["consumer_pause_ms"].as_f64().unwrap() >= 55.0);
    assert!(trace["data"]["stream_wait_ms"].as_f64().unwrap() >= 20.0);
    assert_eq!(
        trace["data"]["response_headers"]["x-request-id"],
        "upstream-123"
    );
    assert_eq!(trace["data"]["response_id"], "response-123");
    assert_eq!(trace["data"]["usage"]["cached_tokens"], 480000);
    assert_eq!(
        trace["data"]["raw_usage"]["prompt_tokens_details"]["cached_tokens"],
        480000
    );
    let artifact = capture
        .root
        .join("calls/call-1")
        .join(trace["data"]["request_artifact"].as_str().unwrap());
    let request: Value = serde_json::from_slice(&std::fs::read(artifact).unwrap()).unwrap();
    assert_eq!(request["body"], server.requests.lock().unwrap()[0]);
    assert_eq!(request["body"]["messages"][0]["content"], original);
    let serialized = trace.to_string();
    assert!(
        serialized.len() < 16384,
        "timing evidence must not copy message bodies"
    );
    for secret in ["secret-cookie", "secret-response-auth", "secret-api-key"] {
        assert!(!serialized.contains(secret));
    }
}

#[tokio::test]
async fn retries_have_separate_traces_and_absent_cache_usage_is_not_zero() {
    let capture = Capture::new(true);
    let server = Server::start(vec![
        Response {
            status: 429,
            chunks: vec![json!({"error":{"message":"rate limited"}}).to_string()],
            keep_open: false,
            first_chunk_gate: None,
            headers_gate: None,
            keep_connection: false,
        },
        successful_response(None),
    ])
    .await;
    let model = server.model();
    let first: Vec<_> = model.chat_stream(capture.request()).collect().await;
    assert!(first
        .iter()
        .any(|event| matches!(event, Err(LlmError::RateLimit { .. }))));
    let second: Vec<_> = model.chat_stream(capture.request()).collect().await;
    assert!(second.iter().all(Result::is_ok));
    let traces = capture.traces("provider");
    assert_eq!(traces.len(), 2);
    assert_ne!(traces[0]["trace_id"], traces[1]["trace_id"]);
    let failed = traces
        .iter()
        .find(|trace| trace["outcome"] == "error")
        .unwrap();
    assert_eq!(failed["data"]["http_status"], 429);
    assert_eq!(failed["data"]["error_category"], "rate_limit");
    assert!(failed["milestones_ms"]["first_text_delta"].is_null());
    let completed = traces
        .iter()
        .find(|trace| trace["outcome"] == "completed")
        .unwrap();
    assert!(completed["data"]["usage"]["cached_tokens"].is_null());
    assert!(completed["data"]["raw_usage"]
        .get("prompt_tokens_details")
        .is_none());
}

#[tokio::test]
async fn dropping_a_pending_stream_records_interruption_not_success() {
    let capture = Capture::new(true);
    let mut response = successful_response(None);
    response.chunks.truncate(3);
    response.keep_open = true;
    let server = Server::start(vec![response]).await;
    let mut stream = server.model().chat_stream(capture.request());
    while !matches!(
        stream.next().await.unwrap().unwrap(),
        AgentEvent::ThinkingDelta { .. }
    ) {}
    drop(stream);
    let traces = capture.traces("provider");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0]["outcome"], "dropped");
    assert!(traces[0]["milestones_ms"]["provider_finished"].is_null());
}

#[tokio::test]
async fn eof_before_provider_finish_records_an_error() {
    let capture = Capture::new(true);
    let mut response = successful_response(None);
    response.chunks.truncate(3);
    let server = Server::start(vec![response]).await;
    let events: Vec<_> = server
        .model()
        .chat_stream(capture.request())
        .collect()
        .await;
    assert!(events.iter().any(Result::is_err));
    let traces = capture.traces("provider");
    assert_eq!(traces[0]["outcome"], "error");
    assert_eq!(traces[0]["data"]["error_kind"], "incomplete_stream");
    assert!(traces[0]["milestones_ms"]["http_eof"].is_number());
}

#[tokio::test]
async fn dropping_an_unpolled_stream_does_not_send_a_request_or_record_success() {
    let capture = Capture::new(true);
    let server = Server::start(vec![successful_response(None)]).await;
    drop(server.model().chat_stream(capture.request()));
    let traces = capture.traces("provider");
    assert_eq!(traces[0]["outcome"], "dropped");
    assert!(traces[0]["milestones_ms"]["http_send_started"].is_null());
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn capture_disabled_or_unwritable_does_not_change_the_stream() {
    for enabled in [false, true] {
        let capture = Capture::new(enabled);
        if enabled {
            std::fs::create_dir_all(&capture.base).unwrap();
            std::fs::File::create(&capture.root)
                .unwrap()
                .sync_all()
                .unwrap();
        }
        let server = Server::start(vec![successful_response(None)]).await;
        let events: Vec<_> = server
            .model()
            .chat_stream(capture.request())
            .collect()
            .await;
        assert!(events.iter().all(Result::is_ok));
        assert!(events
            .iter()
            .any(|event| matches!(event, Ok(AgentEvent::End { .. }))));
        if !enabled {
            assert!(!capture.root.exists());
        }
    }
}

#[tokio::test]
async fn local_rate_limit_wait_is_recorded_separately_from_http_wait() {
    let capture = Capture::new(true);
    let server = Server::start(vec![successful_response(None)]).await;
    let limiter = Arc::new(LlmRateLimiter::new(&RateLimitConfig {
        max_concurrent: 1,
        max_qpm: 0,
        pause_seconds: 0.0,
        jitter_range: 0.0,
        acquire_timeout: 3.0,
    }));
    limiter.acquire().await.unwrap();
    let model = RateLimitChatModel::new(server.model(), limiter.clone());
    let mut stream = model.chat_stream(capture.request());
    assert!(
        tokio::time::timeout(Duration::from_millis(60), stream.next())
            .await
            .is_err()
    );
    limiter.release();
    let events: Vec<_> = stream.collect().await;
    assert!(events.iter().all(Result::is_ok));
    let traces = capture.traces("rate-limit");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0]["outcome"], "completed");
    let phases = &traces[0]["milestones_ms"];
    assert!(
        phases["acquired"].as_f64().unwrap() - phases["acquire_started"].as_f64().unwrap() >= 55.0
    );
}

#[tokio::test]
async fn connection_setup_is_visible_while_waiting_for_response_headers() {
    let capture = Capture::new(true);
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(None);
    response.headers_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let mut stream = server.model().chat_stream(capture.request());
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        AgentEvent::Start { .. }
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.next())
                    .await
                    .is_err()
            );
            if capture.traces("provider")[0]["milestones_ms"]["new_connection_established"]
                .is_number()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let pending = capture.traces("provider").remove(0);
    assert_eq!(pending["outcome"], "in_progress");
    assert!(pending["milestones_ms"]["response_headers"].is_null());
    assert_eq!(
        pending["data"]["http_transport"]["connections"][0]["outcome"],
        "completed"
    );
    // Body consumption is not proof that TLS/socket upload has completed.
    assert_eq!(
        pending["data"]["http_transport"]["upload_timing"],
        "unavailable"
    );
    assert!(pending["milestones_ms"]["request_body_sent"].is_null());
    // A stalled response must not be confused with a stalled local executor.
    assert!(
        tokio::time::timeout(Duration::from_millis(1250), stream.next())
            .await
            .is_err()
    );
    let waiting = capture.traces("provider").remove(0);
    assert!(waiting["milestones_ms"]["http_wait_1s"].is_number());
    assert!(
        waiting["data"]["http_transport"]["scheduler_probe_ticks"]
            .as_u64()
            .unwrap()
            >= 1
    );
    release.send(()).unwrap();
    let events: Vec<_> = stream.collect().await;
    assert!(events.iter().all(Result::is_ok));
    let completed = capture.traces("provider").remove(0);
    assert_eq!(
        completed["data"]["http_transport"]["http_version"],
        "HTTP/1.1"
    );
    assert_eq!(
        completed["data"]["http_transport"]["connection_attempts"],
        1
    );
    assert_eq!(
        completed["data"]["http_transport"]["connection_origin"],
        "created_for_request"
    );
    assert!(completed["data"]["http_transport"]["connections"][0]["finished_ms"].is_number());
    assert!(
        completed["data"]["http_transport"]["scheduler_probe_ticks"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

async fn consume_raw(transport: &llm_multiplex::HttpTransport, capture: &Capture) {
    let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
    let body =
        json!({"messages": [{"role": "user", "content": "same complete request"}], "stream": true});
    let events: Vec<_> = transport
        .post_stream_bytes_traced(
            "/chat/completions",
            serde_json::to_vec(&body).unwrap().into(),
            &mut trace,
        )
        .await
        .unwrap()
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));
    trace.finish("completed");
}

#[tokio::test]
async fn dns_and_connection_reuse_are_observed_without_disabling_the_pool() {
    let capture = Capture::new(true);
    let mut first = successful_response(None);
    first.keep_connection = true;
    let server = Server::start(vec![first, successful_response(None)]).await;
    let transport = llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(
        server.url.replace("127.0.0.1", "localhost"),
        "test",
    ))
    .unwrap();
    consume_raw(&transport, &capture).await;
    consume_raw(&transport, &capture).await;
    let mut traces = capture.traces("provider");
    traces.sort_by_key(|trace| trace["started_at_unix_ms"].as_u64().unwrap());
    let first = &traces[0]["data"]["http_transport"];
    let second = &traces[1]["data"]["http_transport"];
    assert_eq!(first["dns_attempts"], 1);
    assert_eq!(first["dns"][0]["outcome"], "completed");
    assert_eq!(first["connection_origin"], "created_for_request");
    assert_eq!(second["connection_origin"], "pooled_or_shared");
    assert_eq!(second["connection_attempts"], 0);
    assert_eq!(second["dns_attempts"], 0);
    assert_eq!(first["local_addr"], second["local_addr"]);
    assert!(traces[1]["milestones_ms"]["new_connection_established"].is_null());
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn concurrent_calls_do_not_share_connection_or_dns_measurements() {
    let first = Capture::new(true);
    let second = Capture::new(true);
    let server = Server::start(vec![successful_response(None), successful_response(None)]).await;
    let transport = llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(
        server.url.replace("127.0.0.1", "localhost"),
        "test",
    ))
    .unwrap();
    tokio::join!(
        consume_raw(&transport, &first),
        consume_raw(&transport, &second)
    );
    for capture in [&first, &second] {
        let provider = capture.traces("provider").remove(0);
        let network = capture.traces("network").remove(0);
        assert_eq!(network["trace_id"], provider["trace_id"]);
        assert_eq!(
            network["confirmed_connection"]["local_addr"],
            provider["data"]["http_transport"]["local_addr"]
        );
        assert_eq!(network["counter_scope"], "socket_lifetime_not_http_request");
        for sample in network["samples"].as_array().unwrap() {
            if sample["attribution"] == "response_confirmed" {
                assert_eq!(
                    sample["local_addr"],
                    network["confirmed_connection"]["local_addr"]
                );
            }
        }
    }
    let first = first.traces("provider").remove(0);
    let second = second.traces("provider").remove(0);
    for trace in [&first, &second] {
        assert_eq!(trace["data"]["http_transport"]["connection_attempts"], 1);
        assert_eq!(trace["data"]["http_transport"]["dns_attempts"], 1);
        assert_eq!(
            trace["data"]["http_transport"]["connection_origin"],
            "created_for_request"
        );
    }
    assert_ne!(
        first["data"]["http_transport"]["local_addr"],
        second["data"]["http_transport"]["local_addr"]
    );
}

#[tokio::test]
async fn tracing_keeps_the_request_body_replayable_across_http_redirects() {
    let capture = Capture::new(true);
    let mut redirect = Response::sse(Vec::new());
    redirect.status = 307;
    let server = Server::start(vec![redirect, successful_response(None)]).await;
    let events: Vec<_> = server
        .model()
        .chat_stream(capture.request())
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    let trace = capture.traces("provider").remove(0);
    assert_eq!(trace["data"]["http_transport"]["connection_attempts"], 2);
    assert_eq!(
        trace["data"]["http_transport"]["connection_origin"],
        "created_for_request"
    );
    assert_eq!(trace["outcome"], "completed");
}

#[tokio::test]
async fn dropping_a_request_waiting_for_headers_keeps_transport_evidence() {
    let capture = Capture::new(true);
    let (_release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(None);
    response.headers_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let mut stream = server.model().chat_stream(capture.request());
    stream.next().await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let _ = tokio::time::timeout(Duration::from_millis(20), stream.next()).await;
            if capture.traces("provider")[0]["milestones_ms"]["new_connection_established"]
                .is_number()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(stream);
    let trace = capture.traces("provider").remove(0);
    assert_eq!(trace["outcome"], "dropped");
    assert_eq!(
        trace["data"]["http_transport"]["connections"][0]["outcome"],
        "completed"
    );
    assert!(trace["milestones_ms"]["response_headers"].is_null());
    let stopped = capture.traces("network").remove(0);
    assert_eq!(stopped["outcome"], "dropped");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::task::yield_now().await;
    assert_eq!(capture.traces("network")[0], stopped);
}

#[tokio::test]
async fn raw_replay_preserves_json_bytes_including_order_and_whitespace() {
    let capture = Capture::new(true);
    let mut redirect = Response::sse(Vec::new());
    redirect.status = 307;
    let server = Server::start(vec![redirect, successful_response(None)]).await;
    let transport =
        llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(&server.url, "test"))
            .unwrap();
    let original = bytes::Bytes::from_static(br#"{ "z": 1, "stream": true, "messages": [{"role":"user","content":"complete context"}], "a": 2 }"#);
    let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
    let events: Vec<_> = transport
        .post_stream_bytes_traced("/chat/completions", original.clone(), &mut trace)
        .await
        .unwrap()
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));
    trace.finish("completed");
    assert_eq!(
        server.raw_requests.lock().unwrap()[0].as_slice(),
        original.as_ref()
    );
    assert_eq!(
        server.raw_requests.lock().unwrap()[1].as_slice(),
        original.as_ref()
    );
    let trace = capture.traces("provider").remove(0);
    assert_eq!(trace["data"]["request_body_bytes"], original.len());
}

#[tokio::test]
async fn ready_response_does_not_hide_an_overdue_scheduler_probe() {
    // select! randomizes equally ready branches. Exercise several completions so
    // the regression catches a response winning the race against an overdue tick.
    for _ in 0..8 {
        let capture = Capture::new(true);
        let (release, gate) = tokio::sync::oneshot::channel();
        let mut response = Response::sse(Vec::new());
        response.headers_gate = Some(gate);
        let server = Server::start(vec![response]).await;
        let transport = llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(
            &server.url,
            "test",
        ))
        .unwrap();
        let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
        {
            let request = transport.post_stream_bytes_traced(
                "/chat/completions",
                bytes::Bytes::from_static(br#"{"stream":true}"#),
                &mut trace,
            );
            tokio::pin!(request);
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(20), request.as_mut())
                            .await
                            .is_err()
                    );
                    if !server.requests.lock().unwrap().is_empty() {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            release.send(()).unwrap();
            // Hyper can receive the headers while this caller is not polled.
            // On resumption both the response and the 250 ms tick are ready.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let events: Vec<_> = request.as_mut().await.unwrap().collect().await;
            assert!(events.iter().all(Result::is_ok));
        }
        trace.finish("completed");
        let record = capture.traces("provider").remove(0);
        assert!(
            record["data"]["http_transport"]["max_scheduler_lag_ms"]
                .as_f64()
                .unwrap()
                >= 100.0,
            "an overdue final probe was lost: {record}"
        );
    }
}

#[test]
fn captures_wall_and_suspend_clock_snapshots() {
    let capture = Capture::new(true);
    let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
    trace.checkpoint("before_wait");
    std::thread::sleep(Duration::from_millis(15));
    trace.checkpoint("after_wait");
    trace.finish("completed");
    let record = capture.traces("provider").remove(0);
    assert!(record["observed_at_unix_ms"].is_number());
    assert!(record["wall_elapsed_ms"].as_f64().unwrap() >= 15.0);
    assert!(record["milestones_wall_ms"]["after_wait"].is_number());
    #[cfg(target_os = "macos")]
    {
        assert!(record["elapsed_including_suspend_ms"].as_f64().unwrap() >= 15.0);
        assert!(record["suspend_elapsed_ms"].is_number());
        assert!(record["wall_clock_adjustment_ms"].is_number());
        assert!(record["milestones_including_suspend_ms"]["after_wait"].is_number());
    }
}

#[tokio::test]
async fn captures_reused_socket_before_headers_and_confirms_it_at_headers() {
    let capture = Capture::new(true);
    let mut first = successful_response(None);
    first.keep_connection = true;
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut second = successful_response(None);
    second.headers_gate = Some(gate);
    let server = Server::start(vec![first, second]).await;
    let transport =
        llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(&server.url, "test"))
            .unwrap();
    consume_raw(&transport, &capture).await;
    let previous = capture.traces("provider").remove(0);
    let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
    {
        let request = transport.post_stream_bytes_traced(
            "/chat/completions",
            bytes::Bytes::from_static(br#"{"stream":true,"context":"unchanged"}"#),
            &mut trace,
        );
        tokio::pin!(request);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), request.as_mut())
                        .await
                        .is_err()
                );
                if server.requests.lock().unwrap().len() == 2 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // The client's existing sockets must be sampled even before headers.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), request.as_mut())
                .await
                .is_err()
        );
        let current = capture
            .traces("provider")
            .into_iter()
            .find(|record| record["outcome"] == "in_progress")
            .unwrap();
        assert!(current["milestones_ms"]["response_headers"].is_null());
        assert!(current["milestones_ms"]["connection_assigned"].is_null());
        assert_eq!(
            current["data"]["http_transport"]["pre_response_socket_attribution"],
            "transport_candidates_only"
        );
        let network = capture
            .traces("network")
            .into_iter()
            .find(|record| record["trace_id"] == current["trace_id"])
            .expect("network collection is automatic, including reused connections");
        #[cfg(target_os = "macos")]
        assert!(
            network["samples"].as_array().unwrap().iter().any(|sample| {
                sample["local_addr"] == previous["data"]["http_transport"]["local_addr"]
                    && sample["attribution"] == "transport_candidate"
                    && sample["counters"]["tx_bytes"].is_number()
            }),
            "missing pre-header socket counters: {network}"
        );
        assert!(network["confirmed_connection"].is_null());
        #[cfg(not(target_os = "macos"))]
        assert_eq!(network["availability"], "unsupported_platform");
        release.send(()).unwrap();
        let events: Vec<_> = request.as_mut().await.unwrap().collect().await;
        assert!(events.iter().all(Result::is_ok));
    }
    trace.finish("completed");
    let network = capture
        .traces("network")
        .into_iter()
        .find(|record| record["trace_id"] != previous["trace_id"])
        .unwrap();
    assert_eq!(
        network["confirmed_connection"]["local_addr"],
        previous["data"]["http_transport"]["local_addr"]
    );
    assert_eq!(network["outcome"], "completed");
}

#[tokio::test]
async fn network_sampling_continues_during_stream_silence_without_consumer_polling() {
    let capture = Capture::new(true);
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(None);
    response.first_chunk_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let mut stream = server.model().chat_stream(capture.request());
    stream.next().await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.next())
                    .await
                    .is_err()
            );
            if capture.traces("provider")[0]["milestones_ms"]["response_headers"].is_number() {
                break;
            }
        }
    })
    .await
    .unwrap();
    #[cfg(target_os = "macos")]
    let before = capture.traces("network").remove(0);
    // A socket from another client/process activity must never appear in this capture.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unrelated = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (_peer, _) = listener.accept().await.unwrap();
    let unrelated_local = unrelated.local_addr().unwrap().to_string();
    // Do not poll the model stream. Only the autonomous sampler can write here.
    // Boundary snapshots reset the write budget; the first 1 s tick after the
    // 5 s write interval may be the sixth tick.
    tokio::time::sleep(Duration::from_millis(6300)).await;
    let during = capture.traces("network").remove(0);
    #[cfg(target_os = "macos")]
    {
        assert!(
            during["sample_count"].as_u64().unwrap()
                >= before["sample_count"].as_u64().unwrap() + 5
        );
        assert!(during["wall_elapsed_ms"].as_f64().unwrap() >= 5000.0);
        assert!(during["samples"].as_array().unwrap().iter().any(|sample| {
            sample["reason"] == "interval"
                && sample["attribution"] == "response_confirmed"
                && sample["counters"]["tx_bytes"].is_number()
        }));
    }
    #[cfg(not(target_os = "macos"))]
    assert_eq!(during["availability"], "unsupported_platform");
    assert!(during["samples"]
        .as_array()
        .unwrap()
        .iter()
        .all(|sample| sample["local_addr"] != unrelated_local));
    let serialized = during.to_string();
    for private in [
        "secret-api-key",
        "secret-cookie",
        "Keep the complete long context",
        "enable_thinking",
    ] {
        assert!(!serialized.contains(private));
    }
    release.send(()).unwrap();
    assert!(stream.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    assert_eq!(capture.traces("network")[0]["outcome"], "completed");
}

#[tokio::test]
async fn disabling_or_replacing_capture_stops_sampling_but_not_the_http_request() {
    for replace in [false, true] {
        let capture = Capture::new(true);
        let (release, gate) = tokio::sync::oneshot::channel();
        let mut response = successful_response(None);
        response.headers_gate = Some(gate);
        let server = Server::start(vec![response]).await;
        let mut stream = server.model().chat_stream(capture.request());
        stream.next().await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), stream.next())
                        .await
                        .is_err()
                );
                if !server.requests.lock().unwrap().is_empty() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // Registry restoration with identical facts must not cancel a live sampler.
        debug_capture::register_capture(
            capture.address.clone(),
            debug_capture::active_capture(&capture.address).unwrap(),
        );
        assert_eq!(capture.traces("network")[0]["outcome"], "in_progress");
        if replace {
            debug_capture::register_capture(
                capture.address.clone(),
                ActiveCapture {
                    capture_id: "replacement".into(),
                    capture_path: capture.root.join("replacement"),
                },
            );
        } else {
            debug_capture::unregister_capture(&capture.address);
        }
        // Unsupported platforms have no worker; the next boundary/Drop finalizes
        // the unavailable artifact, but there are never any native probes.
        #[cfg(target_os = "macos")]
        {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if capture.traces("network")[0]["outcome"] == "capture_disabled" {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let stopped = capture.traces("network").remove(0);
            tokio::time::pause();
            for _ in 0..6 {
                tokio::time::advance(Duration::from_secs(1)).await;
                tokio::task::yield_now().await;
            }
            assert_eq!(capture.traces("network")[0], stopped);
            tokio::time::resume();
        }
        release.send(()).unwrap();
        let events = stream.collect::<Vec<_>>().await;
        assert!(
            events.iter().all(Result::is_ok),
            "capture cancellation cancelled HTTP"
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert_eq!(capture.traces("network")[0]["outcome"], "capture_disabled");
        assert_eq!(capture.traces("provider")[0]["outcome"], "completed");
    }
}

#[tokio::test]
async fn enabling_capture_after_pool_warmup_still_samples_the_reused_socket() {
    let capture = Capture::new(false);
    let mut first = successful_response(None);
    first.keep_connection = true;
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut second = successful_response(None);
    second.headers_gate = Some(gate);
    let server = Server::start(vec![first, second]).await;
    let transport =
        llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(&server.url, "test"))
            .unwrap();
    consume_raw(&transport, &capture).await;
    assert!(!capture.root.exists(), "disabled capture created artifacts");
    debug_capture::register_capture(
        capture.address.clone(),
        ActiveCapture {
            capture_id: "enabled-after-warmup".into(),
            capture_path: capture.root.clone(),
        },
    );
    let call = consume_raw(&transport, &capture);
    tokio::pin!(call);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), call.as_mut())
            .await
            .is_err()
    );
    let before = capture.traces("network").remove(0);
    assert!(before["confirmed_connection"].is_null());
    #[cfg(target_os = "macos")]
    assert!(before["samples"].as_array().unwrap().iter().any(|sample| {
        sample["attribution"] == "transport_candidate"
            && sample["counters"]["tx_bytes"].as_u64().unwrap_or(0) > 0
    }));
    release.send(()).unwrap();
    call.await;
    let provider = capture.traces("provider").remove(0);
    assert_eq!(
        provider["data"]["http_transport"]["connection_origin"],
        "pooled_or_shared"
    );
    assert_eq!(provider["data"]["http_transport"]["connection_attempts"], 0);
    #[cfg(target_os = "macos")]
    assert!(before["samples"].as_array().unwrap().iter().any(|sample| {
        sample["local_addr"] == provider["data"]["http_transport"]["local_addr"]
    }));
}

#[tokio::test]
async fn initial_network_write_failure_stops_sampling_without_failing_the_request() {
    let capture = Capture::new(true);
    std::fs::create_dir_all(&capture.base).unwrap();
    std::fs::write(&capture.root, b"not a directory").unwrap();
    let (release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(None);
    response.headers_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let mut stream = server.model().chat_stream(capture.request());
    stream.next().await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.next())
                    .await
                    .is_err()
            );
            if !server.requests.lock().unwrap().is_empty() {
                break;
            }
        }
    })
    .await
    .unwrap();
    // Recording is asynchronous: confirm the injected disk failure was observed
    // before repairing the directory and letting the HTTP request finish.
    assert!(debug_capture::flush_capture(&capture.root, Duration::from_secs(5)).is_err());
    std::fs::remove_file(&capture.root).unwrap();
    release.send(()).unwrap();
    assert!(stream.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    // Capture failure stays observable even after later writes succeed.
    assert!(debug_capture::flush_capture(&capture.root, Duration::from_secs(5)).is_err());
    let traces: Vec<Value> = std::fs::read_dir(capture.root.join("calls/call-1"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .collect();
    let provider = traces
        .iter()
        .find(|trace| trace["stage"] == "provider")
        .unwrap();
    assert_eq!(provider["outcome"], "completed");
    assert!(
        !traces.iter().any(|trace| trace["stage"] == "network"),
        "failed sampler restarted behind the user's back"
    );
    assert_eq!(
        provider["data"]["network_capture_outcome"],
        "capture_write_failed"
    );

    // A past failure remains visible, but must not permanently disable sampling
    // for later HTTP attempts after the user fixes the capture directory.
    let server = Server::start(vec![successful_response(None)]).await;
    let mut request = capture.request();
    request.metadata.insert(
        MODEL_CALL_ID_METADATA_KEY.into(),
        json!("call-after-repair"),
    );
    assert!(server
        .model()
        .chat_stream(request)
        .collect::<Vec<_>>()
        .await
        .iter()
        .all(Result::is_ok));
    assert!(debug_capture::flush_capture(&capture.root, Duration::from_secs(5)).is_err());
    let network = std::fs::read_dir(capture.root.join("calls/call-after-repair"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("network-timing-")
        })
        .unwrap();
    let network: Value = serde_json::from_slice(&std::fs::read(network).unwrap()).unwrap();
    assert_eq!(network["outcome"], "completed");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn network_history_is_bounded_and_keeps_startup_and_recent_samples() {
    let capture = Capture::new(true);
    let (_release, gate) = tokio::sync::oneshot::channel();
    let mut response = successful_response(None);
    response.first_chunk_gate = Some(gate);
    let server = Server::start(vec![response]).await;
    let transport =
        llm_multiplex::HttpTransport::new(llm_multiplex::TransportConfig::new(&server.url, "test"))
            .unwrap();
    let mut trace = CaptureTrace::new(&capture.request().metadata, "provider");
    let stream = transport
        .post_stream_bytes_traced(
            "/chat/completions",
            bytes::Bytes::from_static(b"{}"),
            &mut trace,
        )
        .await
        .unwrap();
    // Advance only Tokio's timer to exercise retention quickly; clock mathematics
    // and real sampling intervals are verified separately, without changing macOS time.
    tokio::time::pause();
    for _ in 0..650 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    drop(stream);
    trace.finish("dropped");
    let network = capture.traces("network").remove(0);
    let samples = network["samples"].as_array().unwrap();
    let count = network["sample_count"].as_u64().unwrap();
    assert_eq!(samples.len(), 600);
    assert!(count > 600);
    assert_eq!(network["samples_omitted"].as_u64().unwrap(), count - 600);
    for (index, sample) in samples.iter().take(32).enumerate() {
        assert_eq!(sample["sequence"].as_u64().unwrap(), index as u64 + 1);
    }
    assert!(samples[32]["sequence"].as_u64().unwrap() > 33);
    assert_eq!(samples.last().unwrap()["sequence"].as_u64().unwrap(), count);
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        capture.traces("network")[0],
        network,
        "finished sampler kept writing"
    );
}
