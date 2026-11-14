use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{stream, StreamExt};

static SKILL_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, ToolCall,
};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo, Provider, ProviderInfo};
use llm_multiplex::LlmError;
use mcp_manager::error::McpResult;
use mcp_manager::manager::McpManager;
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpConfig, McpContent, McpReloadReport, McpServerConfig,
    McpServerId, McpShutdownReport, McpToolInfo, ToolFilter,
};
use rua_agent::hooks::PROJECT_SKILLS_ROOT_METADATA_KEY;
use rua_agent::prompt::{AGENT_CUSTOM_PROMPT_METADATA_KEY, AGENT_PROMPT_FILES_METADATA_KEY};
use rua_agent::runtime::ReactAgentRuntime;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::message::ContentPart;
use rua_provider::manager::ProviderManager;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, RuntimeNote,
    RuntimeNotePayload, TurnLifecycle, WorkspaceSessionStore,
};

struct CapturingModel {
    captured: Arc<Mutex<Vec<ChatRequest>>>,
    tool_on_first: Option<ToolCall>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for CapturingModel {
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: None,
            model: Some("mock".to_string()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, req: ChatRequest) -> EventStream {
        let is_first = {
            let mut lock = self.captured.lock().unwrap();
            let first = lock.is_empty();
            lock.push(req);
            first
        };
        if is_first {
            if let Some(ref tc) = self.tool_on_first {
                let tc = tc.clone();
                return Box::pin(stream::iter(vec![
                    Ok(AgentEvent::Start {
                        run_id: "r".to_string(),
                        model: Some("mock".to_string()),
                        metadata: serde_json::Value::Null,
                    }),
                    Ok(AgentEvent::ToolCall { call: tc }),
                    Ok(AgentEvent::End {
                        finish_reason: FinishReason::ToolUse,
                        usage: None,
                        metadata: serde_json::Value::Null,
                    }),
                ]));
            }
        }
        Box::pin(stream::iter(vec![
            Ok(AgentEvent::Start {
                run_id: "r".to_string(),
                model: Some("mock".to_string()),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::TextDelta {
                delta: "ok".to_string(),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            }),
        ]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct MockProvider {
    info: ProviderInfo,
    captured: Arc<Mutex<Vec<ChatRequest>>>,
    tool_on_first: Option<ToolCall>,
    model_info: ModelInfo,
}

struct AvailabilityFlipModel {
    captured: Arc<Mutex<Vec<ChatRequest>>>,
    manager: Arc<MutableMcpManager>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ChatModel for AvailabilityFlipModel {
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("availability flip model uses streaming only")
    }

    fn chat_stream(&self, req: ChatRequest) -> EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.captured.lock().unwrap().push(req);
        match call {
            0 => {
                self.manager.set_available(false);
                Box::pin(stream::iter(vec![
                    Ok(AgentEvent::ToolCall {
                        call: ToolCall::new(
                            "continue-after-failure",
                            "mcp__desktop__echo",
                            serde_json::json!({"text": "captured executor"}),
                        ),
                    }),
                    Ok(AgentEvent::End {
                        finish_reason: FinishReason::ToolUse,
                        usage: None,
                        metadata: serde_json::Value::Null,
                    }),
                ]))
            }
            1 => {
                self.manager.set_available(true);
                Box::pin(stream::iter(vec![
                    Ok(AgentEvent::ToolCall {
                        call: ToolCall::new(
                            "continue-after-recovery",
                            "Unavailable_Test_Continue",
                            serde_json::json!({}),
                        ),
                    }),
                    Ok(AgentEvent::End {
                        finish_reason: FinishReason::ToolUse,
                        usage: None,
                        metadata: serde_json::Value::Null,
                    }),
                ]))
            }
            _ => Box::pin(stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "done".to_string(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                }),
            ])),
        }
    }

    fn model_info(&self) -> &ModelInfo {
        Box::leak(Box::new(ModelInfo {
            id: "availability-flip".to_string(),
            name: "Availability Flip".to_string(),
            context_window: Some(4096),
            max_output_tokens: Some(1024),
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: Default::default(),
        }))
    }
}

struct AvailabilityFlipProvider {
    info: ProviderInfo,
    captured: Arc<Mutex<Vec<ChatRequest>>>,
    manager: Arc<MutableMcpManager>,
    calls: Arc<AtomicUsize>,
}

struct MutableMcpManager {
    available: AtomicBool,
    tool_calls: AtomicUsize,
    discovery_calls: AtomicUsize,
}

#[tokio::test]
async fn configuration_selection_is_offline_and_recovers_without_updating_the_chat() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = Arc::new(MutableMcpManager::new(false));
    let (runtime, captured) = make_runtime(None).await;
    runtime.set_mcp_manager(manager.clone());
    let resolver = runtime.capability_resolver();
    let configuration = resolver
        .configuration(workspace.path(), None, "console")
        .await
        .unwrap();
    assert!(configuration.mcp_servers.contains_key("desktop"));
    assert_eq!(manager.discovery_calls.load(Ordering::SeqCst), 0);
    let request = || {
        let mut request = base_request(workspace.path().into(), "console");
        request.metadata.insert(
            rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
            serde_json::to_value(&configuration).unwrap(),
        );
        request
    };
    for available in [false, true, false, true] {
        manager.set_available(available);
        let events = runtime
            .stream(request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let captured = captured.lock().unwrap();
        assert_eq!(
            tool_names(captured.last().unwrap()).contains(&"mcp__desktop__echo".into()),
            available
        );
    }
    manager.set_available(true);
    assert_eq!(
        resolver
            .configuration(workspace.path(), None, "console")
            .await
            .unwrap(),
        configuration
    );
    let mut invalid = request();
    invalid.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
        serde_json::json!({"skills":[]}),
    );
    let error = match runtime.stream(invalid).await {
        Ok(_) => panic!("policy is required"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("selection_policy"), "{error}");
}

impl MutableMcpManager {
    fn new(available: bool) -> Self {
        Self {
            available: AtomicBool::new(available),
            tool_calls: AtomicUsize::new(0),
            discovery_calls: AtomicUsize::new(0),
        }
    }

    fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    fn tool_call_count(&self) -> usize {
        self.tool_calls.load(Ordering::SeqCst)
    }

    fn status(&self) -> McpServerStatus {
        let available = self.available.load(Ordering::SeqCst);
        McpServerStatus {
            key: "desktop".to_string(),
            name: "Desktop MCP".to_string(),
            enabled: true,
            state: if available {
                McpConnectionState::Ready
            } else {
                McpConnectionState::Failed
            },
            transport_type: "stdio".to_string(),
            tools_count: usize::from(available),
            last_error: (!available).then(|| "desktop command is unavailable".to_string()),
            connected_at: None,
            mobile_unavailable_reason: None,
            config_errors: Vec::new(),
        }
    }
}

#[async_trait]
impl McpManager for MutableMcpManager {
    async fn suspend_new_calls(&self) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }
    async fn configuration(&self) -> mcp_manager::error::McpResult<McpConfig> {
        Ok(
            serde_json::from_value(serde_json::json!({"clients":{"desktop":{
                "name":"Desktop MCP","enabled":true,"transport":"stdio","command":"fixture"
            }}}))
            .unwrap(),
        )
    }

    async fn load_config(&self, _config: McpConfig) -> McpResult<McpReloadReport> {
        Ok(McpReloadReport::default())
    }

    async fn list_servers(&self) -> McpResult<Vec<McpServerStatus>> {
        Ok(vec![self.status()])
    }

    async fn list_tools(&self, _filter: ToolFilter) -> McpResult<Vec<McpToolInfo>> {
        self.discovery_calls.fetch_add(1, Ordering::SeqCst);
        if !self.available.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        Ok(vec![McpToolInfo {
            server_id: McpServerId::new("desktop"),
            name: "echo".to_string(),
            description: "Echo from the desktop MCP".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                }
            }),
            raw: serde_json::Value::Null,
        }])
    }

    async fn call_tool(
        &self,
        _server: &McpServerId,
        _tool: &str,
        _args: serde_json::Value,
        _ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        Ok(McpCallResult {
            structured_content: None,
            content: vec![McpContent::Text {
                text: "captured executor completed".to_string(),
            }],
            is_error: false,
        })
    }

    async fn reload(&self, _new_config: McpConfig) -> McpResult<McpReloadReport> {
        Ok(McpReloadReport::default())
    }

    async fn invalidate_tool_cache(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn connect_server(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn disconnect_server(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn replace_client(
        &self,
        _server_id: &McpServerId,
        _config: McpServerConfig,
    ) -> McpResult<()> {
        Ok(())
    }

    async fn shutdown_all(&self) -> McpResult<McpShutdownReport> {
        Ok(McpShutdownReport::default())
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        !model.trim().is_empty()
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(CapturingModel {
            captured: self.captured.clone(),
            tool_on_first: self.tool_on_first.clone(),
            info: self.model_info.clone(),
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }

    async fn check_connection(&self) -> Result<llm_multiplex::traits::ConnectionStatus, LlmError> {
        Ok(llm_multiplex::traits::ConnectionStatus::Connected)
    }
}

#[async_trait]
impl Provider for AvailabilityFlipProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        !model.trim().is_empty()
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(AvailabilityFlipModel {
            captured: Arc::clone(&self.captured),
            manager: Arc::clone(&self.manager),
            calls: Arc::clone(&self.calls),
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(Vec::new())
    }

    async fn check_connection(&self) -> Result<llm_multiplex::traits::ConnectionStatus, LlmError> {
        Ok(llm_multiplex::traits::ConnectionStatus::Connected)
    }
}

async fn make_runtime(
    tool_on_first: Option<ToolCall>,
) -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    make_runtime_with_model_info(tool_on_first, mock_model_info()).await
}

fn mock_model_info() -> ModelInfo {
    ModelInfo {
        id: "mock".to_string(),
        name: "Mock".to_string(),
        context_window: Some(4096),
        max_output_tokens: Some(1024),
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: Default::default(),
    }
}

async fn make_runtime_with_model_info(
    tool_on_first: Option<ToolCall>,
    model_info: ModelInfo,
) -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    let captured: Arc<Mutex<Vec<ChatRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let pm = ProviderManager::new();
    for provider_id in [
        "mock-provider",
        "dashscope",
        "openai",
        "openai-response",
        "openrouter",
        "google",
        "anthropic",
        "ollama",
    ] {
        pm.register(Arc::new(MockProvider {
            info: ProviderInfo {
                id: provider_id.to_string(),
                name: provider_id.to_string(),
                provider_type: Some("mock".to_string()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
            captured: captured.clone(),
            tool_on_first: tool_on_first.clone(),
            model_info: model_info.clone(),
        }));
    }
    pm.store_default_model(rua_provider::config_loader::DefaultModel {
        provider_id: "mock-provider".to_string(),
        model: "mock".to_string(),
        source: None,
        extra: Default::default(),
    })
    .await;
    let runtime = ReactAgentRuntime::new(Arc::new(pm));
    (runtime, captured)
}

async fn make_runtime_without_default_model() -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    let captured: Arc<Mutex<Vec<ChatRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let pm = ProviderManager::new();
    pm.register(Arc::new(MockProvider {
        info: ProviderInfo {
            id: "slot-provider".to_string(),
            name: "Slot Provider".to_string(),
            provider_type: Some("mock".to_string()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        captured: captured.clone(),
        tool_on_first: None,
        model_info: mock_model_info(),
    }));
    let runtime = ReactAgentRuntime::new(Arc::new(pm));
    (runtime, captured)
}

async fn make_availability_flip_runtime(
    manager: Arc<MutableMcpManager>,
) -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let pm = ProviderManager::new();
    pm.register(Arc::new(AvailabilityFlipProvider {
        info: ProviderInfo {
            id: "availability-flip".to_string(),
            name: "Availability Flip".to_string(),
            provider_type: Some("mock".to_string()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        captured: Arc::clone(&captured),
        manager: Arc::clone(&manager),
        calls,
    }));
    pm.store_default_model(rua_provider::config_loader::DefaultModel {
        provider_id: "availability-flip".to_string(),
        model: "availability-flip".to_string(),
        source: None,
        extra: Default::default(),
    })
    .await;
    (
        ReactAgentRuntime::new(Arc::new(pm)).with_mcp_manager(manager),
        captured,
    )
}

fn write_skill_md(dir: &std::path::Path, name: &str, description: &str, body: &str) {
    fs::create_dir_all(dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}");
    fs::write(dir.join("SKILL.md"), content).unwrap();
}

fn write_invalid_skill_md(dir: &std::path::Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("SKILL.md"), "---\nname: [unclosed\n---\n\nbody\n").unwrap();
}

fn write_skill_json(workspace_dir: &std::path::Path, entries: &[(&str, bool, &[&str])]) {
    let mut skills = serde_json::Map::new();
    for (name, enabled, channels) in entries {
        let channels_json: serde_json::Value =
            serde_json::json!(channels.iter().map(|c| c.to_string()).collect::<Vec<_>>());
        skills.insert(
            name.to_string(),
            serde_json::json!({
                "enabled": enabled,
                "channels": channels_json,
                "source": "customized",
                "metadata": { "name": name, "source": "customized" }
            }),
        );
    }
    let manifest = serde_json::json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 0,
        "skills": skills
    });
    fs::write(
        workspace_dir.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn base_request(workspace_dir: std::path::PathBuf, channel: &str) -> AgentRequest {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("channel".to_string(), serde_json::json!(channel));
    metadata.insert(
        AGENT_CUSTOM_PROMPT_METADATA_KEY.to_string(),
        serde_json::json!("You are helpful."),
    );
    AgentRequest {
        session_id: "sess".into(),
        user_id: "user".to_string(),
        agent_id: "agent".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata,
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace_dir),
        conversation_store_dir: None,
        working_dir: None,
    }
}

fn project_request(
    workspace_dir: std::path::PathBuf,
    project_root: std::path::PathBuf,
) -> AgentRequest {
    let mut request = base_request(workspace_dir, "console");
    request.working_dir = Some(project_root.clone());
    request.metadata.insert(
        PROJECT_SKILLS_ROOT_METADATA_KEY.to_string(),
        serde_json::Value::String(project_root.to_string_lossy().into_owned()),
    );
    request
}

fn tool_names(req: &ChatRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

fn system_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .filter(|m| matches!(m.role, Role::System | Role::Developer))
        .map(|m| m.text_content())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_message_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .flat_map(|m| m.content.iter())
        .filter_map(|p| match p {
            ContentPart::ToolResult { result } => Some(result.text_content()),
            ContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_runtime_availability_invalidates_the_next_capability_snapshot() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let session_store_dir = tmp.path().join("conversation-store");
    let store = WorkspaceSessionStore::new(&session_store_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "sess".to_string(),
            agent_id: "agent".to_string(),
            title: "Capability Snapshot".to_string(),
            created_at: "2026-08-04T00:00:00Z".to_string(),
            last_activity_at: "2026-08-04T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "sess",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-04T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-08-04T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "inspect capabilities"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    let manager = Arc::new(MutableMcpManager::new(true));
    let (runtime, captured) = make_runtime(None).await;
    let runtime = runtime.with_mcp_manager(manager.clone());
    let request = || {
        let mut request = base_request(tmp.path().to_path_buf(), "console");
        request.conversation_store_dir = Some(session_store_dir.clone());
        request
            .metadata
            .insert("turn_id".to_string(), serde_json::json!("turn-1"));
        request
    };

    let first = runtime.stream(request()).await.unwrap();
    let _: Vec<_> = first.collect().await;

    manager.set_available(false);
    let second = runtime.stream(request()).await.unwrap();
    let _: Vec<_> = second.collect().await;

    manager.set_available(true);
    let third = runtime.stream(request()).await.unwrap();
    let _: Vec<_> = third.collect().await;
    let fourth = runtime.stream(request()).await.unwrap();
    let _: Vec<_> = fourth.collect().await;

    {
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(tool_names(&requests[0])
            .iter()
            .any(|name| name == "mcp__desktop__echo"));
        assert!(!tool_names(&requests[1])
            .iter()
            .any(|name| name == "mcp__desktop__echo"));
        assert!(tool_names(&requests[2])
            .iter()
            .any(|name| name == "mcp__desktop__echo"));
        assert!(tool_names(&requests[3])
            .iter()
            .any(|name| name == "mcp__desktop__echo"));

        let snapshots = requests
            .iter()
            .map(|request| {
                request
                    .metadata
                    .get("effective_capability_snapshot")
                    .expect("request should carry capability snapshot diagnostics")
            })
            .collect::<Vec<_>>();
        let generations = snapshots
            .iter()
            .map(|snapshot| {
                snapshot
                    .get("generation")
                    .and_then(serde_json::Value::as_u64)
                    .expect("snapshot should carry a generation")
            })
            .collect::<Vec<_>>();
        assert!(generations[1] > generations[0]);
        assert!(generations[2] > generations[1]);
        assert_eq!(generations[3], generations[2]);
        assert!(snapshots[1]
            .get("diagnostics")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|diagnostics| diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .get("capability_type")
                    .and_then(serde_json::Value::as_str)
                    == Some("mcp")
                    && diagnostic.get("name").and_then(serde_json::Value::as_str) == Some("desktop")
                    && diagnostic.get("state").and_then(serde_json::Value::as_str)
                        == Some("unavailable")
            })));
    }

    let flushed = store
        .flush_pending_runtime_notes("sess", "turn-1")
        .await
        .unwrap();
    assert_eq!(flushed.len(), 2);
    let capability_notes = store
        .read_message_range("sess", 1, 20)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|entry| RuntimeNote::from_message_log_entry(&entry))
        .filter(|note| matches!(note.payload, RuntimeNotePayload::CapabilityState(_)))
        .collect::<Vec<_>>();
    assert_eq!(capability_notes.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_runtime_availability_refreshes_between_model_calls_in_one_run() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let manager = Arc::new(MutableMcpManager::new(true));
    let (runtime, captured) = make_availability_flip_runtime(Arc::clone(&manager)).await;

    let mut request = base_request(tmp.path().to_path_buf(), "console");
    request.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
        serde_json::to_value(
            runtime
                .capability_resolver()
                .configuration(tmp.path(), None, "console")
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    let stream = runtime.stream(request).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(tool_names(&requests[0])
        .iter()
        .any(|name| name == "mcp__desktop__echo"));
    assert!(!tool_names(&requests[1])
        .iter()
        .any(|name| name == "mcp__desktop__echo"));
    assert!(tool_names(&requests[2])
        .iter()
        .any(|name| name == "mcp__desktop__echo"));
    assert_eq!(manager.tool_call_count(), 1);
    assert!(tool_message_text(&requests[1]).contains("captured executor completed"));

    let generations = requests
        .iter()
        .map(|request| {
            request
                .metadata
                .get("effective_capability_snapshot")
                .and_then(|snapshot| snapshot.get("generation"))
                .and_then(serde_json::Value::as_u64)
                .expect("each model call should carry one capability generation")
        })
        .collect::<Vec<_>>();
    assert!(generations[1] > generations[0]);
    assert!(generations[2] > generations[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn i0_runtime_environment_metadata_is_not_injected_into_system_prompt() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata
        .insert("host_profile".to_string(), serde_json::json!("mobile-apk"));
    req.metadata.insert(
        "device_id".to_string(),
        serde_json::json!("0102030405060708"),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    let sys = system_text(&reqs[0]);
    assert!(!sys.contains("mobile-apk"), "sys: {sys}");
    assert!(!sys.contains("0102030405060708"), "sys: {sys}");
}

#[tokio::test(flavor = "current_thread")]
async fn text_only_runtime_strips_images_and_explains_the_limitation() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut info = mock_model_info();
    info.supports_multimodal = Some(false);
    info.supports_image = Some(false);
    info.supports_video = Some(false);
    let (runtime, captured) = make_runtime_with_model_info(None, info).await;
    let mut req = base_request(tmp.path().to_path_buf(), "console");
    req.messages[0].content.push(ContentPart::Image {
        url: "/tmp/photo.png".to_string(),
        media_type: Some("image/png".to_string()),
        alt: Some("photo.png".to_string()),
    });

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    assert!(system_text(&reqs[0]).contains("only understand text content"));
    assert!(reqs[0]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .all(|part| !matches!(part, ContentPart::Image { .. })));
    assert!(reqs[0]
        .messages
        .iter()
        .any(|message| message.text_content().contains("Image was here")));
}

#[tokio::test(flavor = "current_thread")]
async fn chat_model_slot_reasoning_metadata_becomes_canonical_reasoning() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "dashscope",
            "model": "qwen3.7-plus",
            "thinking_strength": "high",
            "thinking_param_style": "budget",
            "thinking_budget_range": [1, 81920]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    assert_eq!(
        reqs[0].reasoning,
        Some(agent_types::ReasoningConfig {
            mode: agent_types::ReasoningMode::Budget,
            enabled: true,
            effort: None,
            budget_tokens: Some(32768),
            summary: Some(agent_types::ReasoningSummary::Auto),
        })
    );
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn chat_model_slot_without_thinking_strength_does_not_send_thinking_options() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "dashscope",
            "model": "qwen3.7-plus",
            "thinking_param_style": "budget",
            "thinking_budget_range": [1, 81920]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    assert!(reqs[0].reasoning.is_none());
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn missing_reasoning_style_does_not_guess_reasoning_mode() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "mock-provider",
            "model": "mock",
            "thinking_strength": "high"
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    assert!(reqs[0].reasoning.is_none());
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn effort_reasoning_is_canonical_before_provider_projection() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "dashscope",
            "model": "glm-5.2",
            "thinking_strength": "high",
            "thinking_param_style": "effort",
            "reasoning_effort_options": ["high", "max"]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert_eq!(
        reqs[0].reasoning,
        Some(agent_types::ReasoningConfig {
            mode: agent_types::ReasoningMode::Effort,
            enabled: true,
            effort: Some("high".to_string()),
            budget_tokens: None,
            summary: Some(agent_types::ReasoningSummary::Auto),
        })
    );
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn reasoning_style_drives_canonical_reasoning_without_provider_branching() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "mock-provider",
            "model": "mock",
            "thinking_strength": "medium",
            "thinking_param_style": "effort",
            "reasoning_effort_options": ["low", "medium", "high"]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert_eq!(
        reqs[0].reasoning,
        Some(agent_types::ReasoningConfig {
            mode: agent_types::ReasoningMode::Effort,
            enabled: true,
            effort: Some("medium".to_string()),
            budget_tokens: None,
            summary: Some(agent_types::ReasoningSummary::Auto),
        })
    );
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_effort_reasoning_requests_a_visible_summary() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "mock-provider",
            "model": "mock",
            "thinking_strength": "auto",
            "thinking_param_style": "effort",
            "reasoning_effort_options": ["none", "low", "medium", "high", "xhigh", "max"]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert_eq!(
        reqs[0].reasoning,
        Some(agent_types::ReasoningConfig {
            mode: agent_types::ReasoningMode::Effort,
            enabled: true,
            effort: None,
            budget_tokens: None,
            summary: Some(agent_types::ReasoningSummary::Auto),
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn provider_identity_does_not_change_canonical_reasoning() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime(None).await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "openrouter",
            "model": "anthropic/claude-sonnet-4",
            "thinking_strength": "low",
            "thinking_param_style": "effort",
            "reasoning_effort_options": ["low", "medium", "high"]
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert_eq!(
        reqs[0].reasoning,
        Some(agent_types::ReasoningConfig {
            mode: agent_types::ReasoningMode::Effort,
            enabled: true,
            effort: Some("low".to_string()),
            budget_tokens: None,
            summary: Some(agent_types::ReasoningSummary::Auto),
        })
    );
    assert!(reqs[0].provider_options.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn chat_model_slot_provider_and_model_can_drive_runtime_without_default_model() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();

    let (runtime, captured) = make_runtime_without_default_model().await;
    let mut req = base_request(ws.to_path_buf(), "console");
    req.metadata.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "slot-provider",
            "model": "mock"
        }),
    );

    let stream = runtime.stream(req).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "slot provider model must be called");
    assert_eq!(reqs[0].model.as_deref(), Some("mock"));
}

#[tokio::test(flavor = "current_thread")]
async fn i1_stream_with_three_effective_skills_registers_one_skill_tool() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("docx", true, &["all"]),
            ("xlsx", true, &["all"]),
        ],
    );

    let (runtime, captured) = make_runtime(None).await;
    let working_dir = tempfile::tempdir().unwrap();
    let mut request = base_request(ws.to_path_buf(), "console");
    request.working_dir = Some(working_dir.path().to_path_buf());
    let stream = runtime.stream(request).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty(), "model must have been called");
    let names = tool_names(&reqs[0]);
    assert!(
        names.contains(&"skill".to_string()),
        "expected skill tool to be registered; got {names:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i2_stream_with_three_effective_skills_adds_one_developer_skill_catalog() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("docx", true, &["all"]),
            ("xlsx", true, &["all"]),
        ],
    );

    let (runtime, captured) = make_runtime(None).await;
    let mut request = base_request(ws.to_path_buf(), "console");
    request.metadata.insert(
        AGENT_PROMPT_FILES_METADATA_KEY.to_string(),
        serde_json::json!("Second Agent Prompt fragment"),
    );
    let stream = runtime.stream(request).await.unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    let skill_sections = reqs[0]
        .messages
        .iter()
        .filter(|message| {
            message
                .metadata
                .get("rua_model_call_context_section")
                .and_then(serde_json::Value::as_str)
                == Some("skill_catalog")
        })
        .collect::<Vec<_>>();
    assert_eq!(skill_sections.len(), 1);
    assert_eq!(skill_sections[0].role, Role::Developer);
    assert_eq!(
        reqs[0]
            .messages
            .iter()
            .filter(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("agent_prompt_files")
            })
            .count(),
        1
    );
    assert_eq!(
        reqs[0]
            .messages
            .iter()
            .filter(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("agent_custom_prompt")
            })
            .count(),
        1
    );
    let sys = system_text(&reqs[0]);
    assert!(
        sys.contains("<available_skills>"),
        "system prompt must contain <available_skills>: {sys}"
    );
    assert!(sys.contains("pdf"), "system prompt must list pdf: {sys}");
    assert!(sys.contains("docx"), "system prompt must list docx: {sys}");
    assert!(sys.contains("xlsx"), "system prompt must list xlsx: {sys}");
    assert!(
        sys.contains("Read PDF files"),
        "system prompt must contain pdf description: {sys}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i3_invoking_skill_tool_returns_skill_body() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(
        &skills_dir.join("pdf"),
        "pdf",
        "Read PDF files",
        "This is the pdf skill body.",
    );
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("docx", true, &["all"]),
            ("xlsx", true, &["all"]),
        ],
    );

    let tc = ToolCall::new("call-i3", "skill", serde_json::json!({"name": "pdf"}));
    let (runtime, captured) = make_runtime(Some(tc)).await;
    let stream = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(
        reqs.len() >= 2,
        "model must be called twice (tool round-trip); got {}",
        reqs.len()
    );
    let tool_result_text = tool_message_text(&reqs[1]);
    assert!(
        tool_result_text.contains("This is the pdf skill body."),
        "tool result must contain pdf body; got: {tool_result_text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn chat_skill_configuration_keeps_references_and_loads_current_body() {
    let _lock = SKILL_ENV_LOCK.lock().await;
    let workspace = tempfile::tempdir().unwrap();
    let skill = workspace.path().join("skills/review");
    write_skill_md(&skill, "review", "Review code", "ORIGINAL_CAPTURED_BODY");
    write_skill_json(workspace.path(), &[("review", true, &["all"])]);
    let (runtime, captured) = make_runtime(Some(ToolCall::new(
        "captured-skill",
        "skill",
        serde_json::json!({"name": "review"}),
    )))
    .await;
    let configuration = runtime
        .capability_resolver()
        .configuration(workspace.path(), None, "console")
        .await
        .unwrap();
    let serialized = serde_json::to_string(&configuration).unwrap();
    assert!(!serialized.contains("ORIGINAL_CAPTURED_BODY"));
    assert!(serde_json::to_value(&configuration).unwrap()["skills"][0]
        .get("body")
        .is_none());
    assert!(!serialized.contains(&workspace.path().to_string_lossy().to_string()));
    write_skill_md(
        &skill,
        "review",
        "Changed description",
        "CHANGED_DEFAULT_BODY",
    );
    let mut request = base_request(workspace.path().to_path_buf(), "console");
    request.metadata.insert(
        rua_agent::runtime::CHAT_SKILLS_SNAPSHOT_METADATA_KEY.to_string(),
        serde_json::to_value(&configuration.skills).unwrap(),
    );
    request.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.to_string(),
        serde_json::to_value(&configuration).unwrap(),
    );
    let events = runtime
        .stream(request)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok));
    let requests = captured.lock().unwrap();
    assert!(requests.len() >= 2);
    let result = tool_message_text(&requests[1]);
    assert!(!result.contains("ORIGINAL_CAPTURED_BODY"));
    assert!(result.contains("CHANGED_DEFAULT_BODY"));
}

