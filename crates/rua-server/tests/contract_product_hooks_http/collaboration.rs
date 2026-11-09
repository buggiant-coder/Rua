use axum::http::{Method, StatusCode};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::{ChatAddress, ChatScopeStore};
use rua_server::services::collaboration::CollaborationCaller;
use rua_session::WorkspaceSessionStore;
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;

fn mock_acp_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn mock_acp_fixture() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-acp/tests/fixtures/mock_acp_agent.py")
}

fn mock_acp_service(
    test: &TestApp,
    env: std::collections::HashMap<String, String>,
) -> Arc<rua_config::types::acp::AcpConfig> {
    let mut agents = std::collections::HashMap::new();
    agents.insert(
        "mock_agent".to_string(),
        rua_config::types::acp::AcpAgentConfig {
            enabled: true,
            command: mock_acp_command().into(),
            args: vec![mock_acp_fixture().to_string_lossy().to_string()],
            env,
            trusted: true,
            tool_parse_mode: Default::default(),
            stdio_buffer_limit_bytes: None,
            extra: Default::default(),
        },
    );
    let config = rua_config::types::acp::AcpConfig {
        agents,
        extra: Default::default(),
    };
    let agent_path = test.state.rua_root.join("workspaces/default/agent.json");
    let mut agent = std::fs::read(&agent_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_else(|| json!({}));
    agent["acp"] = serde_json::to_value(&config).unwrap();
    std::fs::write(&agent_path, serde_json::to_vec_pretty(&agent).unwrap()).unwrap();
    Arc::new(config)
}

struct TestAcpController {
    manager: Arc<rua_command::AcpChatSessionManager>,
    workspace_dir: std::path::PathBuf,
}

#[async_trait::async_trait]
impl rua_command::AcpChatController for TestAcpController {
    async fn enter(&self, chat_id: &str, runner: &str, cwd: Option<&str>) -> String {
        self.manager
            .register_with_runner_session(
                chat_id.to_string(),
                runner.to_string(),
                Some("fixture-runner-session".into()),
                cwd.map(std::path::PathBuf::from)
                    .unwrap_or_else(|| self.workspace_dir.clone()),
            )
            .await;
        "entered".into()
    }

    async fn exit(&self, chat_id: &str) -> String {
        self.manager.unregister(chat_id).await;
        "exited".into()
    }

    async fn list_sessions(&self, _: &str, _: &str, _: Option<&str>) -> String {
        "[]".into()
    }

    async fn resume(&self, chat_id: &str, runner: &str, _: &str, cwd: Option<&str>) -> String {
        self.enter(chat_id, runner, cwd).await
    }
}

#[derive(Debug, Clone)]
struct ProgressSummaryCall {
    activity: String,
    previous_summary: Option<String>,
    model_slot: Option<serde_json::Value>,
}

struct RecordingProgressSummarizer {
    calls: tokio::sync::Mutex<Vec<ProgressSummaryCall>>,
    response: tokio::sync::Mutex<Result<String, String>>,
    release: tokio::sync::Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl RecordingProgressSummarizer {
    fn successful(response: impl Into<String>) -> Self {
        Self {
            calls: Default::default(),
            response: tokio::sync::Mutex::new(Ok(response.into())),
            release: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl rua_server::services::collaboration_progress::CollaborationProgressSummarizer
    for RecordingProgressSummarizer
{
    async fn summarize(
        &self,
        activity: &str,
        previous_summary: Option<&str>,
        model_slot: Option<&serde_json::Value>,
    ) -> Result<String, String> {
        self.calls.lock().await.push(ProgressSummaryCall {
            activity: activity.to_string(),
            previous_summary: previous_summary.map(str::to_string),
            model_slot: model_slot.cloned(),
        });
        if let Some(release) = self.release.lock().await.clone() {
            release.notified().await;
        }
        self.response.lock().await.clone()
    }
}

struct ControlledRuntime {
    requests: Arc<tokio::sync::Mutex<Vec<AgentRequest>>>,
    release: Option<Arc<tokio::sync::Notify>>,
}

async fn wait_for_source_requests(requests: &tokio::sync::Mutex<Vec<AgentRequest>>, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < count {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

struct BackgroundResultRunner {
    release: Arc<tokio::sync::Notify>,
}

fn background_test_router(test: &TestApp) -> axum::Router {
    test.app.clone().merge(
        axum::Router::new()
            .nest(
                "/api",
                rua_server::endpoints::console::agent_scoped_router(),
            )
            .layer(axum::middleware::from_fn_with_state(
                test.state.clone(),
                rua_server::middleware::agent_context_middleware,
            ))
            .with_state(test.state.clone()),
    )
}

#[async_trait::async_trait]
impl rua_task::TaskRunner for BackgroundResultRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }
    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        tokio::select! {
            _ = request.cancel.cancelled() => Err(rua_task::TaskRunnerError::new("cancelled")),
            _ = self.release.notified() => Ok(rua_task::TaskRunOutput::completed("background complete")),
        }
    }
}

#[tokio::test]
async fn background_task_resumes_idle_source_without_viewer_or_polling() {
    assert_background_task_continuation(false).await;
}

#[tokio::test]
async fn background_task_result_wakes_source_after_user_stop() {
    assert_background_task_continuation(true).await;
}

#[tokio::test]
async fn background_task_result_queues_behind_active_source_without_parallel_turn() {
    assert_background_result_behind_active_source(false).await;
}

#[tokio::test]
async fn queued_background_result_is_not_replayed_after_source_stop() {
    assert_background_result_behind_active_source(true).await;
}

async fn assert_background_result_behind_active_source(stop_queued: bool) {
    let test = build_app();
    let app = background_test_router(&test);
    let source = caller(&test, "busy-background-source").await;
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let parent_release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(parent_release.clone()),
        }))
        .await;
    let result = request(
        &app,
        Method::POST,
        &format!(
            "/api/agents/default/chats/{}/console/chat",
            source.address.chat_id()
        ),
        Some(serde_json::json!({"message":"independent work"})),
    )
    .await;
    assert!(result.status().is_success());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let first_turn = test
        .state
        .conversation_authority
        .active_turn(&source.address)
        .await
        .unwrap()
        .turn_id;
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .task_runtime
        .register_runner(Arc::new(BackgroundResultRunner {
            release: release.clone(),
        }));
    let owner = rua_task::TaskOwner::from_address(source.address.clone());
    let id = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "parallel work".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_BackgroundResult".into(),
                    arguments: serde_json::json!({}),
                },
                context_refs: vec![],
                depends_on: vec![],
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !test
            .state
            .task_runtime
            .get(&owner, &id)
            .await
            .unwrap()
            .state
            .is_terminal()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(requests.lock().await.len(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !test
            .state
            .task_runtime
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        requests.lock().await.len(),
        1,
        "queue acceptance must not wait for the active source reply"
    );
    test.state
        .conversation_input_intake
        .resume_task_results(Some(&source.address))
        .await
        .unwrap();
    assert_eq!(
        requests.lock().await.len(),
        1,
        "already queued results must not start a duplicate Turn"
    );
    assert_eq!(
        test.state
            .conversation_authority
            .active_turn(&source.address)
            .await
            .unwrap()
            .turn_id,
        first_turn
    );
    if stop_queued {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        test.state
            .conversation_input_intake
            .request_local_stop("default", source.address.chat_id().as_str())
            .await;
        parent_release.notify_one();
        wait_for_source_requests(&requests, 2).await;
        let result_turn = test
            .state
            .conversation_authority
            .active_turn(&source.address)
            .await
            .expect("queued result should own the next Turn");
        assert_ne!(result_turn.turn_id, first_turn);
        parent_release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while test
                .state
                .conversation_authority
                .active_turn(&source.address)
                .await
                .is_some()
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        test.state
            .conversation_input_intake
            .resume_task_results(Some(&source.address))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            requests.lock().await.len(),
            2,
            "the accepted result must be processed once after Stop, without replay"
        );
        return;
    }
    parent_release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    parent_release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !test
            .state
            .task_runtime
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn assert_background_task_continuation(stop_before_result: bool) {
    let test = build_app();
    let app = background_test_router(&test);
    let source = caller(&test, "background-task-source").await;
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    runtime
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .task_runtime
        .register_runner(Arc::new(BackgroundResultRunner {
            release: release.clone(),
        }));
    let owner = rua_task::TaskOwner::from_address(source.address.clone());
    let task_id = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "background continuation".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_BackgroundResult".into(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    let before = test
        .state
        .conversation_authority
        .background_work_view(&source.address)
        .await;
    assert!(before.items.iter().any(|item| item.id == task_id));
    assert!(test
        .state
        .conversation_authority
        .active_turn(&source.address)
        .await
        .is_none());
    assert!(requests.lock().await.is_empty());
    if stop_before_result {
        let header = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
            .load_chat_header(source.address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap();
        let control = test
            .state
            .conversation_authority
            .control_view(&source.address, &serde_json::Value::Object(header.extra))
            .await;
        let epoch = control.placement.authority_epoch.unwrap();
        let turn = test
            .state
            .conversation_authority
            .start_active_turn(
                rua_server::services::conversation_authority::ConversationActiveTurnStart {
                    address: source.address.clone(),
                    driver:
                        rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                    authority_device_id: control.authority_device_id.unwrap(),
                    authority_epoch: epoch,
                    driver_detail: None,
                },
            )
            .await;
        let response = request(
            &app,
            Method::POST,
            &format!(
                "/api/agents/default/chats/{}/console/stop",
                source.address.chat_id()
            ),
            Some(serde_json::json!({"turn_id":turn.turn_id,"authority_epoch":epoch})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(read_json(response).await["stopped"], true);
    }
    if stop_before_result {
        assert!(
            !test
                .state
                .conversation_authority
                .background_work_view(&source.address)
                .await
                .suppressed
        );
        test.state
            .conversation_authority
            .finish_active_turn(&source.address)
            .await;
    }
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            if !requests.lock().await.is_empty()
                && test
                    .state
                    .task_runtime
                    .pending_terminal_deliveries(Some(&owner))
                    .await
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(requests.lock().await.len(), 1);
    test.state
        .conversation_input_intake
        .resume_task_results(Some(&source.address))
        .await
        .unwrap();
    assert_eq!(requests.lock().await.len(), 1);
    assert!(test
        .state
        .conversation_authority
        .background_work_view(&source.address)
        .await
        .items
        .is_empty());
}

#[async_trait::async_trait]
impl AgentRuntime for ControlledRuntime {
    async fn stream(
        &self,
        request: AgentRequest,
    ) -> rua_core::Result<Pin<Box<dyn futures::Stream<Item = rua_core::Result<RuntimeEvent>> + Send>>>
    {
        self.requests.lock().await.push(request.clone());
        let release = self.release.clone();
        let events = std::collections::VecDeque::from([
            RuntimeEvent::Start {
                session_id: request.session_id.clone(),
                agent_id: Some(request.agent_id.clone()),
            },
            RuntimeEvent::TextDelta {
                delta: "review complete".into(),
                message_id: Some(rua_core::MessageId("review-result".into())),
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]);
        let stream = Box::pin(futures::stream::unfold(
            (events, release),
            |(mut events, mut release)| async move {
                if events.len() == 2 {
                    if let Some(release) = release.take() {
                        release.notified().await;
                    }
                }
                events
                    .pop_front()
                    .map(|event| (Ok(event), (events, release)))
            },
        ));
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &request, stream,
        ))
    }
    async fn cancel(&self, _: &rua_core::SessionId) -> rua_core::Result<()> {
        if let Some(release) = &self.release {
            release.notify_one();
        }
        Ok(())
    }
    async fn warmup(&self, _: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

use super::support::{build_app, read_json, request, TestApp};

async fn caller(test: &TestApp, name: &str) -> CollaborationCaller {
    let chat = super::support::seed_materialized_chat(test, "default", name).await;
    let address = ChatAddress::agent("default", chat["id"].as_str().unwrap());
    CollaborationCaller {
        working_dir: ChatScopeStore::resolve(&test.state.rua_root, address.clone())
            .root()
            .to_path_buf(),
        address,
        turn_id: "source-turn".into(),
        tool_call_id: name.into(),
        configuration_epoch: chat["meta"]["hook_configuration"]["epoch"]
            .as_str()
            .unwrap()
            .into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local Playwright Chromium and Console dependencies"]
async fn collaboration_live_browser_acceptance() {
    let test = build_app();
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    runtime
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            release: Some(Arc::new(tokio::sync::Notify::new())),
        }))
        .await;
    let mut chats = Vec::new();
    let mut turns = Vec::new();
    let mut sources = Vec::new();
    for width in [390, 1440] {
        let source = caller(&test, &format!("browser-{width}")).await;
        sources.push(source.address.chat_id().to_string());
        let child = test
            .state
            .collaboration
            .create(
                source,
                "default".into(),
                Some("Inspect the scoped work".into()),
                0,
            )
            .await
            .unwrap();
        chats.push(child.chat.chat_id().as_str().to_string());
        let active = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(active) = test
                    .state
                    .conversation_authority
                    .active_turn(&child.chat)
                    .await
                {
                    break active;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("collaboration Turn must start");
        turns.push(json!({"turn_id": active.turn_id, "authority_epoch": active.authority_epoch}));
    }
    let console = axum::Router::new()
        .nest(
            "/api",
            rua_server::endpoints::console::agent_scoped_router()
                .merge(rua_server::endpoints::acp::agent_scoped_router())
                .merge(rua_server::endpoints::acp_chat::router())
                .merge(rua_server::endpoints::approvals::router())
                .merge(rua_server::endpoints::commands_discovery::router())
                .merge(rua_server::endpoints::tasks::router()),
        )
        .layer(axum::middleware::from_fn_with_state(
            test.state.clone(),
            rua_server::middleware::agent_context_middleware,
        ))
        .with_state(test.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let cancel = tokio_util::sync::CancellationToken::new();
    let shutdown = cancel.clone();
    let app = test.app.clone().merge(console);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let tool_task = live_registered_tool_checks(&test).await;
    let result = tokio::process::Command::new("npx")
        .args([
            "playwright",
            "test",
            "--config",
            "playwright.collaboration-live.config.ts",
        ])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../console"))
        .env("VITE_PROXY_TARGET", endpoint)
        .env("RUA_LIVE_TOOL_TASK", tool_task.to_string())
        .env(
            "RUA_COLLABORATION_CHATS",
            serde_json::to_string(&chats).unwrap(),
        )
        .env(
            "RUA_COLLABORATION_TURNS",
            serde_json::to_string(&turns).unwrap(),
        )
        .env(
            "RUA_BACKGROUND_SOURCES",
            serde_json::to_string(&sources).unwrap(),
        )
        .kill_on_drop(true)
        .status()
        .await;
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap().success(), "live browser acceptance failed");
}

async fn live_registered_tool_checks(test: &TestApp) -> serde_json::Value {
    use rua_tool::extended::task_tools::TaskRuntimeHandle;
    let source = caller(test, "live-tool-source").await;
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    let toolkit = runtime.task_toolkit().await.unwrap();
    let mut ctx = rua_core::tool::ToolContext {
        agent_id: "default".into(),
        session_id: source.address.chat_id().to_string(),
        working_dir: source.working_dir,
        collaboration_runtime: Some(Arc::clone(&test.state.collaboration_runtime)),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(
            test.state.task_runtime.clone(),
        ))),
        ..Default::default()
    };
    ctx.metadata
        .insert("security_chat_address".into(), json!(source.address));
    ctx.metadata.insert("turn_id".into(), json!(source.turn_id));
    ctx.metadata.insert(
        "chat_configuration_epoch".into(),
        json!(source.configuration_epoch),
    );
    async fn call(
        toolkit: &react_agent::AgentToolkit,
        ctx: &rua_core::tool::ToolContext,
        name: &str,
        id: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let result = toolkit
            .get_executor(name)
            .unwrap()
            .execute(&agent_types::ToolCall::new(id, name, args), ctx)
            .await
            .unwrap();
        assert!(!result.is_error, "{name}: {}", result.text_content());
        println!("LIVE TOOL {name}: {}", result.text_content());
        serde_json::from_str(&result.text_content()).unwrap()
    }
    let created = call(
        &toolkit,
        &ctx,
        "chat_send",
        "live-create",
        json!({"agent_id":"default","message":"initial live request","yield_seconds":0}),
    )
    .await;
    let chat = created["chat"].clone();
    let sent = call(
        &toolkit,
        &ctx,
        "chat_send",
        "live-send",
        json!({"chat":chat,"message":"Hold this review for stop verification","yield_seconds":0}),
    )
    .await;
    let active = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let read = call(
                &toolkit,
                &ctx,
                "chat_read",
                "live-chat-read",
                json!({"chat":chat,"yield_seconds":0}),
            )
            .await;
            if read["active_turn"].is_object() {
                break read["active_turn"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let queued = call(
        &toolkit,
        &ctx,
        "chat_send",
        "live-queued",
        json!({"chat":chat,"message":"Follow-up remains queued","yield_seconds":0}),
    )
    .await;
    assert_ne!(sent["request_id"], queued["request_id"]);
    let stale = call(
        &toolkit,
        &ctx,
        "chat_stop",
        "live-stale-stop",
        json!({"chat":chat,"turn_id":"old-turn","authority_epoch":active["authority_epoch"]}),
    )
    .await;
    assert_eq!(stale["status"], "target_changed");
    let stop = call(&toolkit, &ctx, "chat_stop", "live-stop", json!({"chat":chat,"turn_id":active["turn_id"],"authority_epoch":active["authority_epoch"]})).await;
    assert_eq!(stop["status"], "stop_requested");
    println!("LIVE CHAT verified create/send/read/conditional-stop on actual registry");

    let task = call(&toolkit, &ctx, "task_create", "live-shell", json!({"runner":{"type":"tool","tool_name":"bash","arguments":{
        "command":"printf '\\033[31mBEGIN\\033[0m\\n'; i=0; while [ $i -lt 180 ]; do printf 'row-%03d payload\\n' \"$i\"; i=$((i+1)); done; printf 'TAIL'; printf 'error-evidence\\n' >&2; exit 7",
        "cwd":ctx.working_dir
    }}})).await;
    let task_id = task["task_id"].as_str().unwrap();
    let owner = rua_task::TaskOwner::from_address(source.address.clone());
    let terminal = test
        .state
        .task_runtime
        .await_terminal(&owner, task_id, std::time::Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(terminal.state, rua_task::TaskState::Failed);
    let mut cursor = serde_json::Value::Null;
    let mut text = String::new();
    let mut pages = 0;
    loop {
        let mut args = json!({"task_id":task_id,"max_bytes":512});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let page = call(&toolkit, &ctx, "task_read", "live-task-read", args).await;
        assert_eq!(page["status"], "failed");
        if let Some(output) = page["output"].as_array() {
            for record in output {
                text.push_str(record["text"].as_str().unwrap());
            }
        }
        pages += 1;
        assert!(pages < 100);
        if page["has_more"] == false {
            break;
        }
        cursor = page["cursor"].clone();
        assert!(!cursor.is_null());
    }
    assert!(pages > 1);
    assert!(!text.contains('\u{1b}'));
    assert!(
        text.contains("BEGIN")
            && text.contains("row-179")
            && text.contains("TAIL")
            && text.contains("error-evidence"),
        "{text}"
    );
    let listed = call(
        &toolkit,
        &ctx,
        "task_list",
        "live-list",
        json!({"limit":10}),
    )
    .await;
    assert!(listed["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|task| task["task_id"] == task_id));
    let waiting = call(&toolkit, &ctx, "task_create", "live-cancel-shell", json!({"runner":{"type":"tool","tool_name":"bash","arguments":{"command":"sleep 30","cwd":ctx.working_dir}}})).await;
    let cancelled = call(
        &toolkit,
        &ctx,
        "task_cancel",
        "live-cancel",
        json!({"task_id":waiting["task_id"]}),
    )
    .await;
    assert_eq!(cancelled["status"], "cancelled");
    ctx.session_id = "other-chat".into();
    ctx.metadata.insert(
        "security_chat_address".into(),
        json!(ChatAddress::agent("default", "other-chat")),
    );
    assert!(toolkit
        .get_executor("task_read")
        .unwrap()
        .execute(
            &agent_types::ToolCall::new("live-denied", "task_read", json!({"task_id":task_id})),
            &ctx
        )
        .await
        .is_err());
    println!("LIVE TASK verified real shell failure, ANSI cleanup, {pages} pages, final tail, list, cancel and owner isolation");
    json!({"chat_id":source.address.chat_id(),"task_id":task_id})
}

#[tokio::test]
async fn create_is_idempotent_isolated_and_inherits_source_environment() {
    let test = build_app();
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id":"reviewer","name":"Reviewer"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let source = caller(&test, "create-one").await;
    let created = test
        .state
        .collaboration
        .create(source.clone(), "reviewer".into(), None, 0)
        .await
        .unwrap();
    let repeated = test
        .state
        .collaboration
        .create(source.clone(), "reviewer".into(), None, 0)
        .await
        .unwrap();
    assert_eq!(created.chat, repeated.chat);
    assert_ne!(created.chat, source.address);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, created.chat.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let header = store
        .load_chat_header(created.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        header.extra["collaboration"]["source"],
        serde_json::to_value(&source.address).unwrap()
    );
    assert_eq!(
        header.extra["collaboration"]["working_dir"],
        json!(source.working_dir)
    );
    assert!(store
        .read_turn_range(created.chat.chat_id().as_str(), 1, 10)
        .await
        .unwrap()
        .is_empty());
    let other = caller(&test, "another-source").await;
    assert!(test
        .state
        .collaboration
        .send(other, created.chat.clone(), "hello".into(), 0)
        .await
        .unwrap_err()
        .contains("another source"));
    let child_caller = CollaborationCaller {
        address: created.chat,
        configuration_epoch: header.extra["hook_configuration"]["epoch"]
            .as_str()
            .unwrap()
            .into(),
        ..source
    };
    assert!(test
        .state
        .collaboration
        .create(child_caller, "reviewer".into(), None, 0)
        .await
        .unwrap_err()
        .contains("another level"));
}

#[tokio::test]
async fn create_inherits_source_model_when_executor_default_is_null() {
    let test = build_app();
    let response = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"reviewer","name":"Reviewer","default_model":null})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let mut source = caller(&test, "inherit-model").await;
    let source_scope = ChatScopeStore::resolve(&test.state.rua_root, source.address.clone());
    rua_chat::ChatService::for_resolved_scope(source_scope.root(), source.address.clone())
        .update_setting(
            &source.address,
            rua_chat::ChatSetting::Model(json!({"provider_id":"fixture","model":"source-model"})),
        )
        .await
        .unwrap();
    let source_header = WorkspaceSessionStore::new(source_scope.root())
        .load_chat_header(source.address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let source_model = rua_chat::model_slot_from_header(&source_header).unwrap();
    source.configuration_epoch = source_header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    let created = test
        .state
        .collaboration
        .create(source, "reviewer".into(), None, 0)
        .await
        .unwrap();
    let scope = ChatScopeStore::resolve(&test.state.rua_root, created.chat.clone());
    let header = WorkspaceSessionStore::new(scope.root())
        .load_chat_header(created.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rua_chat::model_slot_from_header(&header),
        Some(source_model.clone())
    );
    let response = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/reviewer/chats/{}", created.chat.chat_id()),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read_json(response).await["model"]["model"],
        source_model["model"]
    );
}

#[tokio::test]
async fn create_prefers_configured_executor_model_over_source_model() {
    let test = build_app();
    let model = json!({"provider_id":"fixture","model":"executor-model"});
    let response = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"reviewer","name":"Reviewer","default_model":model})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let mut source = caller(&test, "executor-model-priority").await;
    let source_scope = ChatScopeStore::resolve(&test.state.rua_root, source.address.clone());
    rua_chat::ChatService::for_resolved_scope(source_scope.root(), source.address.clone())
        .update_setting(
            &source.address,
            rua_chat::ChatSetting::Model(json!({"provider_id":"fixture","model":"source-model"})),
        )
        .await
        .unwrap();
    let source_header = WorkspaceSessionStore::new(source_scope.root())
        .load_chat_header(source.address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    source.configuration_epoch = source_header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    let created = test
        .state
        .collaboration
        .create(source, "reviewer".into(), None, 0)
        .await
        .unwrap();
    let scope = ChatScopeStore::resolve(&test.state.rua_root, created.chat.clone());
    let header = WorkspaceSessionStore::new(scope.root())
        .load_chat_header(created.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rua_chat::model_slot_from_header(&header), Some(model));
}

#[tokio::test]
async fn acp_collaboration_create_fixes_runner_inherits_cwd_and_creates_no_task() {
    let test = build_app();
    let source = caller(&test, "acp-create-source").await;
    let existing = test
        .state
        .acp_chat_registry
        .get("default")
        .expect("default ACP runtime");
    let runner = existing
        .service
        .agent_configs()
        .into_iter()
        .find_map(|(name, config)| config.enabled.then_some(name))
        .expect("enabled ACP runner fixture");
    let manager = Arc::new(rua_command::AcpChatSessionManager::new(
        std::time::Duration::from_secs(300),
    ));
    test.state.acp_chat_registry.register(
        "default".into(),
        Arc::new(
            rua_server::services::acp_chat_registry::AcpChatRuntimeHandle {
                service: Arc::clone(&existing.service),
                manager: Arc::clone(&manager),
                controller: Arc::new(TestAcpController {
                    manager: Arc::clone(&manager),
                    workspace_dir: source.working_dir.clone(),
                }),
                workspace_dir: source.working_dir.clone(),
            },
        ),
    );

    let created = test
        .state
        .collaboration
        .create_acp(source.clone(), runner.clone(), None, 0)
        .await
        .unwrap();
    let session = manager
        .get(created.chat.chat_id().as_str())
        .await
        .expect("ACP collaboration session");
    assert_eq!(session.runner, runner);
    assert_eq!(session.cwd, source.working_dir);
    assert_eq!(
        session.runner_session_id.as_deref(),
        Some("fixture-runner-session")
    );
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, created.chat.clone()).root(),
    );
    let header = store
        .load_chat_header(created.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let descriptor: rua_session::collaboration::CollaborationDescriptor =
        serde_json::from_value(header.extra["collaboration"].clone()).unwrap();
    assert_eq!(
        descriptor.executor,
        rua_session::collaboration::CollaborationExecutor::Acp {
            runner: runner.clone()
        }
    );
    assert_eq!(
        header.extra["acp_binding"]["runner"],
        serde_json::json!(runner)
    );
    assert_eq!(
        header.extra["acp_binding"]["runner_session_id"],
        "fixture-runner-session"
    );
    assert_eq!(
        test.state
            .task_runtime
            .list(
                &rua_task::TaskOwner::from_address(created.chat.clone()),
                rua_task::TaskListFilter::default(),
            )
            .await
            .unwrap()
            .len(),
        0
    );

    let mut changed = source.clone();
    changed.tool_call_id = source.tool_call_id.clone();
    let different_runner = if runner == "codex" { "claude" } else { "codex" };
    assert!(test
        .state
        .collaboration
        .create_acp(changed, different_runner.into(), None, 0)
        .await
        .unwrap_err()
        .contains("different executor"));

    let mut unavailable = source;
    unavailable.tool_call_id = "unavailable-runner-call".into();
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, unavailable.address.clone()).root(),
    );
    let targets_before = source_store
        .load_collaboration_targets(unavailable.address.chat_id().as_str())
        .await
        .unwrap();
    let error = test
        .state
        .collaboration
        .create_acp(unavailable.clone(), "missing-runner".into(), None, 0)
        .await
        .unwrap_err();
    assert!(error.contains("unavailable"));
    let targets_after = source_store
        .load_collaboration_targets(unavailable.address.chat_id().as_str())
        .await
        .unwrap();
    assert_eq!(targets_after, targets_before);
}

#[tokio::test]
async fn acp_collaboration_send_and_read_use_one_chat_session_without_task_wrapper() {
    let test = build_app();
    let source = caller(&test, "acp-protocol-source").await;
    let prompt_log = tempfile::NamedTempFile::new().unwrap();
    let mut env = std::collections::HashMap::new();
    env.insert(
        "ACP_MOCK_PROMPT_LOG_FILE".into(),
        prompt_log.path().to_string_lossy().to_string(),
    );
    env.insert("ACP_MOCK_ECHO_PROMPT_STATE".into(), "1".into());
    let config = mock_acp_service(&test, env);
    let handle = test
        .state
        .acp_chat_registry
        .get("default")
        .expect("default ACP runtime");
    handle.service.replace_config(config);
    let manager = Arc::clone(&handle.manager);

    let first = test
        .state
        .collaboration
        .create_acp(
            source.clone(),
            "mock_agent".into(),
            Some("first prompt".into()),
            10,
        )
        .await
        .unwrap();
    let first_outcome = first.outcome.expect("ACP first result");
    assert_eq!(
        first_outcome.status, "completed",
        "ACP result output: {}",
        first_outcome.output
    );
    assert!(first_outcome.output.contains("prompt=first prompt"));
    let first_scope = ChatScopeStore::resolve(&test.state.rua_root, first.chat.clone());
    let first_store = WorkspaceSessionStore::new(first_scope.root());
    assert_eq!(
        first_store
            .load_chat_header(first.chat.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .metadata
            .title,
        "first prompt"
    );
    let session = manager
        .get(first.chat.chat_id().as_str())
        .await
        .expect("ACP session");
    let session_id = session.runner_session_id.clone().unwrap();

    let mut continuation = source.clone();
    continuation.tool_call_id = "acp-second-call".into();
    let second = test
        .state
        .collaboration
        .send(continuation, first.chat.clone(), "second prompt".into(), 10)
        .await
        .unwrap();
    assert_eq!(second.outcome.as_ref().unwrap().status, "completed");
    assert_eq!(
        first_store
            .load_chat_header(first.chat.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .metadata
            .title,
        "first prompt"
    );
    assert!(second
        .outcome
        .as_ref()
        .unwrap()
        .output
        .contains("prompt=second prompt"));
    assert_eq!(
        manager
            .get(first.chat.chat_id().as_str())
            .await
            .unwrap()
            .runner_session_id
            .as_deref(),
        Some(session_id.as_str())
    );
    let prompts = std::fs::read_to_string(prompt_log.path()).unwrap();
    let entries = prompts
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["session_id"], entries[1]["session_id"]);
    assert_eq!(entries[0]["pid"], entries[1]["pid"]);

    assert_eq!(
        test.state
            .task_runtime
            .list(
                &rua_task::TaskOwner::from_address(first.chat.clone()),
                rua_task::TaskListFilter::default(),
            )
            .await
            .unwrap()
            .len(),
        0
    );
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, first.chat.clone()).root(),
    );
    let turns = store
        .read_turn_range(first.chat.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(turns.len(), 2);
    assert!(turns
        .iter()
        .all(|turn| turn.lifecycle.status == "completed"));
    let read = test
        .state
        .collaboration
        .read(source, first.chat, 0)
        .await
        .unwrap();
    assert!(read.outcome.is_none());
    assert_eq!(read.state, "completed");
}

#[tokio::test]
async fn acp_collaboration_stop_cancels_runner_turn_and_keeps_fixed_session_for_explicit_resume() {
    let test = build_app();
    let source = caller(&test, "acp-stop-source").await;
    let cancel_log = tempfile::NamedTempFile::new().unwrap();
    let mut env = std::collections::HashMap::new();
    env.insert("ACP_MOCK_WAIT_FOR_CANCEL".into(), "1".into());
    env.insert(
        "ACP_MOCK_CANCEL_LOG_FILE".into(),
        cancel_log.path().to_string_lossy().to_string(),
    );
    let config = mock_acp_service(&test, env);
    let handle = test
        .state
        .acp_chat_registry
        .get("default")
        .expect("default ACP runtime");
    handle.service.replace_config(config);
    let manager = Arc::clone(&handle.manager);
    let created = test
        .state
        .collaboration
        .create_acp(source.clone(), "mock_agent".into(), None, 0)
        .await
        .unwrap();

    let service = Arc::clone(&test.state.collaboration);
    let send_source = source.clone();
    let send_target = created.chat.clone();
    let send = tokio::spawn(async move {
        service
            .send(send_source, send_target, "wait for cancel".into(), 30)
            .await
    });
    let active = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(active) = test
                .state
                .conversation_authority
                .active_turn(&created.chat)
                .await
            {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        active.driver,
        rua_server::services::conversation_authority::ConversationDriverKind::Acp
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let sid = rua_core::id::SessionId::new(created.chat.chat_id().to_string());
        while !handle.service.turn_in_progress(&sid, "mock_agent").await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ACP runner prompt did not start");
    let control = test
        .state
        .collaboration
        .read(source.clone(), created.chat.clone(), 0)
        .await
        .unwrap()
        .active_turn
        .expect("read must expose ACP stop credentials");
    let stopped = test
        .state
        .collaboration
        .stop_turn(
            source,
            created.chat.clone(),
            control.turn_id.clone(),
            control.authority_epoch,
        )
        .await
        .unwrap();
    assert!(stopped.stopped);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(cancel_log.path())
                .unwrap()
                .lines()
                .next()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ACP runner did not observe session/cancel");
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), send)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let outcome = reply.outcome.expect("stopped ACP result");
    assert_ne!(outcome.status, "completed");
    assert!(outcome.partial);
    assert!(!std::fs::read_to_string(cancel_log.path())
        .unwrap()
        .trim()
        .is_empty());
    assert_eq!(
        manager
            .get(created.chat.chat_id().as_str())
            .await
            .unwrap()
            .runner,
        "mock_agent"
    );
}

#[tokio::test]
async fn acp_collaboration_process_failure_settles_and_explicit_recreate_restarts_without_task() {
    let test = build_app();
    let source = caller(&test, "acp-failure-source").await;
    let mut failing_env = std::collections::HashMap::new();
    failing_env.insert("ACP_MOCK_FAIL_ON_PROMPT".into(), "1".into());
    let failing_config = mock_acp_service(&test, failing_env);
    let handle = test
        .state
        .acp_chat_registry
        .get("default")
        .expect("default ACP runtime");
    handle.service.replace_config(failing_config);
    let child = test
        .state
        .collaboration
        .create_acp(source.clone(), "mock_agent".into(), None, 0)
        .await
        .unwrap();
    let mut first_call = source.clone();
    first_call.tool_call_id = "acp-failing-send".into();
    let failed = test
        .state
        .collaboration
        .send(first_call, child.chat.clone(), "fail now".into(), 10)
        .await
        .unwrap();
    let outcome = failed.outcome.expect("failed ACP outcome");
    assert_eq!(outcome.status, "failed");
    assert!(outcome.partial);
    let restarted = rua_server::AppState::from_tempdir(
        test.state.rua_root.clone(),
        rua_server::ServerProfile::Test,
    );
    let recovery = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(recovery.interrupted, 0);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while test
            .state
            .conversation_authority
            .active_turn(&child.chat)
            .await
            .is_some()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let working_config = mock_acp_service(&test, Default::default());
    handle.service.replace_config(working_config);
    let recreated = test
        .state
        .collaboration
        .create_acp(source.clone(), "mock_agent".into(), None, 0)
        .await
        .unwrap();
    assert_eq!(recreated.chat, child.chat);
    let mut second_call = source;
    second_call.tool_call_id = "acp-restarted-send".into();
    let completed = test
        .state
        .collaboration
        .send(second_call, child.chat.clone(), "works now".into(), 10)
        .await
        .unwrap();
    assert_eq!(completed.outcome.unwrap().status, "completed");
    assert!(test
        .state
        .task_runtime
        .list(
            &rua_task::TaskOwner::from_address(child.chat),
            rua_task::TaskListFilter::default(),
        )
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn explicit_source_configuration_update_propagates_inheritance_without_overwriting_executor()
{
    let test = build_app();
    let source = caller(&test, "configuration-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let before = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();

    let mut updated_security = before.security_config.clone();
    updated_security.tool_guard.approval_mode = "ask_first".into();
    test.state
        .security
        .save_and_invalidate("default", &updated_security)
        .unwrap();
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap()
            .epoch,
        before.epoch,
        "Agent default changes must not propagate without a source Chat apply"
    );

    let path = format!(
        "/api/agents/default/chats/{}/configuration",
        source.address.chat_id()
    );
    let preview = read_json(request(&test.app, Method::GET, &path, None).await).await;
    assert_eq!(
        preview["candidate"]["security_config"]["tool_guard"]["approval_mode"],
        "ask_first"
    );
    let candidate_epoch = preview["candidate"]["epoch"].as_str().unwrap().to_string();
    let applied = request(
        &test.app,
        Method::PUT,
        &path,
        Some(json!({
            "expected_epoch": preview["current"]["epoch"],
            "candidate_epoch": candidate_epoch,
        })),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);

    let after = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();
    assert_ne!(after.epoch, before.epoch);
    assert_eq!(after.security_config, updated_security);
    assert_eq!(after.running_config, before.running_config);
    assert_eq!(after.agent_assignment, before.agent_assignment);
    assert_eq!(after.prompt_file_content, before.prompt_file_content);
    assert_eq!(after.tool_config_snapshot, before.tool_config_snapshot);
    assert_eq!(
        after.capability_configuration,
        before.capability_configuration
    );
    assert_eq!(after.selected_agent_id, before.selected_agent_id);
    assert_eq!(
        after.model.filter(|value| !value.is_null()),
        before.model.filter(|value| !value.is_null())
    );
    assert_eq!(
        after.thinking.filter(|value| !value.is_null()),
        before.thinking.filter(|value| !value.is_null())
    );
    assert_eq!(
        after.harness_configuration.filter(|value| !value.is_null()),
        before
            .harness_configuration
            .filter(|value| !value.is_null())
    );
    assert_eq!(
        after.extra["collaboration_source_epoch"],
        serde_json::json!(candidate_epoch)
    );

    let child_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let header = child_store
        .load_chat_header(child.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let mut descriptor: rua_session::collaboration::CollaborationDescriptor =
        serde_json::from_value(header.extra["collaboration"].clone()).unwrap();
    assert!(descriptor.configuration.is_ready());
    assert_eq!(
        descriptor.configuration.applied_source_epoch,
        candidate_epoch
    );

    descriptor.configuration.status =
        rua_session::collaboration::CollaborationConfigurationStatus::Failed;
    descriptor.configuration.error = Some("fixture descriptor write failure".into());
    let mut failed_header = header;
    failed_header.extra.insert(
        "collaboration".into(),
        serde_json::to_value(&descriptor).unwrap(),
    );
    child_store.save_chat_header(&failed_header).await.unwrap();
    let child_path = format!(
        "/api/agents/default/chats/{}/configuration",
        child.chat.chat_id()
    );
    let failed_view = read_json(request(&test.app, Method::GET, &child_path, None).await).await;
    assert_eq!(failed_view["collaboration"]["status"], "failed");
    assert_eq!(
        failed_view["collaboration"]["error"],
        "fixture descriptor write failure"
    );

    let retried = request(
        &test.app,
        Method::PUT,
        &path,
        Some(json!({
            "expected_epoch": preview["current"]["epoch"],
            "candidate_epoch": candidate_epoch,
        })),
    )
    .await;
    assert_eq!(retried.status(), StatusCode::OK);
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap()
            .epoch,
        after.epoch,
        "propagation retry must not publish another child snapshot"
    );
    let recovered_view = read_json(request(&test.app, Method::GET, &child_path, None).await).await;
    assert_eq!(recovered_view["collaboration"]["status"], "ready");
    assert!(recovered_view["collaboration"].get("error").is_none());

    let error = test
        .state
        .collaboration
        .send(source, child.chat, "must use a new source Turn".into(), 0)
        .await
        .unwrap_err();
    assert!(error.contains("Source Chat configuration changed"));
}

#[tokio::test]
async fn failed_child_configuration_rejects_chat_and_task_admission_until_recovered() {
    let test = build_app();
    let source = caller(&test, "failed-configuration-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let child_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let mut header = child_store
        .load_chat_header(child.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let mut descriptor: rua_session::collaboration::CollaborationDescriptor =
        serde_json::from_value(header.extra["collaboration"].clone()).unwrap();
    descriptor.configuration.status =
        rua_session::collaboration::CollaborationConfigurationStatus::Failed;
    descriptor.configuration.error = Some("fixture propagation failure".into());
    header.extra.insert(
        "collaboration".into(),
        serde_json::to_value(&descriptor).unwrap(),
    );
    child_store.save_chat_header(&header).await.unwrap();

    let send_error = test
        .state
        .collaboration
        .send(source.clone(), child.chat.clone(), "blocked".into(), 0)
        .await
        .unwrap_err();
    assert!(send_error.contains("not ready"));
    assert!(send_error.contains("fixture propagation failure"));

    let outcome = test
        .state
        .conversation_input_intake
        .submit_delivery(
            "default",
            rua_channel::core::console::ConsoleChatRequest {
                chat_id: Some(child.chat.chat_id().to_string()),
                message: "direct human input".into(),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: Default::default(),
                channel_kind: None,
            },
        )
        .await;
    assert!(matches!(
        outcome,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected { detail, .. }
            if detail.contains("not ready")
    ));

    let current_epoch = header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    let task_error = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: rua_task::TaskOwner::from_address(child.chat.clone()),
                instruction: "must not persist".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_Blocked".into(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_configuration_epoch(current_epoch.clone()),
        )
        .await
        .unwrap_err();
    assert!(task_error.to_string().contains("not ready"));

    let source_configuration = test
        .state
        .product_hooks
        .chat_configuration(&source.address)
        .await
        .unwrap();
    test.state
        .collaboration
        .propagate_source_configuration(source.address, source_configuration)
        .await
        .unwrap();
    test.state
        .collaboration
        .ensure_configuration_ready(&child.chat)
        .await
        .unwrap();
    let stale_error = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: rua_task::TaskOwner::from_address(child.chat.clone()),
                instruction: "stale epoch must not persist".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_Stale".into(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_configuration_epoch(current_epoch),
        )
        .await
        .unwrap_err();
    assert!(stale_error.to_string().contains("stale"));
}

#[tokio::test]
async fn deleting_source_preserves_ready_child_snapshot_for_direct_input() {
    let test = build_app();
    let source = caller(&test, "deleted-configuration-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let before = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();
    WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    )
    .delete_chat_storage_by_chat_id(source.address.chat_id().as_str())
    .await
    .unwrap();

    let outcome = test
        .state
        .conversation_input_intake
        .submit_delivery(
            "default",
            rua_channel::core::console::ConsoleChatRequest {
                chat_id: Some(child.chat.chat_id().to_string()),
                message: "continue independently".into(),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: Default::default(),
                channel_kind: None,
            },
        )
        .await;
    assert!(!matches!(
        outcome,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected { .. }
    ));
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn direct_child_configuration_update_preserves_inherited_security() {
    let test = build_app();
    let source = caller(&test, "direct-child-configuration-source").await;
    let child = test
        .state
        .collaboration
        .create(source, "default".into(), None, 0)
        .await
        .unwrap();
    let before = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();
    let mut target_default = before.security_config.clone();
    target_default.tool_guard.approval_mode =
        if before.security_config.tool_guard.approval_mode == "hands_off" {
            "ask_first".into()
        } else {
            "hands_off".into()
        };
    test.state
        .security
        .save_and_invalidate("default", &target_default)
        .unwrap();

    let path = format!(
        "/api/agents/default/chats/{}/configuration",
        child.chat.chat_id()
    );
    let preview = read_json(request(&test.app, Method::GET, &path, None).await).await;
    assert_eq!(
        preview["candidate"]["security_config"],
        serde_json::to_value(&before.security_config).unwrap()
    );
    assert_eq!(preview["collaboration"]["status"], "ready");
    let applied = request(
        &test.app,
        Method::PUT,
        &path,
        Some(json!({
            "expected_epoch": preview["current"]["epoch"],
            "candidate_epoch": preview["candidate"]["epoch"],
        })),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap()
            .security_config,
        before.security_config
    );
}

#[tokio::test]
async fn startup_reconciliation_interrupts_without_replay_and_redelivers_once_after_resume() {
    let test = build_app();
    let source = caller(&test, "restart-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    target_store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: request_id.clone(),
            source: source.address.clone(),
            target: child.chat.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: "restart-call".into(),
            input_digest: "digest".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: Some("accepted-input".into()),
            turn_id: None,
            outcome: None,
            delivery: rua_session::collaboration::CollaborationDeliveryState::Undecided,
            extra: Default::default(),
        })
        .await
        .unwrap();
    let unconfirmed_request_id = uuid::Uuid::new_v4().to_string();
    target_store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: unconfirmed_request_id.clone(),
            source: source.address.clone(),
            target: child.chat.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: "restart-unconfirmed-call".into(),
            input_digest: "unconfirmed-digest".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: None,
            turn_id: None,
            outcome: None,
            delivery: rua_session::collaboration::CollaborationDeliveryState::Undecided,
            extra: Default::default(),
        })
        .await
        .unwrap();
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    source_store
        .set_collaboration_wake_suppressed(source.address.chat_id().as_str(), true)
        .await
        .unwrap();

    let restarted = rua_server::AppState::from_tempdir(
        test.state.rua_root.clone(),
        rua_server::ServerProfile::Test,
    );
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    restarted
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let first = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(first.interrupted, 2);
    assert_eq!(first.redelivered, 0);
    assert!(requests.lock().await.is_empty());
    let interrupted = target_store
        .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
        .await
        .unwrap()
        .unwrap();
    let result = interrupted.outcome.as_ref().unwrap();
    assert_eq!(result.status, "interrupted");
    assert!(result.partial);
    assert!(result.output.contains("not replayed"));
    assert_eq!(
        interrupted.delivery,
        rua_session::collaboration::CollaborationDeliveryState::Deferred
    );
    let unconfirmed = target_store
        .load_collaboration_request(child.chat.chat_id().as_str(), &unconfirmed_request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unconfirmed.outcome.as_ref().unwrap().status, "interrupted");
    assert!(unconfirmed
        .outcome
        .as_ref()
        .unwrap()
        .output
        .contains("acceptance remained unconfirmed"));

    source_store
        .set_collaboration_wake_suppressed(source.address.chat_id().as_str(), false)
        .await
        .unwrap();
    let second = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(second.interrupted, 0);
    assert_eq!(second.redelivered, 2);
    wait_for_source_requests(&requests, 1).await;
    assert_eq!(requests.lock().await.len(), 1);
    let third = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(third.redelivered, 0);
    assert_eq!(requests.lock().await.len(), 1);

    let mut fresh_caller = source.clone();
    fresh_caller.tool_call_id = "post-restart-fresh-call".into();
    let fresh = restarted
        .collaboration
        .send(
            fresh_caller,
            child.chat.clone(),
            "new work after reconciliation".into(),
            0,
        )
        .await
        .unwrap();
    assert!(fresh.request_id.is_some());
    wait_for_source_requests(&requests, 2).await;
    assert_eq!(requests.lock().await.len(), 2);
}

#[tokio::test]
async fn startup_reconciliation_reuses_claimed_identity_and_skips_consumed_results() {
    let test = build_app();
    let source = caller(&test, "restart-delivery-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let cases = [
        (
            "automatic-observation",
            rua_session::collaboration::CollaborationDeliveryState::Automatic {
                observation_id: "automatic-observation".into(),
            },
        ),
        (
            "failed-observation",
            rua_session::collaboration::CollaborationDeliveryState::DeliveryFailed {
                observation_id: "failed-observation".into(),
                error: "crashed before receipt".into(),
            },
        ),
        (
            "direct-result",
            rua_session::collaboration::CollaborationDeliveryState::Direct,
        ),
        (
            "delivered-observation",
            rua_session::collaboration::CollaborationDeliveryState::Delivered {
                observation_id: "delivered-observation".into(),
                source_turn_id: Some("source-result-turn".into()),
            },
        ),
    ];
    for (result_id, delivery) in cases {
        target_store
            .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                source: source.address.clone(),
                target: child.chat.clone(),
                origin_turn_id: source.turn_id.clone(),
                origin_tool_call_id: format!("call-{result_id}"),
                input_digest: format!("digest-{result_id}"),
                created_at: chrono::Utc::now().to_rfc3339(),
                input_id: Some(format!("input-{result_id}")),
                turn_id: Some(format!("turn-{result_id}")),
                outcome: Some(rua_session::collaboration::CollaborationOutcome {
                    result_id: result_id.into(),
                    status: "completed".into(),
                    output: format!("output-{result_id}"),
                    partial: false,
                    extra: Default::default(),
                }),
                delivery,
                extra: Default::default(),
            })
            .await
            .unwrap();
    }

    let restarted = rua_server::AppState::from_tempdir(
        test.state.rua_root.clone(),
        rua_server::ServerProfile::Test,
    );
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    restarted
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let report = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(report.redelivered, 2);
    wait_for_source_requests(&requests, 2).await;
    assert_eq!(requests.lock().await.len(), 2);
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&restarted.rua_root, source.address.clone()).root(),
    );
    let observation_ids = source_store
        .read_message_range(source.address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .filter_map(|note| match note.payload {
            rua_session::RuntimeNotePayload::CollaborationResult(_) => Some(note.runtime_note_id),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        observation_ids,
        std::collections::HashSet::from([
            "automatic-observation".to_string(),
            "failed-observation".to_string(),
        ])
    );
    let second = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(second.redelivered, 0);
    assert_eq!(requests.lock().await.len(), 2);
}

#[tokio::test]
async fn startup_reconciliation_retains_result_when_source_was_deleted_without_recreating_it() {
    let test = build_app();
    let source = caller(&test, "restart-deleted-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    target_store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: request_id.clone(),
            source: source.address.clone(),
            target: child.chat.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: "deleted-source-call".into(),
            input_digest: "deleted-source-digest".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: Some("deleted-source-input".into()),
            turn_id: Some("deleted-source-turn".into()),
            outcome: Some(rua_session::collaboration::CollaborationOutcome {
                result_id: "deleted-source-result".into(),
                status: "completed".into(),
                output: "retained output".into(),
                partial: false,
                extra: Default::default(),
            }),
            delivery: rua_session::collaboration::CollaborationDeliveryState::Deferred,
            extra: Default::default(),
        })
        .await
        .unwrap();
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    source_store
        .delete_chat_storage_by_chat_id(source.address.chat_id().as_str())
        .await
        .unwrap();

    let restarted = rua_server::AppState::from_tempdir(
        test.state.rua_root.clone(),
        rua_server::ServerProfile::Test,
    );
    let report = restarted.collaboration.reconcile_startup().await.unwrap();
    assert_eq!(report.redelivered, 0);
    assert_eq!(report.failed, 1);
    let retained = target_store
        .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.outcome.as_ref().unwrap().output, "retained output");
    assert!(matches!(
        retained.delivery,
        rua_session::collaboration::CollaborationDeliveryState::DeliveryFailed { ref error, .. }
            if error.contains("Source Chat is unavailable")
    ));
    assert!(source_store
        .load_chat_header(source.address.chat_id().as_str())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn chat_lists_separate_user_and_collaboration_views_and_keep_deleted_source_child_visible() {
    let test = build_app();
    let source = caller(&test, "conversation-view-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();

    let users = request(
        &test.app,
        Method::GET,
        "/api/agents/default/chats?view=user",
        None,
    )
    .await;
    assert_eq!(users.status(), StatusCode::OK);
    let users = read_json(users).await;
    assert!(users
        .as_array()
        .unwrap()
        .iter()
        .all(|chat| chat["id"] != child.chat.chat_id().as_str()));

    let collaboration = request(
        &test.app,
        Method::GET,
        &format!(
            "/api/agents/default/chats?view=collaboration&source_chat_id={}",
            source.address.chat_id()
        ),
        None,
    )
    .await;
    assert_eq!(collaboration.status(), StatusCode::OK);
    let collaboration = read_json(collaboration).await;
    let listed = collaboration
        .as_array()
        .unwrap()
        .iter()
        .find(|chat| chat["id"] == child.chat.chat_id().as_str())
        .expect("collaboration child in collaboration view");
    assert_eq!(
        listed["collaboration"]["source"],
        serde_json::to_value(&source.address).unwrap()
    );
    assert_eq!(listed["collaboration"]["executor"]["kind"], "agent");
    assert_eq!(listed["collaboration"]["source_available"], true);

    let child_scope = ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone());
    let child_store = WorkspaceSessionStore::new(child_scope.root());
    let header_before = child_store
        .load_chat_header(child.chat.chat_id().as_str())
        .await
        .unwrap();
    rua_session::chat_index::delete(child_scope.root(), child.chat.chat_id().as_str())
        .await
        .unwrap();
    rua_chat::ChatService::new(&test.state.rua_root)
        .repair_projection(&child.chat)
        .await
        .unwrap();
    assert_eq!(
        child_store
            .load_chat_header(child.chat.chat_id().as_str())
            .await
            .unwrap(),
        header_before
    );
    let rebuilt = read_json(
        request(
            &test.app,
            Method::GET,
            "/api/agents/default/chats?view=collaboration",
            None,
        )
        .await,
    )
    .await;
    assert!(rebuilt
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == child.chat.chat_id().as_str()));

    WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    )
    .delete_chat_storage_by_chat_id(source.address.chat_id().as_str())
    .await
    .unwrap();
    let after_delete = read_json(
        request(
            &test.app,
            Method::GET,
            "/api/agents/default/chats?view=collaboration",
            None,
        )
        .await,
    )
    .await;
    let listed = after_delete
        .as_array()
        .unwrap()
        .iter()
        .find(|chat| chat["id"] == child.chat.chat_id().as_str())
        .expect("child remains discoverable after source deletion");
    assert_eq!(listed["collaboration"]["source_available"], false);
}