#[tokio::test(flavor = "current_thread")]
async fn project_skill_configuration_loads_resource_body_without_persisting_it() {
    let _lock = SKILL_ENV_LOCK.lock().await;
    let workspace = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let skill = project.path().join(".agents/skills/review");
    write_skill_md(&skill, "review", "Review code", "PROJECT_BODY_BEFORE");
    let (runtime, captured) = make_runtime(Some(ToolCall::new(
        "project-skill",
        "skill",
        serde_json::json!({"name":"project:review"}),
    )))
    .await;
    let configuration = runtime
        .capability_resolver()
        .configuration(workspace.path(), Some(project.path()), "console")
        .await
        .unwrap();
    assert!(!serde_json::to_string(&configuration)
        .unwrap()
        .contains("PROJECT_BODY_BEFORE"));
    assert_eq!(configuration.skills.len(), 1);
    assert!(configuration.skills[0].project);
    write_skill_md(&skill, "review", "Review code", "PROJECT_BODY_CURRENT");
    let mut request = project_request(workspace.path().into(), project.path().into());
    request.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
        serde_json::to_value(configuration).unwrap(),
    );
    let mut stream = runtime.stream(request).await.unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = captured.lock().unwrap();
    assert!(!requests[0]
        .messages
        .iter()
        .any(|message| message.text_content().contains("PROJECT_BODY_CURRENT")));
    let result = tool_message_text(&requests[1]);
    assert!(result.contains("PROJECT_BODY_CURRENT"), "{result}");
    assert!(!result.contains("PROJECT_BODY_BEFORE"));
}

#[tokio::test(flavor = "current_thread")]
async fn skill_configuration_rejects_missing_and_escaping_resource_references() {
    let _lock = SKILL_ENV_LOCK.lock().await;
    let workspace = tempfile::tempdir().unwrap();
    let (runtime, _) = make_runtime(None).await;
    for relative in ["skills/missing", "../outside"] {
        let mut request = base_request(workspace.path().into(), "console");
        request.metadata.insert(rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(), serde_json::json!({
            "selection_policy":{"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"skill"}]}, "skills":[{"name":"review","description":"review","project":false,"relative_dir":relative}]
        }));
        let rejected = match runtime.stream(request).await {
            Err(_) => true,
            Ok(mut stream) => {
                let mut rejected = false;
                while let Some(event) = stream.next().await {
                    rejected |= event.is_err();
                }
                rejected
            }
        };
        assert!(
            rejected,
            "invalid reference {relative} must not load unrelated content"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn i4_stream_with_zero_effective_skills_does_not_register_skill_tool() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_json(ws, &[("pdf", false, &["all"])]);

    let (runtime, captured) = make_runtime(None).await;
    let stream = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(
        !tool_names(&reqs[0]).contains(&"skill".to_string()),
        "disabled skills must not register tool; names: {:?}",
        tool_names(&reqs[0])
    );
    let sys = system_text(&reqs[0]);
    assert!(
        sys.contains("<available_skills>") && sys.contains("No skills are installed or enabled"),
        "disabled skills must inject explicit no-skills prompt block; sys: {sys}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i5_per_channel_effective_skill_filter_still_applies() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["console"]),
            ("docx", true, &["console"]),
            ("xlsx", true, &["dingtalk"]),
        ],
    );

    let (runtime, captured) = make_runtime(None).await;
    let stream = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    let tool = reqs[0].tools.iter().find(|t| t.name == "skill");
    assert!(tool.is_some(), "skill tool must be registered for console");
    let desc = &tool.unwrap().description;
    assert!(desc.contains("pdf"), "pdf must be in tool desc: {desc}");
    assert!(desc.contains("docx"), "docx must be in tool desc: {desc}");
    assert!(
        !desc.contains("xlsx"),
        "xlsx must NOT be in tool desc for console: {desc}"
    );
    let sys = system_text(&reqs[0]);
    assert!(sys.contains("pdf"), "pdf must be in prompt: {sys}");
    assert!(sys.contains("docx"), "docx must be in prompt: {sys}");
    assert!(
        !sys.contains("xlsx"),
        "xlsx must NOT be in console prompt: {sys}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i6_skill_added_then_bump_appears_on_next_request() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_json(ws, &[("pdf", true, &["all"]), ("docx", true, &["all"])]);

    let (runtime, captured) = make_runtime(None).await;
    let stream1 = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream1.collect().await;

    let sys_first = {
        let reqs = captured.lock().unwrap();
        system_text(&reqs[0])
    };
    assert!(sys_first.contains("pdf"), "first request must have pdf");
    assert!(sys_first.contains("docx"), "first request must have docx");
    assert!(
        !sys_first.contains("xlsx"),
        "first request must not have xlsx yet"
    );

    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("docx", true, &["all"]),
            ("xlsx", true, &["all"]),
        ],
    );

    runtime.bump_prepared_generation();

    let stream2 = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream2.collect().await;

    let sys_second = {
        let reqs = captured.lock().unwrap();
        assert!(reqs.len() >= 2, "must have at least 2 captured requests");
        system_text(&reqs[1])
    };
    assert!(
        sys_second.contains("xlsx"),
        "after a generation bump the second request must include the newly added xlsx skill: {sys_second}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i7_prepared_context_is_cached_until_generation_bump() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_json(ws, &[("pdf", true, &["all"])]);

    let (runtime, captured) = make_runtime(None).await;

    let s1 = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = s1.collect().await;

    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_json(ws, &[("pdf", true, &["all"]), ("docx", true, &["all"])]);

    let s2 = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = s2.collect().await;

    runtime.bump_prepared_generation();
    let s3 = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = s3.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(reqs.len() >= 3, "must have 3 captured requests");
    let sys2 = system_text(&reqs[1]);
    assert!(
        sys2.contains("pdf") && !sys2.contains("docx"),
        "cache hit: turn 2 must still show only pdf, not the on-disk docx: {sys2}"
    );
    let sys3 = system_text(&reqs[2]);
    assert!(
        sys3.contains("docx"),
        "after bump: turn 3 must include docx: {sys3}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn i8_prepared_context_cache_is_per_channel() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );

    write_skill_json(
        ws,
        &[("pdf", true, &["console"]), ("xlsx", true, &["dingtalk"])],
    );

    let (runtime, captured) = make_runtime(None).await;

    let s_console = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = s_console.collect().await;

    let s_dingtalk = runtime
        .stream(base_request(ws.to_path_buf(), "dingtalk"))
        .await
        .unwrap();
    let _: Vec<_> = s_dingtalk.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(reqs.len() >= 2, "must have 2 captured requests");
    let sys_console = system_text(&reqs[0]);
    assert!(
        sys_console.contains("pdf") && !sys_console.contains("xlsx"),
        "console cache entry must hold pdf only: {sys_console}"
    );
    let sys_dingtalk = system_text(&reqs[1]);
    assert!(
        sys_dingtalk.contains("xlsx") && !sys_dingtalk.contains("pdf"),
        "dingtalk cache entry must hold xlsx only (not console's pdf): {sys_dingtalk}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn n1_invoking_skill_with_unknown_name_returns_is_error() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_md(
        &skills_dir.join("xlsx"),
        "xlsx",
        "Read Excel files",
        "xlsx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("docx", true, &["all"]),
            ("xlsx", true, &["all"]),
        ],
    );

    let tc = ToolCall::new(
        "call-n1",
        "skill",
        serde_json::json!({"name": "nonexistent"}),
    );
    let (runtime, captured) = make_runtime(Some(tc)).await;
    let stream = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let reqs = captured.lock().unwrap();
    assert!(
        reqs.len() >= 2,
        "model must be called for tool round-trip; got {}",
        reqs.len()
    );
    let tool_result_text = tool_message_text(&reqs[1]);
    assert!(
        tool_result_text.contains("nonexistent"),
        "error body must mention the requested name; got: {tool_result_text}"
    );
    assert!(
        tool_result_text.contains("pdf")
            && tool_result_text.contains("docx")
            && tool_result_text.contains("xlsx"),
        "error body must list available skills; got: {tool_result_text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn n2_skill_with_invalid_skill_md_is_skipped_other_skills_still_register() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_skill_md(&skills_dir.join("pdf"), "pdf", "Read PDF files", "pdf body");
    write_invalid_skill_md(&skills_dir.join("bad-skill"));
    write_skill_md(
        &skills_dir.join("docx"),
        "docx",
        "Work with Word docs",
        "docx body",
    );
    write_skill_json(
        ws,
        &[
            ("pdf", true, &["all"]),
            ("bad-skill", true, &["all"]),
            ("docx", true, &["all"]),
        ],
    );

    let (runtime, captured) = make_runtime(None).await;
    let result = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await;
    assert!(
        result.is_ok(),
        "stream() must not fail when one skill is invalid"
    );
    let _: Vec<_> = result.unwrap().collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty());
    let names = tool_names(&reqs[0]);
    assert!(
        names.contains(&"skill".to_string()),
        "skill tool must still be registered; names: {names:?}"
    );
    let desc = reqs[0].tools.iter().find(|t| t.name == "skill").unwrap();
    assert!(desc.description.contains("pdf"), "pdf must be in tool desc");
    assert!(
        desc.description.contains("docx"),
        "docx must be in tool desc"
    );
    assert!(
        !desc.description.contains("bad-skill"),
        "bad-skill must not appear in tool desc: {}",
        desc.description
    );
}

#[tokio::test(flavor = "current_thread")]
async fn n3_all_skills_invalid_degrades_to_zero_state() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let skills_dir = ws.join("skills");

    write_invalid_skill_md(&skills_dir.join("bad1"));
    write_invalid_skill_md(&skills_dir.join("bad2"));
    write_invalid_skill_md(&skills_dir.join("bad3"));
    write_skill_json(
        ws,
        &[
            ("bad1", true, &["all"]),
            ("bad2", true, &["all"]),
            ("bad3", true, &["all"]),
        ],
    );

    let (runtime, captured) = make_runtime(None).await;
    let result = runtime
        .stream(base_request(ws.to_path_buf(), "console"))
        .await;
    assert!(
        result.is_ok(),
        "stream() must not fail when all skills are invalid"
    );
    let _: Vec<_> = result.unwrap().collect().await;

    let reqs = captured.lock().unwrap();
    assert!(!reqs.is_empty());
    assert!(
        !tool_names(&reqs[0]).contains(&"skill".to_string()),
        "must not register skill tool when all fail to parse"
    );
    let sys = system_text(&reqs[0]);
    assert!(
        sys.contains("<available_skills>") && sys.contains("No skills are installed or enabled"),
        "must inject explicit no-skills prompt block when all skills invalid; sys: {sys}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn p1_project_and_agent_skills_merge_without_shadowing_same_names() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let agent_workspace = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_skill_md(
        &agent_workspace.path().join("skills/review"),
        "review",
        "Agent review",
        "agent review body",
    );
    write_skill_json(agent_workspace.path(), &[("review", true, &["all"])]);
    write_skill_md(
        &project.path().join(".agents/skills/review"),
        "review",
        "Project review",
        "project review body",
    );
    write_skill_md(
        &project.path().join(".codex/skills/codex-only"),
        "codex-only",
        "Codex only",
        "must stay invisible",
    );

    let (runtime, captured) = make_runtime(Some(ToolCall::new(
        "call-project-ambiguous",
        "skill",
        serde_json::json!({"name": "review"}),
    )))
    .await;
    let stream = runtime
        .stream(project_request(
            agent_workspace.path().to_path_buf(),
            project.path().to_path_buf(),
        ))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let requests = captured.lock().unwrap();
    assert!(requests.len() >= 2);
    let system = system_text(&requests[0]);
    assert!(system.contains("source `agent`"));
    assert!(system.contains("source `project`"));
    assert!(system.contains("Agent review"));
    assert!(system.contains("Project review"));
    assert!(!system.contains("codex-only"));
    let result = tool_message_text(&requests[1]);
    assert!(result.contains("Ambiguous skill 'review'"), "{result}");
    assert!(result.contains("path:"), "{result}");
    assert!(
        !std::fs::read_to_string(agent_workspace.path().join("skills/review/SKILL.md"))
            .unwrap()
            .contains("project review body")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn p2_project_skill_source_qualified_invocation_reads_in_place() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let agent_workspace = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_skill_md(
        &agent_workspace.path().join("skills/review"),
        "review",
        "Agent review",
        "agent review body",
    );
    write_skill_json(agent_workspace.path(), &[("review", true, &["all"])]);
    let project_skill = project.path().join(".agents/skills/review");
    write_skill_md(
        &project_skill,
        "review",
        "Project review",
        "project review body",
    );
    fs::create_dir_all(project_skill.join("scripts")).unwrap();
    fs::write(
        project_skill.join("scripts/run.sh"),
        "touch SHOULD_NOT_RUN_WHEN_SKILL_IS_LOADED",
    )
    .unwrap();

    let (runtime, captured) = make_runtime(Some(ToolCall::new(
        "call-project-qualified",
        "skill",
        serde_json::json!({"name": "project:review"}),
    )))
    .await;
    let stream = runtime
        .stream(project_request(
            agent_workspace.path().to_path_buf(),
            project.path().to_path_buf(),
        ))
        .await
        .unwrap();
    let _: Vec<_> = stream.collect().await;

    let requests = captured.lock().unwrap();
    let result = tool_message_text(&requests[1]);
    assert!(result.contains("project review body"), "{result}");
    assert!(result.contains("Source: project"), "{result}");
    assert!(
        result.contains(&project_skill.display().to_string()),
        "{result}"
    );
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .filter(|tool| tool.name == "skill")
            .count(),
        1
    );
    assert!(requests[0].tools.iter().all(|tool| tool.name != "run.sh"));
    assert!(!project
        .path()
        .join("SHOULD_NOT_RUN_WHEN_SKILL_IS_LOADED")
        .exists());
}

#[tokio::test(flavor = "current_thread")]
async fn p3_project_skill_watcher_refreshes_the_next_turn_catalog() {
    let _l = SKILL_ENV_LOCK.lock().await;
    let agent_workspace = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_skill_md(
        &project.path().join(".agents/skills/first"),
        "first",
        "First Project Skill",
        "first body",
    );

    let (runtime, captured) = make_runtime(None).await;
    let first = runtime
        .stream(project_request(
            agent_workspace.path().to_path_buf(),
            project.path().to_path_buf(),
        ))
        .await
        .unwrap();
    let _: Vec<_> = first.collect().await;
    assert!(system_text(&captured.lock().unwrap()[0]).contains("First Project Skill"));

    write_skill_md(
        &project.path().join(".agents/skills/second"),
        "second",
        "Second Project Skill",
        "second body",
    );
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let second = runtime
        .stream(project_request(
            agent_workspace.path().to_path_buf(),
            project.path().to_path_buf(),
        ))
        .await
        .unwrap();
    let _: Vec<_> = second.collect().await;
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        system_text(&requests[1]).contains("Second Project Skill"),
        "next Turn did not observe watcher invalidation: {}",
        system_text(&requests[1])
    );
}