#[tokio::test]
async fn chat_read_progress_summary_is_bounded_private_incremental_and_singleflight() {
    let test = build_app();
    let mut source = caller(&test, "progress-source").await;
    let source_scope = ChatScopeStore::resolve(&test.state.rua_root, source.address.clone());
    rua_chat::ChatService::for_resolved_scope(source_scope.root(), source.address.clone())
        .update_setting(
            &source.address,
            rua_chat::ChatSetting::Model(json!({"provider_id":"fixture","model":"progress-model"})),
        )
        .await
        .unwrap();
    let source_header = WorkspaceSessionStore::new(source_scope.root())
        .load_chat_header(source.address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    source.configuration_epoch = source_header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let summarizer = Arc::new(RecordingProgressSummarizer::successful("S".repeat(900)));
    let release = Arc::new(tokio::sync::Notify::new());
    *summarizer.release.lock().await = Some(release.clone());
    test.state
        .collaboration
        .install_progress_summarizer(summarizer.clone());
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: request_id.clone(),
            source: source.address.clone(),
            target: child.chat.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: "progress-call".into(),
            input_digest: "progress-digest".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: Some("progress-input".into()),
            turn_id: Some("progress-running-turn".into()),
            outcome: None,
            delivery: rua_session::collaboration::CollaborationDeliveryState::Deferred,
            extra: Default::default(),
        })
        .await
        .unwrap();
    for ordinal in 0..30 {
        store
            .append_conversation_turn(
                child.chat.chat_id().as_str(),
                rua_session::NewConversationTurn {
                    turn_id: format!("progress-turn-{ordinal}"),
                    lifecycle: rua_session::TurnLifecycle {
                        user_submitted_at: format!("2026-09-21T00:00:{ordinal:02}Z"),
                        assistant_started_at: Some(format!("2026-09-21T00:01:{ordinal:02}Z")),
                        assistant_completed_at: Some(format!("2026-09-21T00:02:{ordinal:02}Z")),
                        status: "completed".into(),
                    },
                    selected_variant_id: None,
                    messages: vec![rua_session::NewConversationMessage {
                        message_id: format!("progress-message-{ordinal}"),
                        role: "assistant".into(),
                        created_at: format!("2026-09-21T00:02:{ordinal:02}Z"),
                        status: "completed".into(),
                        content: json!([
                            {"type":"thinking","thinking":format!("PRIVATE_REASONING_{ordinal}")},
                            {"type":"text","text":format!("confirmed finding {ordinal} {}", "v".repeat(900))},
                            {"type":"tool_result","content":format!("RAW_TOOL_SECRET_{ordinal} {}", "x".repeat(3000))}
                        ]),
                        provider: None,
                        tool: Some(rua_session::MessageToolLink {
                            tool_name: "PublicTool".into(),
                            tool_call_id: Some(format!("private-call-{ordinal}")),
                        }),
                        metadata: serde_json::Map::new(),
                    }],
                },
            )
            .await
            .unwrap();
    }

    let left_service = Arc::clone(&test.state.collaboration);
    let left_source = source.clone();
    let left_target = child.chat.clone();
    let left = tokio::spawn(async move { left_service.read(left_source, left_target, 1).await });
    let right_service = Arc::clone(&test.state.collaboration);
    let right_source = source.clone();
    let right_target = child.chat.clone();
    let right =
        tokio::spawn(async move { right_service.read(right_source, right_target, 1).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while summarizer.calls.lock().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(summarizer.calls.lock().await.len(), 1);
    release.notify_waiters();
    let left = left.await.unwrap().unwrap().progress.unwrap();
    let right = right.await.unwrap().unwrap().progress.unwrap();
    assert_eq!(left, right);
    assert_eq!(left.summary.as_ref().unwrap().chars().count(), 512);
    let calls = summarizer.calls.lock().await;
    let initial_activity_len = calls[0].activity.len();
    assert!(initial_activity_len <= 12_000);
    assert!(calls[0].activity.contains("confirmed finding"));
    assert!(calls[0].activity.contains("PublicTool (completed)"));
    assert!(!calls[0].activity.contains("PRIVATE_REASONING"));
    assert!(!calls[0].activity.contains("RAW_TOOL_SECRET"));
    assert!(!calls[0].activity.contains("private-call"));
    assert_eq!(
        calls[0]
            .model_slot
            .as_ref()
            .and_then(|slot| slot.get("model")),
        Some(&json!("progress-model"))
    );
    drop(calls);

    let cached = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert_eq!(cached.progress.unwrap(), left);
    assert_eq!(summarizer.calls.lock().await.len(), 1);

    *summarizer.release.lock().await = None;
    store
        .append_conversation_turn(
            child.chat.chat_id().as_str(),
            rua_session::NewConversationTurn {
                turn_id: "progress-turn-new".into(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-09-21T01:00:00Z".into(),
                    assistant_started_at: Some("2026-09-21T01:00:01Z".into()),
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![rua_session::NewConversationMessage {
                    message_id: "progress-message-new".into(),
                    role: "assistant".into(),
                    created_at: "2026-09-21T01:00:01Z".into(),
                    status: "in_progress".into(),
                    content: json!([{"type":"text","text":"new observed step"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    let updated = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 1)
        .await
        .unwrap()
        .progress
        .unwrap();
    assert_ne!(updated.activity_version, left.activity_version);
    let calls = summarizer.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].previous_summary.as_deref(),
        left.summary.as_deref()
    );
    assert!(calls[1].activity.contains("new observed step"));
    assert!(calls[1].activity.len() < initial_activity_len);
    drop(calls);

    *summarizer.response.lock().await = Err("summary provider failed".into());
    store
        .set_turn_lifecycle(
            child.chat.chat_id().as_str(),
            "progress-turn-new",
            rua_session::TurnLifecycle {
                user_submitted_at: "2026-09-21T01:00:00Z".into(),
                assistant_started_at: Some("2026-09-21T01:00:01Z".into()),
                assistant_completed_at: Some("2026-09-21T01:00:02Z".into()),
                status: "completed".into(),
            },
        )
        .await
        .unwrap();
    let stale = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 1)
        .await
        .unwrap()
        .progress
        .unwrap();
    assert!(stale.stale);
    assert_eq!(stale.summary, updated.summary);
    assert!(stale.error.unwrap().contains("provider failed"));
    *summarizer.response.lock().await = Ok("recovered summary".into());
    let recovered = test
        .state
        .collaboration
        .read(source, child.chat, 1)
        .await
        .unwrap()
        .progress
        .unwrap();
    assert!(!recovered.stale);
    assert_eq!(recovered.summary.as_deref(), Some("recovered summary"));
}

#[tokio::test]
async fn chat_read_progress_timeout_does_not_cancel_child_and_final_result_skips_summary() {
    let test = build_app();
    let source = caller(&test, "progress-timeout-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let summarizer = Arc::new(RecordingProgressSummarizer::successful("unused"));
    *summarizer.release.lock().await = Some(Arc::new(tokio::sync::Notify::new()));
    test.state
        .collaboration
        .install_progress_summarizer(summarizer.clone());
    test.state
        .collaboration
        .set_progress_summary_timeout(std::time::Duration::from_millis(25));
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut record = rua_session::collaboration::CollaborationRequest {
        request_id: request_id.clone(),
        source: source.address.clone(),
        target: child.chat.clone(),
        origin_turn_id: source.turn_id.clone(),
        origin_tool_call_id: "progress-timeout-call".into(),
        input_digest: "progress-timeout-digest".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        input_id: Some("progress-timeout-input".into()),
        turn_id: Some("progress-timeout-turn".into()),
        outcome: None,
        delivery: rua_session::collaboration::CollaborationDeliveryState::Deferred,
        extra: Default::default(),
    };
    store.save_collaboration_request(&record).await.unwrap();
    let immediate = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert!(immediate.progress.unwrap().summary.is_none());
    assert!(summarizer.calls.lock().await.is_empty());
    let timed_out = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 1)
        .await
        .unwrap();
    let progress = timed_out.progress.unwrap();
    assert!(matches!(
        progress.status,
        rua_server::services::collaboration_progress::CollaborationProgressStatus::Unavailable
    ));
    assert!(progress.error.unwrap().contains("timed out"));
    assert!(store
        .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
        .await
        .unwrap()
        .unwrap()
        .outcome
        .is_none());
    assert_eq!(summarizer.calls.lock().await.len(), 1);

    test.state
        .collaboration
        .set_progress_summary_timeout(std::time::Duration::from_secs(15));
    let started = std::time::Instant::now();
    let deadline_reply = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 1)
        .await
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(deadline_reply
        .progress
        .unwrap()
        .error
        .unwrap()
        .contains("timed out"));

    record.outcome = Some(rua_session::collaboration::CollaborationOutcome {
        result_id: "progress-timeout-result".into(),
        status: "completed".into(),
        output: "final output".into(),
        partial: false,
        extra: Default::default(),
    });
    store.save_collaboration_request(&record).await.unwrap();
    let final_reply = test
        .state
        .collaboration
        .read(source, child.chat, 0)
        .await
        .unwrap();
    assert_eq!(final_reply.outcome.unwrap().output, "final output");
    assert!(final_reply.progress.is_none());
    assert_eq!(summarizer.calls.lock().await.len(), 2);
}

#[tokio::test]
async fn chat_read_deadline_includes_waiting_for_authority_routing() {
    let test = build_app();
    let source = caller(&test, "routing-deadline-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let scope = ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    store
        .update_chat_header(child.chat.chat_id().as_str(), |header| {
            header.extra["authority_placement"]["device_id"] = json!("remote-device");
        })
        .await
        .unwrap();
    let routing_lock = test.state.device_network.lock().await;
    let started = std::time::Instant::now();
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        test.state.collaboration.read(source, child.chat.clone(), 1),
    )
    .await
    .expect("read must bound routing lock wait");
    let error = reply.unwrap_err();
    assert!(error.contains("deadline exceeded"), "{error}");
    assert!(error.contains("not cancelled"));
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    drop(routing_lock);
    assert!(store
        .list_collaboration_requests(child.chat.chat_id().as_str())
        .await
        .unwrap()
        .is_empty());
    assert!(test
        .state
        .conversation_authority
        .active_turn(&child.chat)
        .await
        .is_none());
}

#[tokio::test]
async fn send_yields_without_cancelling_and_deduplicates_execution() {
    let test = build_app();
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id":"reviewer","name":"Reviewer"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let source = caller(&test, "yield-source").await;
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            release: None,
        }))
        .await;
    let created = test
        .state
        .collaboration
        .create(source.clone(), "reviewer".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("reviewer")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;
    let mut invocation = source.clone();
    invocation.tool_call_id = "send-one".into();
    let receipt = test
        .state
        .collaboration
        .send(
            invocation.clone(),
            created.chat.clone(),
            "review only this input".into(),
            0,
        )
        .await
        .unwrap();
    assert!(receipt.outcome.is_none());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        requests.lock().await[0].working_dir.as_ref(),
        Some(&source.working_dir)
    );
    release.notify_one();
    let result = test
        .state
        .collaboration
        .send(
            invocation,
            created.chat.clone(),
            "review only this input".into(),
            5,
        )
        .await
        .unwrap();
    assert_eq!(requests.lock().await.len(), 1);
    assert_eq!(receipt.request_id, result.request_id);
    assert_eq!(
        requests.lock().await[0]
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, created.chat.clone()).root(),
    );
    let persisted = store
        .load_collaboration_request(
            created.chat.chat_id().as_str(),
            result.request_id.as_deref().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(result.outcome.is_none());
    assert_eq!(
        persisted
            .outcome
            .as_ref()
            .map(|outcome| outcome.output.as_str()),
        Some("review complete")
    );
    assert_eq!(result.state, "running");
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    let source_config = source_store
        .load_configuration_snapshot(
            source.address.chat_id().as_str(),
            &source.configuration_epoch,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        requests.lock().await[0].metadata
            [rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY],
        source_config["security_config"]
    );
    let source_notes = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notes = source_store
                .read_message_range(source.address.chat_id().as_str(), 1, usize::MAX)
                .await
                .unwrap()
                .into_iter()
                .filter_map(|message| {
                    let note = rua_session::RuntimeNote::from_message_log_entry(&message)?;
                    matches!(
                        note.payload,
                        rua_session::RuntimeNotePayload::CollaborationResult(_)
                    )
                    .then_some((message, note))
                })
                .collect::<Vec<_>>();
            if !notes.is_empty() {
                break notes;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(source_notes.len(), 1);
    assert_eq!(source_notes[0].0.role, "runtime");
    let rua_session::RuntimeNotePayload::CollaborationResult(delivered) =
        &source_notes[0].1.payload
    else {
        unreachable!()
    };
    assert_eq!(delivered.output, "review complete");
    assert_eq!(delivered.request_id, receipt.request_id.as_deref().unwrap());
    let delivered_record = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = store
                .load_collaboration_request(
                    created.chat.chat_id().as_str(),
                    receipt.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if matches!(
                record.delivery,
                rua_session::collaboration::CollaborationDeliveryState::Delivered { .. }
            ) {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        delivered_record.delivery,
        rua_session::collaboration::CollaborationDeliveryState::Delivered { .. }
    ));
}

#[tokio::test]
async fn collaboration_first_input_preserves_custom_title() {
    let test = build_app();
    let source = caller(&test, "custom-title-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let scope = ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone());
    let service = rua_chat::ChatService::for_resolved_scope(scope.root(), child.chat.clone());
    service
        .update_listing(
            &child.chat,
            rua_chat::ChatListUpdate {
                title: Some("My review".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            release: None,
        }))
        .await;
    let result = test
        .state
        .collaboration
        .send(source, child.chat.clone(), "first input".into(), 5)
        .await
        .unwrap();
    assert_eq!(result.state, "completed");
    assert_eq!(
        WorkspaceSessionStore::new(scope.root())
            .load_chat_header(child.chat.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .metadata
            .title,
        "My review"
    );
}

#[tokio::test]
async fn quick_send_returns_final_result_and_changed_input_is_not_reexecuted() {
    let test = build_app();
    let source = caller(&test, "quick-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let reply = test
        .state
        .collaboration
        .send(source.clone(), child.chat.clone(), "quick input".into(), 5)
        .await
        .unwrap();
    assert_eq!(reply.state, "completed");
    assert_eq!(reply.outcome.unwrap().output, "review complete");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    assert!(source_store
        .read_message_range(source.address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .all(|note| !matches!(
            note.payload,
            rua_session::RuntimeNotePayload::CollaborationResult(_)
        )));
    assert!(test
        .state
        .collaboration
        .send(source, child.chat, "changed input".into(), 0)
        .await
        .unwrap_err()
        .contains("different input"));
    assert_eq!(requests.lock().await.len(), 1);
}

#[tokio::test]
async fn registered_tools_create_and_send_using_trusted_call_context() {
    let test = build_app();
    let source = caller(&test, "tool-source").await;
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    let toolkit = runtime.task_toolkit().await.unwrap();
    let create_definition = toolkit
        .get_definition("chat_send")
        .expect("registered chat_send definition");
    assert_eq!(
        create_definition.parameters["oneOf"],
        json!([
            {"required":["chat"],"not":{"anyOf":[{"required":["agent_id"]},{"required":["acp_runner"]}]}},
            {"required":["agent_id"],"not":{"anyOf":[{"required":["chat"]},{"required":["acp_runner"]}]}},
            {"required":["acp_runner"],"not":{"anyOf":[{"required":["chat"]},{"required":["agent_id"]}]}}
        ])
    );
    assert_eq!(
        create_definition.parameters["properties"]["yield_seconds"],
        json!({"type":"integer","minimum":0,"maximum":30,"default":10})
    );
    let send_definition = toolkit
        .get_definition("chat_send")
        .expect("registered chat_send definition");
    assert_eq!(send_definition.parameters["required"], json!(["message"]));
    let read_definition = toolkit
        .get_definition("chat_read")
        .expect("registered chat_read definition");
    assert_eq!(read_definition.parameters["required"], json!(["chat"]));
    let stop_definition = toolkit
        .get_definition("chat_stop")
        .expect("registered chat_stop definition");
    assert_eq!(
        stop_definition.parameters["required"],
        json!(["chat", "turn_id", "authority_epoch"])
    );
    let create = toolkit
        .get_executor("chat_send")
        .expect("registered chat_send");
    let task_owner = rua_task::TaskOwner::new("default", source.address.chat_id().as_str());
    assert!(test
        .state
        .task_runtime
        .list(&task_owner, Default::default())
        .await
        .unwrap()
        .is_empty());
    let call = agent_types::ToolCall::new(
        "create-tool",
        "chat_send",
        json!({"agent_id":"default","message":"initial request","yield_seconds":5}),
    );
    assert!(create.execute(&call, &Default::default()).await.is_err());
    let mut context = rua_core::tool::ToolContext {
        agent_id: "default".into(),
        session_id: source.address.chat_id().to_string(),
        working_dir: source.working_dir.clone(),
        collaboration_runtime: Some(Arc::clone(&test.state.collaboration_runtime)),
        ..Default::default()
    };
    context
        .metadata
        .insert("security_chat_address".into(), json!(source.address));
    context
        .metadata
        .insert("turn_id".into(), json!(source.turn_id));
    context.metadata.insert(
        "chat_configuration_epoch".into(),
        json!(source.configuration_epoch),
    );
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    runtime
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let result = create.execute(&call, &context).await.unwrap();
    assert!(!result.is_error, "{}", result.text_content());
    let created: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(created["state"], "completed");
    assert_eq!(created["request_progress"]["phase"], "terminal");
    assert!(toolkit.get_executor("chat_create").is_none());
    assert!(toolkit.get_executor("chat_stop_turn").is_none());
    let send = toolkit
        .get_executor("chat_send")
        .expect("registered chat_send");
    let call = agent_types::ToolCall::new(
        "send-tool",
        "chat_send",
        json!({"chat":created["chat"],"message":"check input","yield_seconds":5}),
    );
    let (first, second) =
        tokio::join!(send.execute(&call, &context), send.execute(&call, &context));
    let first: serde_json::Value = serde_json::from_str(&first.unwrap().text_content()).unwrap();
    let second: serde_json::Value = serde_json::from_str(&second.unwrap().text_content()).unwrap();
    assert_eq!(first["request_id"], second["request_id"]);
    assert_eq!(first["state"], "completed");
    assert_eq!(second["state"], "completed");
    assert!(first["input_id"].as_str().is_some());
    assert!(first["turn_id"].as_str().is_some());
    assert_eq!(first["request_progress"]["phase"], "terminal");
    assert!(first.get("progress").is_none());
    assert_eq!(first["outcome"]["output"], "review complete");
    assert_eq!(requests.lock().await.len(), 2);
    let other = caller(&test, "unrelated-tool-source").await;
    context.session_id = other.address.chat_id().to_string();
    context
        .metadata
        .insert("security_chat_address".into(), json!(other.address));
    context
        .metadata
        .insert("turn_id".into(), json!(other.turn_id));
    context.metadata.insert(
        "chat_configuration_epoch".into(),
        json!(other.configuration_epoch),
    );
    let read = toolkit.get_executor("chat_read").unwrap();
    let rejected = read
        .execute(
            &agent_types::ToolCall::new(
                "unauthorized-read",
                "chat_read",
                json!({"chat": created["chat"],"yield_seconds":0}),
            ),
            &context,
        )
        .await
        .unwrap();
    assert!(rejected.is_error);
    assert_eq!(
        rejected.text_content(),
        "Target collaboration belongs to another source Chat"
    );
    assert!(test
        .state
        .task_runtime
        .list(&task_owner, Default::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn agent_tool_toggle_filters_chat_tools_in_new_configuration_epochs() {
    let test = build_app();
    let mut catalog = (*test.state.tool_catalog).clone();
    catalog["chat_send"]["enabled"] = json!(false);
    test.state
        .agent_tools_cells
        .cell("default")
        .write_value(json!({"builtin_tools": catalog}))
        .await
        .unwrap();

    let source = caller(&test, "chat-tool-filter").await;
    let configuration = test
        .state
        .product_hooks
        .chat_configuration(&source.address)
        .await
        .expect("Chat configuration");
    let tools = &configuration
        .capability_configuration
        .expect("capability configuration")
        .tools;
    assert!(!tools.iter().any(|name| name == "chat_send"));
    assert!(tools.iter().any(|name| name == "chat_read"));
}

#[tokio::test]
async fn research_preset_collaboration_keeps_readonly_capability_inventory() {
    let test = build_app();
    let source = caller(&test, "research-preset-source").await;
    let child = test
        .state
        .collaboration
        .create(source, "research".into(), None, 0)
        .await
        .unwrap();
    let configuration = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();
    let capabilities = configuration.capability_configuration.unwrap();
    assert!(capabilities.tools.iter().any(|name| name == "read"));
    let allowed = [
        "read",
        "grep",
        "glob",
        "web_search",
        "web_fetch",
        "conversation_search",
        "conversation_read_span",
    ];
    assert!(
        capabilities
            .tools
            .iter()
            .all(|name| allowed.contains(&name.as_str())),
        "{:?}",
        capabilities.tools
    );
    assert!(capabilities.mcp_servers.is_empty());
    assert!(capabilities.plugin_ids.is_empty());
    let assignment = configuration.agent_assignment.unwrap();
    for tool in [
        "bash",
        "edit",
        "python",
        "task_create",
        "chat_send",
        "memory_write",
    ] {
        assert!(
            assignment.disabled_tools.contains(&tool.to_string()),
            "{tool}"
        );
    }
}

#[tokio::test]
async fn implementation_preset_retains_source_security_and_scoped_tools() {
    let test = build_app();
    let source = caller(&test, "implementation-preset-source").await;
    let parent = test
        .state
        .product_hooks
        .chat_configuration(&source.address)
        .await
        .unwrap();
    let child = test
        .state
        .collaboration
        .create(source, "implementation".into(), None, 0)
        .await
        .unwrap();
    let configuration = test
        .state
        .product_hooks
        .chat_configuration(&child.chat)
        .await
        .unwrap();
    assert_eq!(configuration.security_config, parent.security_config);
    let capabilities = configuration.capability_configuration.unwrap();
    for tool in ["read", "edit", "bash", "task_create", "task_read"] {
        assert!(capabilities.tools.contains(&tool.to_string()), "{tool}");
    }
    assert!(!capabilities.tools.contains(&"chat_send".into()));
    assert!(!capabilities.tools.contains(&"sync_send".into()));
}

#[tokio::test]
async fn collaboration_inventory_uses_current_configured_agents_and_tools() {
    let test = build_app();
    let inventory = rua_server::services::console_chat::collaboration_agent_inventory(
        &test.state.agent_service,
    )
    .await
    .unwrap();
    let research = inventory
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agent_id"] == "research")
        .unwrap();
    assert!(research["configured_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("read")));
    assert!(!research["configured_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("bash")));
    let mut tools = test
        .state
        .agent_tools_cells
        .cell("research")
        .read_value()
        .await
        .unwrap()
        .unwrap();
    tools["builtin_tools"]["read"]["enabled"] = false.into();
    test.state
        .agent_tools_cells
        .cell("research")
        .write_value(tools)
        .await
        .unwrap();
    let inventory = rua_server::services::console_chat::collaboration_agent_inventory(
        &test.state.agent_service,
    )
    .await
    .unwrap();
    let research = inventory
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agent_id"] == "research")
        .unwrap();
    assert!(!research["configured_tools"]
        .as_array()
        .unwrap()
        .contains(&json!("read")));
}

#[tokio::test]
async fn disabled_collaboration_executor_rejects_new_input_without_persisting_request() {
    let test = build_app();
    let source = caller(&test, "disabled-executor-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "research".into(), None, 0)
        .await
        .unwrap();
    let disabled = request(
        &test.app,
        Method::PATCH,
        "/api/agents/research/toggle",
        Some(json!({"enabled":false})),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let error = test
        .state
        .collaboration
        .send(source, child.chat.clone(), "must not start".into(), 0)
        .await
        .unwrap_err();
    assert!(error.contains("disabled"));
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    assert!(store
        .list_collaboration_requests(child.chat.chat_id().as_str())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn collaboration_presets_enforce_permissions_through_real_task_execution() {
    use rua_tool::ToolTaskExecution;
    let test = build_app();
    let source = caller(&test, "preset-execution-source").await;
    let registry: Arc<dyn rua_workspace::WorkspaceRegistry> = test.state.registry.clone();
    let execution = rua_workspace::task_tool_execution::WorkspaceToolTaskExecution::new(
        Arc::downgrade(&registry),
    );
    for agent in ["research", "implementation"] {
        let mut invocation = source.clone();
        invocation.tool_call_id = format!("preset-{agent}");
        let child = test
            .state
            .collaboration
            .create(invocation, agent.into(), None, 0)
            .await
            .unwrap();
        let configuration = test
            .state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap();
        let workspace = test
            .state
            .registry
            .get_or_create(agent)
            .await
            .unwrap()
            .workspace_dir();
        let target = workspace.join("controlled-test.txt");
        let request = rua_tool::ToolTaskExecutionRequest {
            task_id: format!("controlled-{agent}"),
            attempt_no: 1,
            owner: rua_task::TaskOwner::from_address(child.chat.clone()),
            authorization_agent_id: agent.into(),
            configuration_epoch: Some(configuration.epoch),
            tool_name: "edit".into(),
            arguments: json!({"patch":format!("*** Begin Patch\n*** Add File: {}\n+verified\n*** End Patch", target.display())}),
            origin_turn_id: None,
            dependencies: Vec::new(),
            cancel: tokio_util::sync::CancellationToken::new(),
            session: rua_task::TaskRunnerSession::bound(format!("controlled-{agent}"), 1),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            execution.execute(request),
        )
        .await
        .expect("controlled execution must not wait indefinitely");
        if agent == "research" {
            assert!(result.unwrap_err().contains("disabled"));
            assert!(!target.exists());
            let readable = workspace.join("read-fixture.txt");
            rua_config::atomic_write_json(&readable, &json!("research can inspect evidence"))
                .unwrap();
            let read = execution
                .execute(rua_tool::ToolTaskExecutionRequest {
                    task_id: "research-read".into(),
                    attempt_no: 1,
                    owner: rua_task::TaskOwner::from_address(child.chat.clone()),
                    authorization_agent_id: agent.into(),
                    configuration_epoch: Some(
                        test.state
                            .product_hooks
                            .chat_configuration(&child.chat)
                            .await
                            .unwrap()
                            .epoch,
                    ),
                    tool_name: "read".into(),
                    arguments: json!({"file_path":readable}),
                    origin_turn_id: None,
                    dependencies: Vec::new(),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    session: rua_task::TaskRunnerSession::bound("research-read", 1),
                })
                .await
                .unwrap();
            assert!(!read.is_error, "{}", read.text_content());
            assert!(read
                .text_content()
                .contains("research can inspect evidence"));
        } else {
            let result = result.unwrap();
            assert!(!result.is_error, "{}", result.text_content());
            assert_eq!(
                tokio::fs::read_to_string(&target).await.unwrap(),
                "verified\n"
            );
            let verification = execution.execute(rua_tool::ToolTaskExecutionRequest {
                task_id: "implementation-verify".into(), attempt_no: 1,
                owner: rua_task::TaskOwner::from_address(child.chat.clone()),
                authorization_agent_id: agent.into(),
                configuration_epoch: Some(test.state.product_hooks.chat_configuration(&child.chat).await.unwrap().epoch),
                tool_name: "bash".into(), arguments: json!({"command":"test -f controlled-test.txt && printf 'verification passed'", "cwd": workspace}),
                origin_turn_id: None, dependencies: Vec::new(), cancel: tokio_util::sync::CancellationToken::new(),
                session: rua_task::TaskRunnerSession::bound("implementation-verify", 1),
            }).await.unwrap();
            assert!(!verification.is_error, "{}", verification.text_content());
            assert!(verification.text_content().contains("verification passed"));
        }
    }
}

#[tokio::test]
async fn task_approval_wait_is_visible_and_decision_preserves_write_boundary() {
    use rua_contract::Tool;
    use rua_tool::extended::task_tools::{
        TaskCreateTool, TaskListTool, TaskReadTool, TaskRuntimeHandle,
    };
    let test = build_app();
    let mut security = rua_config::types::security::SecurityConfig::default();
    security.tool_guard.approval_mode = "ask_first".into();
    test.state
        .security
        .save_and_invalidate("default", &security)
        .unwrap();
    let approvals = axum::Router::new()
        .nest("/api", rua_server::endpoints::approvals::router())
        .layer(axum::middleware::from_fn_with_state(
            test.state.clone(),
            rua_server::middleware::agent_context_middleware,
        ))
        .with_state(test.state.clone());
    for decision in ["run_once", "reject", "cancel"] {
        let source = caller(&test, &format!("task-approval-{decision}")).await;
        let target = source.working_dir.join(format!("approval-{decision}.txt"));
        let child = test
            .state
            .collaboration
            .create(source, "implementation".into(), None, 0)
            .await
            .unwrap();
        let config = test
            .state
            .product_hooks
            .chat_configuration(&child.chat)
            .await
            .unwrap();
        assert_eq!(config.security_config.tool_guard.approval_mode, "ask_first");
        let ctx = rua_core::tool::ToolContext {
            agent_id: "implementation".into(),
            session_id: child.chat.chat_id().as_str().into(),
            working_dir: target.parent().unwrap().into(),
            metadata: std::collections::HashMap::from([
                ("chat_configuration_epoch".into(), json!(config.epoch)),
                ("turn_id".into(), json!("task-approval-turn")),
                ("tool_use_id".into(), json!(format!("create-{decision}"))),
            ]),
            task_runtime: Some(Arc::new(TaskRuntimeHandle::new(
                test.state.task_runtime.clone(),
            ))),
            ..Default::default()
        };
        let created = TaskCreateTool.call(json!({"runner":{"type":"tool","tool_name":"edit","arguments":{
            "patch":format!("*** Begin Patch\n*** Add File: {}\n+approved\n*** End Patch", target.display())
        }}}), ctx.clone()).await.unwrap();
        let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
        let task_id = created["task_id"].as_str().unwrap();
        let owner = rua_task::TaskOwner::from_address(child.chat.clone());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let record = test.state.task_runtime.get(&owner, task_id).await.unwrap();
                assert!(
                    !record.state.is_terminal(),
                    "Task finished before approval: {record:?}"
                );
                if record.state == rua_task::TaskState::WaitingApproval {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(!target.exists());
        assert!(!test
            .state
            .task_runtime
            .set_waiting_approval(&owner, task_id, 99, false)
            .await
            .unwrap());
        assert!(test
            .state
            .task_runtime
            .set_waiting_approval(
                &rua_task::TaskOwner::new("other", "other"),
                task_id,
                1,
                false
            )
            .await
            .is_err());
        for tool in [&TaskReadTool as &dyn Tool, &TaskListTool] {
            let output = tool
                .call(json!({"task_id":task_id}), ctx.clone())
                .await
                .unwrap();
            assert!(output.text_content().contains("waiting_approval"));
            assert!(
                rua_tool::result_store::estimate_text_tokens(&output.text_content(), 4.0) < 10_000
            );
        }
        let pending = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let response = request(
                    &approvals,
                    Method::GET,
                    &format!(
                        "/api/agents/implementation/approvals/snapshot?chat_id={}",
                        child.chat.chat_id()
                    ),
                    None,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let body = read_json(response).await;
                if let Some(pending) = body["pending"].as_array().and_then(|items| items.first()) {
                    assert_eq!(
                        body["pending"].as_array().unwrap().len(),
                        1,
                        "Task must reuse the single Tool approval request"
                    );
                    assert_eq!(pending["tool_name"], "edit");
                    assert_eq!(
                        pending["tool_call_id"],
                        format!("task:{task_id}:attempt:1:tool")
                    );
                    break pending["id"].as_str().unwrap().to_string();
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        if decision == "cancel" {
            test.state
                .task_runtime
                .cancel(&owner, task_id)
                .await
                .unwrap();
        }
        let response = request(
            &approvals,
            Method::POST,
            &format!("/api/agents/implementation/approvals/{pending}/decide"),
            Some(json!({"decision":if decision == "cancel" { "run_once" } else { decision },"decided_by":"test"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let finished = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let record = test.state.task_runtime.get(&owner, task_id).await.unwrap();
                if record.state.is_terminal() {
                    break record;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            finished.state,
            if decision == "run_once" {
                rua_task::TaskState::Completed
            } else if decision == "cancel" {
                rua_task::TaskState::Cancelled
            } else {
                rua_task::TaskState::Failed
            }
        );
        assert_eq!(target.exists(), decision == "run_once");
        assert!(!test
            .state
            .task_runtime
            .set_waiting_approval(&owner, task_id, 1, false)
            .await
            .unwrap());
    }
}

#[tokio::test]
async fn chat_read_waits_without_intervening_and_does_not_repeat_automatic_result() {
    let test = build_app();
    let source = caller(&test, "read-source").await;
    let other = caller(&test, "other-read-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;

    let receipt = test
        .state
        .collaboration
        .send(
            source.clone(),
            child.chat.clone(),
            "read me later".into(),
            0,
        )
        .await
        .unwrap();
    let pending = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert_eq!(pending.state, "running");
    assert!(pending.outcome.is_none());
    assert_eq!(requests.lock().await.len(), 1);
    assert!(test
        .state
        .collaboration
        .read(other, child.chat.clone(), 0)
        .await
        .unwrap_err()
        .contains("another source"));

    release.notify_one();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    release.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    receipt.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if matches!(
                record.delivery,
                rua_session::collaboration::CollaborationDeliveryState::Delivered { .. }
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let reviewed = test
        .state
        .collaboration
        .read(source, child.chat, 0)
        .await
        .unwrap();
    assert_eq!(reviewed.state, "completed");
    assert!(reviewed.outcome.is_none());
    assert!(reviewed.result_ref.is_some());
}

#[tokio::test]
async fn chat_read_can_claim_an_undelivered_result_once() {
    let test = build_app();
    let source = caller(&test, "claim-read-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: request_id.clone(),
            source: source.address.clone(),
            target: child.chat.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: source.tool_call_id.clone(),
            input_digest: "read-fixture".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: Some("input".into()),
            turn_id: Some("turn".into()),
            outcome: Some(rua_session::collaboration::CollaborationOutcome {
                result_id: "read-result".into(),
                status: "completed".into(),
                output: "read-only delivery".into(),
                partial: false,
                extra: Default::default(),
            }),
            delivery: rua_session::collaboration::CollaborationDeliveryState::Deferred,
            extra: Default::default(),
        })
        .await
        .unwrap();

    let first = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert_eq!(first.outcome.as_ref().unwrap().output, "read-only delivery");
    let retry = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert_eq!(retry.outcome.as_ref().unwrap().output, "read-only delivery");
    let delivery = store
        .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
        .await
        .unwrap()
        .unwrap()
        .delivery;
    let mut later = source;
    later.tool_call_id = "new-read-after-lost-reply".into();
    let reviewed = test
        .state
        .collaboration
        .read(later.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    assert!(reviewed.outcome.is_none());
    let recovered = test
        .state
        .collaboration
        .read_result(later, child.chat.clone(), reviewed.result_ref.unwrap())
        .await
        .unwrap();
    assert_eq!(recovered.outcome.unwrap().output, "read-only delivery");
    assert_eq!(
        store
            .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        delivery
    );
    assert!(matches!(
        store
            .load_collaboration_request(child.chat.chat_id().as_str(), &request_id)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        rua_session::collaboration::CollaborationDeliveryState::Read { .. }
    ));
}

#[tokio::test]
async fn deleted_source_is_not_recreated_and_delivery_failure_is_persisted() {
    let test = build_app();
    let source = caller(&test, "deleted-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;
    let receipt = test
        .state
        .collaboration
        .send(
            source.clone(),
            child.chat.clone(),
            "finish after deletion".into(),
            0,
        )
        .await
        .unwrap();
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    source_store
        .delete_chat_storage_by_chat_id(source.address.chat_id().as_str())
        .await
        .unwrap();

    release.notify_one();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let failed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    receipt.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if matches!(
                record.delivery,
                rua_session::collaboration::CollaborationDeliveryState::DeliveryFailed { .. }
            ) {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let rua_session::collaboration::CollaborationDeliveryState::DeliveryFailed { error, .. } =
        failed.delivery
    else {
        unreachable!()
    };
    assert!(error.contains("Source Chat is unavailable"));
    assert_eq!(failed.outcome.as_ref().unwrap().output, "review complete");
    assert!(source_store
        .load_chat_header(source.address.chat_id().as_str())
        .await
        .unwrap()
        .is_none());
    assert_eq!(requests.lock().await.len(), 1);
}

#[tokio::test]
async fn chat_stop_is_conditional_and_stale_retry_does_not_stop_next_turn() {
    use rua_server::services::collaboration::CollaborationStopStatus;
    let test = build_app();
    let source = caller(&test, "stop-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    assert!(child.active_turn.is_none());
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;
    let first = test
        .state
        .collaboration
        .send(source.clone(), child.chat.clone(), "stop this".into(), 0)
        .await
        .unwrap();
    let active = test
        .state
        .conversation_authority
        .active_turn(&child.chat)
        .await
        .unwrap();
    assert_eq!(active.turn_id, first.turn_id.as_deref().unwrap());
    let control = first
        .active_turn
        .as_ref()
        .expect("send must expose stop credentials");
    assert_eq!(control.turn_id, active.turn_id);
    assert_eq!(control.authority_epoch, active.authority_epoch);
    let read = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap();
    let read_control = read.active_turn.as_ref().expect("active stop credentials");
    assert_eq!(read_control.turn_id, control.turn_id);
    assert_eq!(read_control.authority_epoch, control.authority_epoch);
    let other_source = caller(&test, "unrelated-stop-source").await;
    assert!(test
        .state
        .collaboration
        .stop_turn(
            other_source,
            child.chat.clone(),
            control.turn_id.clone(),
            control.authority_epoch
        )
        .await
        .unwrap_err()
        .contains("another source Chat"));
    wait_for_source_requests(&requests, 1).await;
    let wrong_epoch = rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device("wrong-epoch-device", Some(control.authority_epoch));
    let mismatch = test
        .state
        .collaboration
        .stop_turn(
            source.clone(),
            child.chat.clone(),
            control.turn_id.clone(),
            wrong_epoch,
        )
        .await
        .unwrap();
    assert!(!mismatch.stopped);
    assert_eq!(mismatch.status, CollaborationStopStatus::TargetChanged);
    let still_active = test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap()
        .active_turn
        .expect("a rejected stop must leave the active Turn running");
    assert_eq!(still_active.turn_id, control.turn_id);
    assert_eq!(still_active.authority_epoch, control.authority_epoch);
    let mut followup_caller = source.clone();
    followup_caller.tool_call_id = "queued-before-stop".into();
    let queued = test
        .state
        .collaboration
        .send(
            followup_caller,
            child.chat.clone(),
            "report status only".into(),
            0,
        )
        .await
        .unwrap();
    assert_eq!(queued.state, "queued");
    let stopped = test
        .state
        .collaboration
        .stop_turn(
            source.clone(),
            child.chat.clone(),
            control.turn_id.clone(),
            control.authority_epoch,
        )
        .await
        .unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.status, CollaborationStopStatus::StopRequested);
    assert!(
        !WorkspaceSessionStore::new(
            ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
        )
        .load_collaboration_wake_state(child.chat.chat_id().as_str())
        .await
        .unwrap()
        .suppressed
    );
    release.notify_waiters();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let cancelled = target_store
        .load_collaboration_request(
            child.chat.chat_id().as_str(),
            queued.request_id.as_deref().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.outcome.unwrap().status, "cancelled");
    assert!(cancelled.turn_id.is_none());
    let first_record = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    first.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if record.outcome.is_some() {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(first_record.outcome.as_ref().unwrap().partial);
    assert_ne!(first_record.outcome.as_ref().unwrap().status, "completed");

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while test
            .state
            .conversation_authority
            .active_turn(&child.chat)
            .await
            .is_some()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let repeated = test
        .state
        .collaboration
        .stop_turn(
            source.clone(),
            child.chat.clone(),
            control.turn_id.clone(),
            control.authority_epoch,
        )
        .await
        .unwrap();
    assert!(!repeated.stopped);
    assert_eq!(repeated.status, CollaborationStopStatus::NoActiveTurn);
    assert!(test
        .state
        .collaboration
        .read(source.clone(), child.chat.clone(), 0)
        .await
        .unwrap()
        .active_turn
        .is_none());

    let mut next_source = source.clone();
    next_source.tool_call_id = "next-turn-call".into();
    let next = test
        .state
        .collaboration
        .send(next_source, child.chat.clone(), "next turn".into(), 0)
        .await
        .unwrap();
    let next_active = test
        .state
        .conversation_authority
        .active_turn(&child.chat)
        .await
        .unwrap();
    assert_eq!(next_active.turn_id, next.turn_id.as_deref().unwrap());
    assert_ne!(next_active.turn_id, active.turn_id);
    let stale = test
        .state
        .collaboration
        .stop_turn(source, child.chat, active.turn_id, active.authority_epoch)
        .await
        .unwrap();
    assert!(!stale.stopped);
    assert_eq!(stale.status, CollaborationStopStatus::TargetChanged);
    release.notify_waiters();
}

#[tokio::test]
async fn source_user_stop_preserves_late_result_wake_without_stopping_child() {
    let test = build_app();
    let source = caller(&test, "suppressed-source").await;
    let mut source_events = test
        .state
        .conversation_authority
        .subscribe_control_events(&source.address)
        .await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;
    let receipt = test
        .state
        .collaboration
        .send(
            source.clone(),
            child.chat.clone(),
            "finish while stopped".into(),
            0,
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while test
            .state
            .conversation_authority
            .active_turn(&child.chat)
            .await
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let running = test
        .state
        .conversation_authority
        .background_work_view(&source.address)
        .await;
    assert_eq!(running.items.len(), 1);
    assert_eq!(running.items[0].kind, "collaboration");
    assert_eq!(running.items[0].status, "running");
    assert_eq!(running.items[0].chat, child.chat);

    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    test.state.conversation_authority.start_active_turn(
        rua_server::services::conversation_authority::ConversationActiveTurnStart {
            address: source.address.clone(),
            driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
            authority_device_id: "fixture-device".into(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        },
    ).await;
    test.state
        .conversation_authority
        .request_stop(None, "console", &source.address)
        .await
        .unwrap();
    test.state
        .conversation_authority
        .finish_active_turn(&source.address)
        .await;
    assert!(test
        .state
        .conversation_authority
        .active_turn(&child.chat)
        .await
        .is_some());
    release.notify_one();
    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    receipt.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if matches!(
                record.delivery,
                rua_session::collaboration::CollaborationDeliveryState::Delivered { .. }
            ) {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        completed.delivery,
        rua_session::collaboration::CollaborationDeliveryState::Delivered { .. }
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    assert_eq!(requests.lock().await.len(), 2);
    assert!(source_store
        .read_message_range(source.address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .any(|note| matches!(
            note.payload,
            rua_session::RuntimeNotePayload::CollaborationResult(_)
        )));
    assert!(
        !source_store
            .load_collaboration_wake_state(source.address.chat_id().as_str())
            .await
            .unwrap()
            .suppressed
    );
    assert!(test
        .state
        .conversation_authority
        .active_turn(&child.chat)
        .await
        .is_none());
    let idle = test
        .state
        .conversation_authority
        .background_work_view(&source.address)
        .await;
    assert!(
        idle.items.is_empty(),
        "Deferred result must not make an idle child active"
    );
    let mut observed_idle = false;
    while let Ok(event) = source_events.try_recv() {
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        if event["reason"] == "background_work_changed"
            && event["control"]["background_work"]["items"] == serde_json::json!([])
        {
            observed_idle = true;
        }
    }
    assert!(
        observed_idle,
        "Child Turn finish must refresh source background activity"
    );
}

#[tokio::test]
async fn busy_collaboration_sends_interject_and_retry_is_idempotent() {
    let test = build_app();
    let source = caller(&test, "interjection-source").await;
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: Some(release.clone()),
        }))
        .await;
    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    source_store
        .set_collaboration_wake_suppressed(source.address.chat_id().as_str(), true)
        .await
        .unwrap();

    let first = test
        .state
        .collaboration
        .send(
            source.clone(),
            child.chat.clone(),
            "first request".into(),
            0,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let mut second_caller = source.clone();
    second_caller.tool_call_id = "interjection-second-call".into();
    let second = test
        .state
        .collaboration
        .send(
            second_caller,
            child.chat.clone(),
            "replacement request".into(),
            0,
        )
        .await
        .unwrap();
    assert_ne!(first.request_id, second.request_id);
    assert_eq!(
        *requests.lock().await[0]
            .run_control
            .as_ref()
            .expect("running collaboration must receive interjection control")
            .borrow(),
        rua_core::RunControl::InterjectAfterTool,
        "follow-up must interrupt model generation, not merely wait in the queue"
    );
    assert_eq!(second.state, "queued");
    assert!(second.turn_id.is_none());
    assert_eq!(
        second
            .active_turn
            .as_ref()
            .map(|turn| (&turn.turn_id, &turn.authority_epoch)),
        first
            .active_turn
            .as_ref()
            .map(|turn| (&turn.turn_id, &turn.authority_epoch))
    );
    let mut third_caller = source.clone();
    third_caller.tool_call_id = "interjection-third-call".into();
    let (third, retry) = tokio::join!(
        test.state.collaboration.send(
            third_caller.clone(),
            child.chat.clone(),
            "third request".into(),
            0
        ),
        test.state
            .collaboration
            .send(third_caller, child.chat.clone(), "third request".into(), 0)
    );
    let third = third.unwrap();
    let retry = retry.unwrap();
    assert_eq!(third.state, "queued");
    assert_ne!(third.request_id, second.request_id);
    assert_eq!(retry.request_id, third.request_id);
    assert_eq!(retry.input_id, third.input_id);
    let persisted = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    )
    .list_collaboration_requests(child.chat.chat_id().as_str())
    .await
    .unwrap();
    assert_eq!(persisted.len(), 3);
    let queued = persisted
        .iter()
        .find(|record| Some(&record.request_id) == third.request_id.as_ref())
        .unwrap();
    assert_eq!(queued.input_id, third.input_id);
    assert!(queued.turn_id.is_none());
    assert!(queued.outcome.is_none());
    assert_eq!(requests.lock().await.len(), 1);
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    release.notify_waiters();

    let target_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone()).root(),
    );
    let (old, new) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let old = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    first.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            let new = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    second.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if old.outcome.is_some() && new.outcome.is_some() {
                break (old, new);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let old_outcome = old.outcome.unwrap();
    let new_outcome = new.outcome.unwrap();
    assert_eq!(old_outcome.status, "interjected");
    assert!(old_outcome.partial);
    assert_eq!(new_outcome.status, "completed");
    assert!(!new_outcome.partial);
    assert_ne!(old.turn_id, new.turn_id);
    assert_ne!(old_outcome.result_id, new_outcome.result_id);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().await.len() < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        loop {
            release.notify_one();
            let record = target_store
                .load_collaboration_request(
                    child.chat.chat_id().as_str(),
                    third.request_id.as_deref().unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            if let Some(outcome) = record.outcome {
                assert_eq!(outcome.status, "completed");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(requests.lock().await.len(), 3);
}

#[tokio::test]
async fn explicit_resume_batches_all_deferred_results_into_one_source_turn() {
    let test = build_app();
    let source = caller(&test, "resume-source").await;
    let first = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    let mut second_caller = source.clone();
    second_caller.tool_call_id = "resume-source-second".into();
    let second = test
        .state
        .collaboration
        .create(second_caller, "default".into(), None, 0)
        .await
        .unwrap();
    assert_ne!(first.chat, second.chat);

    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;

    let mut persisted = Vec::new();
    for (index, target) in [first.chat, second.chat].into_iter().enumerate() {
        let request_id = uuid::Uuid::new_v4().to_string();
        let record = rua_session::collaboration::CollaborationRequest {
            request_id: request_id.clone(),
            source: source.address.clone(),
            target: target.clone(),
            origin_turn_id: source.turn_id.clone(),
            origin_tool_call_id: format!("call-{index}"),
            input_digest: format!("digest-{index}"),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: Some(format!("input-{index}")),
            turn_id: Some(format!("child-turn-{index}")),
            outcome: Some(rua_session::collaboration::CollaborationOutcome {
                result_id: format!("result-{index}"),
                status: "completed".into(),
                output: format!("output-{index}{}", "中文结果".repeat(5000)),
                partial: false,
                extra: Default::default(),
            }),
            delivery: rua_session::collaboration::CollaborationDeliveryState::Deferred,
            extra: Default::default(),
        };
        let store = WorkspaceSessionStore::new(
            ChatScopeStore::resolve(&test.state.rua_root, target.clone()).root(),
        );
        store.save_collaboration_request(&record).await.unwrap();
        persisted.push((store, target, request_id));
    }

    test.state
        .collaboration
        .resume_deferred_results(source.address.clone())
        .await
        .unwrap();
    wait_for_source_requests(&requests, 1).await;
    assert_eq!(requests.lock().await.len(), 1);

    let mut observation_ids = Vec::new();
    for (store, target, request_id) in persisted {
        let record = store
            .load_collaboration_request(target.chat_id().as_str(), &request_id)
            .await
            .unwrap()
            .unwrap();
        let rua_session::collaboration::CollaborationDeliveryState::Delivered {
            observation_id,
            ..
        } = record.delivery
        else {
            panic!("deferred collaboration result was not delivered");
        };
        observation_ids.push(observation_id);
    }
    assert_eq!(observation_ids[0], observation_ids[1]);

    let source_store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, source.address.clone()).root(),
    );
    let notes = source_store
        .read_message_range(source.address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .filter(|note| {
            matches!(
                note.payload,
                rua_session::RuntimeNotePayload::CollaborationResult(_)
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 1);
    let rua_session::RuntimeNotePayload::CollaborationResult(result) = &notes[0].payload else {
        unreachable!();
    };
    assert_eq!(result.request_id, "batch:2");
    assert_eq!(result.results.len(), 2);
    let toolkit = test
        .state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .task_toolkit()
        .await
        .unwrap();
    let read_tool = toolkit.get_executor("chat_read").unwrap();
    let mut context = rua_core::tool::ToolContext {
        agent_id: "default".into(),
        session_id: source.address.chat_id().to_string(),
        working_dir: source.working_dir.clone(),
        collaboration_runtime: Some(test.state.collaboration_runtime.clone()),
        ..Default::default()
    };
    context
        .metadata
        .insert("security_chat_address".into(), json!(source.address));
    context
        .metadata
        .insert("turn_id".into(), json!(source.turn_id));
    context.metadata.insert(
        "chat_configuration_epoch".into(),
        json!(source.configuration_epoch),
    );
    for reference in &result.results {
        let mut cursor = json!(reference.result);
        let mut text = String::new();
        let output_dir = tempfile::tempdir().unwrap();
        let processor = rua_tool::result_store::ToolResultOutputProcessor::new_with_limits(
            output_dir.path(),
            10_000,
            1024 * 1024,
            4.0,
        );
        loop {
            let mut reply = read_tool
                .execute(
                    &agent_types::ToolCall::new(
                        "result-page",
                        "chat_read",
                        json!({"chat":reference.chat, "result":cursor}),
                    ),
                    &context,
                )
                .await
                .unwrap();
            assert!(!reply.is_error, "{}", reply.text_content());
            let original = reply.text_content();
            assert!(
                !processor
                    .process(
                        "result-page",
                        "chat_read",
                        &json!({"chat":reference.chat,"result":cursor}),
                        &mut reply
                    )
                    .await
            );
            assert_eq!(
                reply.text_content(),
                original,
                "default model budget must preserve page control fields"
            );
            if cursor["offset"] == 0 {
                let mut limited = reply.clone();
                let small = rua_tool::result_store::ToolResultOutputProcessor::new_with_limits(
                    output_dir.path(),
                    512,
                    1024 * 1024,
                    4.0,
                );
                assert!(
                    small
                        .process(
                            "small-result-page",
                            "chat_read",
                            &json!({"chat":reference.chat,"result":cursor}),
                            &mut limited
                        )
                        .await
                );
                assert!(
                    limited.metadata["tool_output"]["visible_tokens"]
                        .as_u64()
                        .unwrap()
                        <= 512
                );
                let recall_key = limited
                    .truncation
                    .as_ref()
                    .and_then(|meta| meta.recall_key.as_ref())
                    .expect("truncated result must have recall evidence");
                let recalled = rua_contract::Tool::call(
                    &rua_tool::extended::recall::RecallToolResultTool,
                    json!({"recall_key":recall_key,"max_tokens":10_000}),
                    rua_core::tool::ToolContext {
                        tool_result_store_dir: Some(output_dir.path().into()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                assert!(!recalled.is_error);
                assert!(
                    recalled.text_content().contains(&original),
                    "recall must preserve the result page and next cursor"
                );
            }
            assert!(
                reply.metadata["tool_output"]["visible_tokens"]
                    .as_u64()
                    .unwrap()
                    <= 10_000
            );
            let reply: serde_json::Value = serde_json::from_str(&reply.text_content()).unwrap();
            let page = &reply["outcome"];
            let chunk = page["output"].as_str().unwrap();
            assert!(chunk.len() <= 8000);
            text.push_str(chunk);
            if page["has_more"] == false {
                break;
            }
            cursor = page["next"].clone();
        }
        assert_eq!(
            text,
            format!(
                "output-{}{}",
                reference.result.result_id.strip_prefix("result-").unwrap(),
                "中文结果".repeat(5000)
            )
        );
        let mut invalid = reference.result.clone();
        invalid.result_id = "wrong-result".into();
        assert!(test
            .state
            .collaboration
            .read_result(source.clone(), reference.chat.clone(), invalid)
            .await
            .is_err());
        let other = caller(&test, &format!("other-{}", reference.result.result_id)).await;
        assert!(test
            .state
            .collaboration
            .read_result(other, reference.chat.clone(), reference.result.clone())
            .await
            .unwrap_err()
            .contains("another source"));
    }
    assert_eq!(requests.lock().await.len(), 1);
}

#[tokio::test]
async fn create_preserves_chat_reference_when_first_submission_fails() {
    let test = build_app();
    let source = caller(&test, "failed-first-submission").await;
    let reply = test
        .state
        .collaboration
        .create(source, "default".into(), Some("   ".into()), 0)
        .await
        .unwrap();

    assert_eq!(reply.state, "submission_failed");
    assert!(reply
        .error
        .as_deref()
        .is_some_and(|error| error.contains("must not be empty")));
    assert!(reply.request_id.is_none());
    let store = WorkspaceSessionStore::new(
        ChatScopeStore::resolve(&test.state.rua_root, reply.chat.clone()).root(),
    );
    assert!(store
        .load_chat_header(reply.chat.chat_id().as_str())
        .await
        .unwrap()
        .is_some());
    assert!(store
        .read_turn_range(reply.chat.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn project_collaboration_keeps_resource_root_and_rejects_archived_project() {
    let test = build_app();
    let template = caller(&test, "project-template").await;
    let original_scope = ChatScopeStore::resolve(&test.state.rua_root, template.address.clone());
    let mut header = WorkspaceSessionStore::new(original_scope.root())
        .load_chat_header(template.address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let device = header.extra["authority_placement"]["device_id"]
        .as_str()
        .unwrap()
        .to_string();
    let resource = tempfile::tempdir().unwrap();
    let project = test
        .state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: device,
            resource_root: resource.path().to_string_lossy().into_owned(),
            display_name: Some("Collaboration fixture".into()),
        })
        .unwrap()
        .project;
    let binding = test
        .state
        .project_bindings
        .get(&project.project_id)
        .unwrap()
        .unwrap();
    let address =
        ChatAddress::project(project.project_id.clone(), uuid::Uuid::new_v4().to_string());
    header.chat_id = address.chat_id().to_string();
    header.metadata.chat_id = header.chat_id.clone();
    header
        .extra
        .insert("project_id".into(), json!(project.project_id));
    header
        .extra
        .insert("selected_agent_id".into(), json!("default"));
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    WorkspaceSessionStore::new(scope.root())
        .save_chat_header(&header)
        .await
        .unwrap();
    let source = CollaborationCaller {
        address,
        working_dir: binding.primary_root().to_path_buf().clone(),
        ..template
    };
    let child = test
        .state
        .collaboration
        .create(source.clone(), "default".into(), None, 0)
        .await
        .unwrap();
    assert_eq!(child.chat.project_id(), Some(&project.project_id));
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests: requests.clone(),
            release: None,
        }))
        .await;
    let mut invocation = source.clone();
    invocation.tool_call_id = "project-send".into();
    let result = test
        .state
        .collaboration
        .send(invocation, child.chat.clone(), "inspect project".into(), 5)
        .await
        .unwrap();
    assert_eq!(result.state, "completed");
    assert_eq!(
        requests.lock().await[0].working_dir.as_deref(),
        Some(binding.primary_root())
    );
    let child_scope = ChatScopeStore::resolve(&test.state.rua_root, child.chat.clone());
    let child_store = WorkspaceSessionStore::new(child_scope.root());
    assert_eq!(
        child_store
            .load_chat_header(child.chat.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .metadata
            .title,
        "inspect project"
    );
    test.state
        .project_catalog
        .set_lifecycle(&project.project_id, rua_project::ProjectLifecycle::Archived)
        .unwrap();
    let mut next = source;
    next.tool_call_id = "archived-send".into();
    assert!(test
        .state
        .collaboration
        .send(next, child.chat, "must not run".into(), 0)
        .await
        .unwrap_err()
        .contains("not active"));
    assert_eq!(requests.lock().await.len(), 1);
}

#[tokio::test]
async fn in_flight_limit_rejects_new_work_but_allows_duplicate_request() {
    let test = build_app();
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id":"reviewer","name":"Reviewer"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let source = caller(&test, "capacity-source").await;
    let service = rua_server::services::collaboration::CollaborationService::new(
        test.state.rua_root.clone(),
        Arc::downgrade(&test.state.registry),
        test.state.agent_service.clone(),
        test.state.product_hooks.clone(),
        test.state.conversation_input_intake.clone(),
        test.state
            .product_hooks
            .chat_configuration(&source.address)
            .await
            .unwrap()
            .authority_device_id
            .unwrap(),
        1,
    );
    let created = service
        .create(source.clone(), "reviewer".into(), None, 0)
        .await
        .unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    test.state
        .registry
        .get_or_create("reviewer")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(ControlledRuntime {
            requests,
            release: Some(release.clone()),
        }))
        .await;
    let accepted = service
        .send(source.clone(), created.chat.clone(), "hold".into(), 0)
        .await
        .unwrap();
    let duplicate = service
        .send(source.clone(), created.chat.clone(), "hold".into(), 0)
        .await
        .unwrap();
    assert_eq!(accepted.request_id, duplicate.request_id);
    let mut second = source.clone();
    second.tool_call_id = "second".into();
    assert!(service
        .send(second.clone(), created.chat.clone(), "other".into(), 0)
        .await
        .unwrap_err()
        .contains("limit reached"));
    assert!(service
        .create(second, "reviewer".into(), Some("other".into()), 0)
        .await
        .unwrap_err()
        .contains("limit reached"));
    release.notify_one();
    assert!(service
        .send(source.clone(), created.chat.clone(), "hold".into(), 5)
        .await
        .unwrap()
        .outcome
        .is_none());
    let mut after_terminal = source;
    after_terminal.tool_call_id = "after-terminal".into();
    assert!(service
        .send(after_terminal, created.chat, "new work".into(), 0)
        .await
        .is_ok());
}