#[tokio::test]
async fn acp_session_stdio_mcp_tools_are_exposed_for_the_turn() {
    if !python3_available() {
        eprintln!("python3 not on PATH - skipping ACP session MCP test");
        return;
    }

    let (runtime, captured) = make_runtime(None).await;
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
    let mut request = base_request(workspace.path().to_path_buf(), "acp");
    let mcp_config_handle = rua_acp::server::register_request_mcp_config(serde_json::json!({
        "clients": {
            "acp_0_mock": {
                "name": "mock",
                "enabled": true,
                "transport": "stdio",
                "command": "python3",
                "args": [fixture],
                "env": {"MOCK_SCENARIO": "fifo"},
                "cwd": workspace.path(),
                "auto_connect": "manual"
            }
        }
    }));
    request.metadata.insert(
        rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY.to_string(),
        serde_json::Value::String(mcp_config_handle),
    );

    let stream = runtime.stream(request).await.expect("ACP MCP stream");
    let _: Vec<_> = stream.collect().await;

    let requests = captured.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert!(
        tool_names(&requests[0])
            .iter()
            .any(|name| name == "mcp__acp_0_mock__echo"),
        "ACP session MCP tool missing from model request: {:?}",
        tool_names(&requests[0])
    );
}

#[tokio::test]
async fn selection_policy_keeps_discovery_and_execution_consistent_for_new_product_tools() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    for restricted in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let toolkit = rua_agent::toolkit::build_core_toolkit_with_recall(
            rua_tool::default_command_executor(),
            rua_tool::ToolProfile::DesktopFull,
        );
        let mut toolkit = toolkit.create_filtered(&[]);
        toolkit.register_with_disclosure(
            agent_types::ToolDefinition {
                name: "new_product_tool".into(),
                description: "New product Tool".into(),
                parameters: serde_json::json!({"type":"object"}),
                strict: None,
                annotations: Default::default(),
                metadata: Default::default(),
            },
            Arc::new(react_agent::toolkit::MockToolExecutor::new(
                "new tool executed",
            )),
            rua_agent::tool_disclosure::builtin_tool_disclosure("new_product_tool"),
        );
        let (runtime, captured) = make_runtime(Some(ToolCall::new(
            "discover",
            "list_tools",
            serde_json::json!({}),
        )))
        .await;
        let runtime = runtime.with_toolkit(Arc::new(toolkit));
        let mut configuration = runtime
            .capability_resolver()
            .configuration(workspace.path(), None, "console")
            .await
            .unwrap();
        configuration.selection_policy = if restricted {
            ToolSelectionPolicy::restricted(vec![
                ToolIdentity::builtin("list_tools"),
                ToolIdentity::builtin("read"),
            ])
        } else {
            ToolSelectionPolicy::default_enabled()
        };
        let mut request = base_request(workspace.path().into(), "console");
        request.metadata.insert(
            rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
            serde_json::to_value(configuration).unwrap(),
        );
        let events = runtime
            .stream(request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let requests = captured.lock().unwrap();
        assert!(requests.len() >= 2);
        assert_eq!(
            tool_names(&requests[0]).contains(&"new_product_tool".into()),
            !restricted
        );
        let listed = tool_message_text(&requests[1]);
        assert_eq!(
            listed.contains("**new_product_tool**"),
            !restricted,
            "{listed}"
        );
        assert!(listed.contains("**read**"));
    }
}

#[test]
fn product_and_external_sources_keep_stable_identity_and_restriction_boundaries() {
    use react_agent::toolkit::ToolSourceKind;
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let mut configuration: rua_agent::runtime::ChatCapabilityConfiguration = serde_json::from_value(serde_json::json!({
        "selection_policy":{"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[]},"skills":[]
    })).unwrap();
    let mut meta = rua_agent::tool_disclosure::mcp_tool_disclosure(Some("rua_browser".into()));
    meta.original_name = Some("navigate".into());
    // A familiar server name alone never enrolls an external source.
    assert!(!configuration.selects_tool("mcp__rua_browser__navigate", None, Some(&meta)));
    meta.product_capability = Some("browser_use".into());
    assert!(configuration.selects_tool("mcp__rua_browser__navigate", None, Some(&meta)));
    configuration.selection_policy =
        ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("read")]);
    assert!(!configuration.selects_tool("mcp__rua_browser__navigate", None, Some(&meta)));
    configuration.selection_policy =
        ToolSelectionPolicy::restricted(vec![ToolIdentity::mcp("rua_browser", "navigate")]);
    assert!(configuration.selects_tool("mcp__rua_browser__navigate", None, Some(&meta)));
    let mut policy = ToolSelectionPolicy::default_enabled();
    if let ToolSelectionPolicy::DefaultEnabled {
        disabled_capabilities,
        ..
    } = &mut policy
    {
        disabled_capabilities.push("browser_use".into());
    }
    configuration.selection_policy = policy;
    assert!(!configuration.selects_tool("mcp__rua_browser__navigate", None, Some(&meta)));
    meta.source_kind = ToolSourceKind::Builtin;
    meta.product_capability = Some("phone_use".into());
    assert!(configuration.selects_tool("phone_observe", None, Some(&meta)));
}

#[tokio::test]
async fn disconnected_phone_actions_are_excluded_while_status_stays_discoverable() {
    let workspace = tempfile::tempdir().unwrap();
    let mut registry = rua_tool::ToolRegistry::new();
    registry
        .register(
            Arc::new(rua_tool::builtin::phone_use::PhoneStatusTool),
            rua_tool::builtin::phone_use::PhoneStatusTool::spec(),
            rua_tool::ToolNamespace::Builtin,
        )
        .unwrap();
    registry
        .register(
            Arc::new(rua_tool::builtin::phone_use::PhoneObserveTool),
            rua_tool::builtin::phone_use::PhoneObserveTool::spec(),
            rua_tool::ToolNamespace::Builtin,
        )
        .unwrap();
    let mut toolkit = react_agent::AgentToolkit::new();
    rua_agent::tool_disclosure::populate_toolkit_from_registry(&mut toolkit, &registry);
    let (runtime, captured) = make_runtime(None).await;
    let runtime = runtime.with_toolkit(Arc::new(toolkit));
    let configuration = runtime
        .capability_resolver()
        .configuration(workspace.path(), None, "console")
        .await
        .unwrap();
    let mut request = base_request(workspace.path().into(), "console");
    request.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
        serde_json::to_value(configuration).unwrap(),
    );
    let events = runtime
        .stream(request)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let requests = captured.lock().unwrap();
    let names = tool_names(&requests[0]);
    assert!(names.contains(&"phone_status".into()));
    assert!(!names.contains(&"phone_observe".into()));
}

#[test]
fn bundled_mcp_ownership_comes_from_the_registered_transport() {
    let mut status = MutableMcpManager::new(true).status();
    status.key = "rua_browser".into();
    assert!(rua_agent::toolkit::product_mcp_sources(&[status.clone()]).is_empty());
    status.transport_type = "bundled_browser".into();
    assert_eq!(
        rua_agent::toolkit::product_mcp_sources(&[status.clone()])["rua_browser"],
        "browser_use"
    );
    status.transport_type = "bundled_computer".into();
    assert_eq!(
        rua_agent::toolkit::product_mcp_sources(&[status])["rua_browser"],
        "computer_use"
    );
}

struct PolicyWriteProbe(Arc<AtomicUsize>);

#[async_trait]
impl react_agent::toolkit::ToolExecutor for PolicyWriteProbe {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<agent_types::ToolResult, react_agent::AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(agent_types::ToolResult::text(
            call.id.clone(),
            "write executed",
        ))
    }
}

fn policy_probe_toolkit(writes: Arc<AtomicUsize>) -> Arc<react_agent::AgentToolkit> {
    let mut toolkit = react_agent::AgentToolkit::new();
    for name in ["read", "write", "future_product_tool"] {
        let executor: Arc<dyn react_agent::toolkit::ToolExecutor> = if name == "write" {
            Arc::new(PolicyWriteProbe(writes.clone()))
        } else {
            Arc::new(react_agent::toolkit::MockToolExecutor::new("ok"))
        };
        toolkit.register_with_disclosure(
            agent_types::ToolDefinition {
                name: name.into(),
                description: name.into(),
                parameters: serde_json::json!({"type":"object"}),
                strict: None,
                annotations: Default::default(),
                metadata: Default::default(),
            },
            executor,
            rua_agent::tool_disclosure::builtin_tool_disclosure(name),
        );
    }
    Arc::new(toolkit)
}

async fn policy_cells(
    root: &std::path::Path,
    policy: serde_json::Value,
) -> Arc<rua_agent::AgentCells> {
    let cells = Arc::new(rua_agent::AgentCells::new(
        Arc::new(cell_store::FileSystemCellStore::new()),
        root.join("workspaces"),
    ));
    cells
        .section::<serde_json::Value>("research", "tools")
        .write(serde_json::json!({
            "selection_policy":policy,
            "builtin_tools":{"read":{"max_bytes":123,"future_behavior":true}}
        }))
        .await
        .unwrap();
    cells
}

#[tokio::test]
async fn agent_policy_is_captured_at_runtime_boundary_for_unconfigured_channel_requests() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    for (restricted, allowlist) in [(true, false), (true, true), (false, false), (false, true)] {
        let workspace = tempfile::tempdir().unwrap();
        let policy = if restricted {
            ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("read")])
        } else {
            ToolSelectionPolicy::default_enabled()
        };
        let cells = policy_cells(workspace.path(), serde_json::to_value(policy).unwrap()).await;
        let writes = Arc::new(AtomicUsize::new(0));
        let (runtime, captured) =
            make_runtime(Some(ToolCall::new("write", "write", serde_json::json!({})))).await;
        let runtime = runtime
            .with_toolkit(policy_probe_toolkit(writes.clone()))
            .with_agent_tool_configuration("research", cells, workspace.path().into());
        let mut request = base_request(workspace.path().into(), "dingtalk");
        if allowlist {
            request.metadata.insert(
                rua_agent::runtime::CHAT_TOOL_ALLOWLIST_METADATA_KEY.into(),
                serde_json::json!(["read", "write"]),
            );
        }
        let events = runtime
            .stream(request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let requests = captured.lock().unwrap();
        let names = tool_names(&requests[0]);
        assert!(names.contains(&"read".into()));
        assert_eq!(names.contains(&"write".into()), !restricted);
        assert_eq!(
            names.contains(&"future_product_tool".into()),
            !restricted && !allowlist
        );
        assert_eq!(writes.load(Ordering::SeqCst), usize::from(!restricted));
    }
}

#[tokio::test]
async fn agent_runtime_requires_explicit_policy_without_overwriting_captured_chat_policy() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let workspace = tempfile::tempdir().unwrap();
    let cells = policy_cells(workspace.path(), serde_json::Value::Null).await;
    let (runtime, captured) = make_runtime(None).await;
    let runtime = runtime
        .with_toolkit(policy_probe_toolkit(Arc::new(AtomicUsize::new(0))))
        .with_agent_tool_configuration("research", cells.clone(), workspace.path().into());
    let error = match runtime
        .stream(base_request(workspace.path().into(), "cron"))
        .await
    {
        Ok(_) => panic!("missing policy must fail closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("required"), "{error}");
    assert!(captured.lock().unwrap().is_empty());
    let mut configuration = runtime
        .capability_resolver()
        .configuration(workspace.path(), None, "console")
        .await
        .unwrap();
    configuration.selection_policy =
        ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("read")]);
    cells
        .section::<serde_json::Value>("research", "tools")
        .write(serde_json::json!({
            "selection_policy":ToolSelectionPolicy::default_enabled(), "builtin_tools":{}
        }))
        .await
        .unwrap();
    let mut request = base_request(workspace.path().into(), "console");
    request.metadata.insert(
        rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
        serde_json::to_value(configuration).unwrap(),
    );
    let events = runtime
        .stream(request)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(tool_names(&captured.lock().unwrap()[0]), ["read"]);
}

#[tokio::test]
async fn acp_session_sources_require_the_bound_agent_policy_and_preserve_chat_enrollment() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    if !python3_available() {
        return;
    }
    for (allow_mcp, frozen_chat) in [(false, false), (true, false), (true, true)] {
        let workspace = tempfile::tempdir().unwrap();
        let policy = ToolSelectionPolicy::restricted(if allow_mcp {
            vec![ToolIdentity::mcp("acp_0_mock", "echo")]
        } else {
            vec![ToolIdentity::builtin("read")]
        });
        let cells = policy_cells(workspace.path(), serde_json::to_value(policy).unwrap()).await;
        let (runtime, captured) = make_runtime(None).await;
        let runtime =
            runtime.with_agent_tool_configuration("research", cells, workspace.path().into());
        let mut request = base_request(workspace.path().into(), "acp");
        if frozen_chat {
            let configuration = runtime
                .capability_resolver()
                .configuration(workspace.path(), None, "acp")
                .await
                .unwrap();
            request.metadata.insert(
                rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
                serde_json::to_value(configuration).unwrap(),
            );
        }
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
        let handle = rua_acp::server::register_request_mcp_config(serde_json::json!({"clients":{
            "acp_0_mock":{"name":"mock","enabled":true,"transport":"stdio","command":"python3","args":[fixture],
                "env":{"MOCK_SCENARIO":"fifo"},"cwd":workspace.path(),"auto_connect":"manual"}
        }}));
        request.metadata.insert(
            rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY.into(),
            serde_json::json!(handle),
        );
        let events = runtime
            .stream(request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(
            tool_names(&captured.lock().unwrap()[0]).contains(&"mcp__acp_0_mock__echo".into()),
            allow_mcp && !frozen_chat
        );
    }
}
