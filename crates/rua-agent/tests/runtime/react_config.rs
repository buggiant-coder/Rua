use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, ToolCall,
    ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::{stream, StreamExt};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo, Provider, ProviderInfo};
use llm_multiplex::LlmError;
use react_agent::{AgentError, AgentToolkit, MockToolExecutor, ToolExecutor};
use rua_agent::hooks::{PROJECT_INSTRUCTIONS_METADATA_KEY, PROJECT_SKILLS_ROOT_METADATA_KEY};
use rua_agent::prompt::{
    AGENT_CUSTOM_PROMPT_METADATA_KEY, AGENT_PROMPT_FILES_METADATA_KEY,
    WORKSPACE_CONTEXT_METADATA_KEY,
};
use rua_agent::runtime::{
    build_default_toolkit, ReactAgentRuntime, EXECUTION_ENVIRONMENT_METADATA_KEY,
};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::message::ContentPart;
use rua_provider::manager::ProviderManager;

struct SequencedModel {
    sequences: Arc<Mutex<VecDeque<Vec<AgentEvent>>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for SequencedModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: agent_types::ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: None,
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: HashMap::new(),
        })
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        fn observation(part: &ContentPart) -> Option<String> {
            match part {
                ContentPart::Text { text } => serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| v["observation_id"].as_str().map(str::to_owned)),
                ContentPart::ToolResult { result } => result.content.iter().find_map(observation),
                _ => None,
            }
        }
        let observed = request
            .messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter())
            .find_map(observation);
        self.requests.lock().expect("requests lock").push(request);
        let mut events = self
            .sequences
            .lock()
            .expect("sequences lock")
            .pop_front()
            .unwrap_or_else(|| {
                vec![
                    AgentEvent::Start {
                        run_id: "mock-run".into(),
                        model: Some(self.info.id.clone()),
                        metadata: serde_json::Value::Null,
                    },
                    AgentEvent::TextDelta {
                        delta: "fallback".into(),
                        metadata: serde_json::Value::Null,
                    },
                    AgentEvent::End {
                        finish_reason: FinishReason::Stop,
                        usage: None,
                        metadata: serde_json::Value::Null,
                    },
                ]
            });
        for event in &mut events {
            if let AgentEvent::ToolCall { call } = event {
                if call.arguments["observation_id"] == "$latest-observation" {
                    call.arguments["observation_id"] = serde_json::json!(observed
                        .as_ref()
                        .expect("model must receive the observation before input"));
                }
            }
        }
        Box::pin(stream::iter(events.into_iter().map(Ok)))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct SequencedProvider {
    image_capability: Option<bool>,
    info: ProviderInfo,
    sequences: Arc<Mutex<VecDeque<Vec<AgentEvent>>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

struct RecordingWorkingDirTool {
    observed: Arc<Mutex<Option<std::path::PathBuf>>>,
}

struct RecordingToolMetadata {
    observed: Arc<Mutex<Option<HashMap<String, serde_json::Value>>>>,
    observed_tool_result_store_dir: Arc<Mutex<Option<std::path::PathBuf>>>,
}

#[async_trait]
impl ToolExecutor for RecordingWorkingDirTool {
    async fn execute(
        &self,
        call: &ToolCall,
        ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, AgentError> {
        *self.observed.lock().expect("working dir lock") = Some(ctx.working_dir.clone());
        Ok(ToolResult::text(call.id.clone(), "recorded"))
    }
}

#[async_trait]
impl ToolExecutor for RecordingToolMetadata {
    async fn execute(
        &self,
        call: &ToolCall,
        ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, AgentError> {
        *self.observed.lock().expect("metadata lock") = Some(ctx.metadata.clone());
        *self
            .observed_tool_result_store_dir
            .lock()
            .expect("Tool Result Store lock") = ctx.tool_result_store_dir.clone();
        Ok(ToolResult::text(call.id.clone(), "recorded"))
    }
}

#[async_trait]
impl Provider for SequencedProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        model == "mock"
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(SequencedModel {
            sequences: Arc::clone(&self.sequences),
            requests: Arc::clone(&self.requests),
            info: ModelInfo {
                id: "mock".into(),
                name: "Mock".into(),
                context_window: Some(4096),
                max_output_tokens: Some(1024),
                supports_multimodal: self.image_capability,
                supports_image: self.image_capability,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }

    async fn check_connection(&self) -> Result<llm_multiplex::ConnectionStatus, LlmError> {
        Ok(llm_multiplex::ConnectionStatus::Connected)
    }
}

fn tool_round(id: &str) -> Vec<AgentEvent> {
    named_tool_round(id, "echo_tool")
}

fn named_tool_round(id: &str, tool_name: &str) -> Vec<AgentEvent> {
    vec![
        AgentEvent::Start {
            run_id: "mock-run".into(),
            model: Some("mock".into()),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::ToolCall {
            call: ToolCall::new(id, tool_name, serde_json::json!({})),
        },
        AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ]
}

fn text_round(text: &str) -> Vec<AgentEvent> {
    vec![
        AgentEvent::Start {
            run_id: "mock-run".into(),
            model: Some("mock".into()),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::TextDelta {
            delta: text.into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ]
}

async fn runtime_with_sequences(sequences: Vec<Vec<AgentEvent>>) -> ReactAgentRuntime {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        agent_types::ToolDefinition {
            name: "echo_tool".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("ok")),
    );

    runtime_with_sequences_and_toolkit(sequences, Arc::new(toolkit)).await
}

async fn runtime_with_sequences_and_toolkit(
    sequences: Vec<Vec<AgentEvent>>,
    toolkit: Arc<AgentToolkit>,
) -> ReactAgentRuntime {
    runtime_with_sequences_and_toolkit_recording(sequences, toolkit)
        .await
        .0
}

async fn runtime_with_sequences_and_toolkit_recording(
    sequences: Vec<Vec<AgentEvent>>,
    toolkit: Arc<AgentToolkit>,
) -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    runtime_with_model_capability(sequences, toolkit, None).await
}

async fn runtime_with_model_capability(
    sequences: Vec<Vec<AgentEvent>>,
    toolkit: Arc<AgentToolkit>,
    image_capability: Option<bool>,
) -> (ReactAgentRuntime, Arc<Mutex<Vec<ChatRequest>>>) {
    let pm = ProviderManager::new();
    let requests = Arc::new(Mutex::new(Vec::new()));
    pm.register(Arc::new(SequencedProvider {
        image_capability,
        info: ProviderInfo {
            id: "mock-provider".into(),
            name: "Mock Provider".into(),
            provider_type: Some("mock".into()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        sequences: Arc::new(Mutex::new(VecDeque::from(sequences))),
        requests: Arc::clone(&requests),
    }));
    pm.store_default_model(rua_provider::config_loader::DefaultModel {
        provider_id: "mock-provider".into(),
        model: "mock".into(),
        source: None,
        extra: Default::default(),
    })
    .await;

    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 1,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;

    (
        ReactAgentRuntime::new(Arc::new(pm))
            .with_running_config(Arc::new(running))
            .with_toolkit(toolkit),
        requests,
    )
}

fn request(workspace_dir: std::path::PathBuf) -> AgentRequest {
    AgentRequest {
        session_id: "max-iters-session".into(),
        user_id: "user".into(),
        agent_id: "agent".into(),
        messages: vec![AgentInputMessage {
            role: "user".into(),
            content: vec![ContentPart::Text { text: "go".into() }],
        }],
        model_override: None,
        tool_mode: "auto".into(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: HashMap::new(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace_dir),
        conversation_store_dir: None,
        working_dir: None,
    }
}

#[tokio::test]
async fn agent_scoped_model_calls_do_not_inject_legacy_static_sections() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![tool_round("tc-contract"), text_round("done")],
        Arc::new({
            let mut toolkit = AgentToolkit::new();
            toolkit.register(
                ToolDefinition {
                    name: "echo_tool".into(),
                    description: "echo".into(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                    strict: None,
                    annotations: Default::default(),
                    metadata: Default::default(),
                },
                Arc::new(MockToolExecutor::new("ok")),
            );
            toolkit
        }),
    )
    .await;

    let events: Vec<_> = runtime
        .stream(request(workspace.path().to_path_buf()))
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        for message in &request.messages {
            let section = message
                .metadata
                .get("rua_model_call_context_section")
                .and_then(serde_json::Value::as_str);
            assert!(
                !matches!(
                    section,
                    Some("base_agent_contract" | "markdown_plus" | "structured_memory")
                ),
                "legacy static section must not be injected: {section:?}"
            );
        }
    }
}

#[tokio::test]
async fn invalid_captured_model_policy_is_rejected_before_provider_call() {
    let workspace = tempfile::tempdir().unwrap();
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![text_round("should not execute")],
        Arc::new(AgentToolkit::new()),
    )
    .await;
    let mut request = request(workspace.path().to_path_buf());
    let mut running = serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap();
    running["llm_max_concurrent"] = serde_json::json!(0);
    request.metadata.insert(
        rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.to_string(),
        running,
    );
    assert!(runtime.stream(request).await.is_err());
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn captured_chat_without_security_snapshot_never_calls_provider() {
    let workspace = tempfile::tempdir().unwrap();
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![text_round("must not run")],
        Arc::new(AgentToolkit::new()),
    )
    .await;
    let mut input = request(workspace.path().to_path_buf());
    input.metadata.insert(
        "chat_configuration_epoch".into(),
        serde_json::json!("epoch"),
    );
    assert!(runtime.stream(input).await.is_err());
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn captured_security_deny_prevents_real_tool_executor() {
    let workspace = tempfile::tempdir().unwrap();
    let observed = Arc::new(Mutex::new(None));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "bash".into(),
            description: "record only".into(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(RecordingWorkingDirTool {
            observed: observed.clone(),
        }),
    );
    let mut events = named_tool_round("denied-call", "bash");
    for event in &mut events {
        if let AgentEvent::ToolCall { call } = event {
            call.arguments = serde_json::json!({"command": "echo hello"});
        }
    }
    let (runtime, _) = runtime_with_sequences_and_toolkit_recording(
        vec![events, text_round("denied safely")],
        Arc::new(toolkit),
    )
    .await;
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            workspace.path().to_path_buf(),
        )),
        None,
    ));
    let runtime = runtime.with_security(security);
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.custom_rules.push(serde_json::from_value(serde_json::json!({"id": "deny-echo", "tools": ["bash"], "command_set": ["echo"], "action": "deny"})).unwrap());
    let mut input = request(workspace.path().to_path_buf());
    input.metadata.insert(
        "chat_configuration_epoch".into(),
        serde_json::json!("captured"),
    );
    input.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(config).unwrap(),
    );
    let events = runtime
        .stream(input)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok));
    assert!(observed.lock().unwrap().is_none());
}

#[tokio::test]
async fn chat_tool_allowlist_filters_the_actual_model_tool_surface() {
    let workspace = tempfile::tempdir().unwrap();
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "later_added_tool".into(),
            description: "not in Chat snapshot".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("unexpected")),
    );
    let (runtime, requests) =
        runtime_with_sequences_and_toolkit_recording(vec![text_round("done")], Arc::new(toolkit))
            .await;
    let mut input = request(workspace.path().to_path_buf());
    input.metadata.insert(
        rua_agent::runtime::CHAT_TOOL_ALLOWLIST_METADATA_KEY.to_string(),
        serde_json::json!([]),
    );
    let mut stream = runtime.stream(input).await.unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = requests.lock().unwrap();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|request| request
        .tools
        .iter()
        .all(|tool| tool.name != "later_added_tool")));
}

#[tokio::test]
async fn chat_mcp_selection_keeps_restrictions_and_dispatches_only_selected_tools() {
    use mcp_manager::manager::McpManager;
    use mcp_manager::types::{McpConfig, McpServerId};
    use rua_agent::runtime::{
        ChatCapabilityConfiguration, CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY,
    };
    use serde_json::json;

    let workspace = tempfile::tempdir().unwrap();
    let driver = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
    let start_log = workspace.path().join("mcp-starts");
    let manager = Arc::new(rua_mcp::McpManagerImpl::new(rua_mcp::McpMobilePolicy::new(
        rua_mcp::PlatformProfile::Desktop,
    )));
    let server = json!({"name":"fixture","enabled":true,"transport":"stdio","command":"python3","args":[driver],
        "auto_connect":"lazy_on_first_use","env":{"MOCK_START_LOG":start_log,"FIXTURE_SECRET":"must-not-be-snapshotted"}});
    let mut config = McpConfig::default();
    for name in ["selected", "restricted", "disabled"] {
        let mut value = server.clone();
        if name == "restricted" {
            value["disabled_tools"] = json!(["echo"]);
        }
        if name == "disabled" {
            value["enabled"] = json!(false);
        }
        config.clients.insert(
            McpServerId::new(name),
            serde_json::from_value(value).unwrap(),
        );
    }
    manager.load_config(config.clone()).await.unwrap();
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![
            vec![
                AgentEvent::ToolCall {
                    call: ToolCall::new(
                        "mcp-call",
                        "mcp__selected__echo",
                        json!({"msg":"selected executor proof"}),
                    ),
                },
                AgentEvent::End {
                    finish_reason: FinishReason::ToolUse,
                    usage: None,
                    metadata: serde_json::Value::Null,
                },
            ],
            text_round("done"),
        ],
        Arc::new(AgentToolkit::new()),
    )
    .await;
    let runtime = runtime.with_mcp_manager(manager.clone());
    let captured = runtime
        .capability_resolver()
        .configuration(workspace.path(), None, "console")
        .await
        .unwrap();
    assert!(!start_log.exists(), "configuration must not spawn MCP");
    assert_eq!(
        captured.mcp_servers.keys().cloned().collect::<Vec<_>>(),
        vec!["restricted", "selected"]
    );
    let value = serde_json::to_value(&captured).unwrap();
    assert!(!value.to_string().contains("FIXTURE_SECRET"));
    assert!(!value.to_string().contains("must-not-be-snapshotted"));
    assert!(!value
        .to_string()
        .contains(&workspace.path().to_string_lossy().to_string()));
    let round_trip: ChatCapabilityConfiguration = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(captured, round_trip);
    config
        .clients
        .get_mut(&McpServerId::new("restricted"))
        .unwrap()
        .disabled_tools
        .clear();
    config
        .clients
        .get_mut(&McpServerId::new("disabled"))
        .unwrap()
        .enabled = true;
    // Even a source whose textual prefix overlaps must not inherit this Chat's selection.
    for name in ["later-added", "selected__nested"] {
        config.clients.insert(
            McpServerId::new(name),
            serde_json::from_value(server.clone()).unwrap(),
        );
    }
    manager.load_config(config).await.unwrap();
    let mut input = request(workspace.path().into());
    input
        .metadata
        .insert(CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(), value);
    let events = runtime
        .stream(input)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    manager.shutdown_all().await.unwrap();
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(events.iter().any(|event| matches!(event, Ok(rua_core::event::RuntimeEvent::ToolResult { result }) if !result.is_error && result.text_content().contains("selected executor proof"))));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let names = request.metadata["effective_capability_snapshot"]["tools"]
            .as_array()
            .unwrap();
        assert!(names.contains(&json!("mcp__selected__echo")));
        assert!(!names.iter().any(|name| name
            .as_str()
            .is_some_and(|name| name.starts_with("mcp__") && name != "mcp__selected__echo")));
    }
}

#[tokio::test]
async fn configuration_does_not_discover_active_plugin_tools() {
    use rua_plugin_runtime::{ActivePluginRegistry, PluginClient, PluginClientConfig};
    use std::time::Duration;

    let workspace = tempfile::tempdir().unwrap();
    let log = workspace.path().join("plugin-requests");
    let driver = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
    let mut config = PluginClientConfig::new("selected-plugin", "unused");
    config.command = Some("python3".into());
    config.args = Some(vec![driver.to_string_lossy().into_owned()]);
    config.working_dir = workspace.path().into();
    config.heartbeat_interval = Duration::from_secs(60);
    config.env.insert("MOCK_SCENARIO".into(), "delay".into());
    config.env.insert("MOCK_DELAY_MS".into(), "5000".into());
    config.env.insert(
        "MOCK_REQUEST_LOG".into(),
        log.to_string_lossy().into_owned(),
    );
    let registry = Arc::new(ActivePluginRegistry::new());
    registry
        .insert(
            "selected-plugin".into(),
            PluginClient::spawn(config).await.unwrap(),
        )
        .await
        .unwrap();
    let runtime = runtime_with_sequences(vec![])
        .await
        .with_active_plugin_registry(registry.clone());
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        runtime
            .capability_resolver()
            .configuration(workspace.path(), None, "console"),
    )
    .await;
    registry
        .remove("selected-plugin")
        .await
        .unwrap()
        .client
        .shutdown()
        .await
        .unwrap();
    let configuration = result
        .expect("configuration cannot wait on plugin tools/list")
        .unwrap();
    assert_eq!(configuration.plugin_ids, ["selected-plugin"]);
    assert!(!std::fs::read_to_string(log).unwrap().contains("tools/list"));
}

#[test]
fn old_chat_capability_configuration_round_trips_without_adding_permissions() {
    use rua_agent::runtime::ChatCapabilityConfiguration;
    use serde_json::json;
    let value = json!({"tools":["mcp__saved__echo"],"skills":[],"future_field":{"keep":true}});
    let configuration: ChatCapabilityConfiguration = serde_json::from_value(value.clone()).unwrap();
    assert!(configuration.mcp_servers.is_empty());
    assert!(configuration.plugin_ids.is_empty());
    assert_eq!(serde_json::to_value(configuration).unwrap(), value);
}

#[tokio::test]
async fn first_turn_bootstrap_uses_typed_section_and_marks_completion() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("BOOTSTRAP.md"), "Bootstrap guidance").unwrap();
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![text_round("done")],
        Arc::new(AgentToolkit::new()),
    )
    .await;

    let events: Vec<_> = runtime
        .stream(request(workspace.path().to_path_buf()))
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    {
        let requests = requests.lock().expect("requests lock");
        let bootstrap = requests[0]
            .messages
            .iter()
            .filter(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("bootstrap_context")
            })
            .collect::<Vec<_>>();
        assert_eq!(bootstrap.len(), 1);
        assert_eq!(bootstrap[0].role, Role::Developer);
        assert!(bootstrap[0].text_content().contains("Bootstrap guidance"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !workspace.path().join(".bootstrap_completed").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("bootstrap completion flag");
}

#[tokio::test]
async fn model_call_context_diagnostics_are_recaptured_for_each_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![tool_round("tc-context"), text_round("done")],
        Arc::new({
            let mut toolkit = AgentToolkit::new();
            toolkit.register(
                ToolDefinition {
                    name: "echo_tool".into(),
                    description: "echo".into(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                    strict: None,
                    annotations: Default::default(),
                    metadata: Default::default(),
                },
                Arc::new(MockToolExecutor::new("ok")),
            );
            toolkit
        }),
    )
    .await;

    let events: Vec<_> = runtime
        .stream(request(workspace.path().to_path_buf()))
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    for (iteration, request) in requests.iter().enumerate() {
        let diagnostic = request
            .metadata
            .get("rua_model_call_context")
            .and_then(serde_json::Value::as_object)
            .expect("Model Call Context diagnostic");
        assert_eq!(
            diagnostic
                .get("iteration")
                .and_then(serde_json::Value::as_u64),
            Some(iteration as u64)
        );
        assert!(diagnostic
            .get("input_estimate_tokens")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|tokens| tokens > 0));
        let sections = diagnostic
            .get("sections")
            .and_then(serde_json::Value::as_object)
            .expect("Model Call Context section diagnostics");
        for removed in ["base_agent_contract", "markdown_plus", "structured_memory"] {
            assert!(!sections.contains_key(removed));
        }
        assert_eq!(
            diagnostic
                .get("context_window_tokens")
                .and_then(serde_json::Value::as_u64),
            Some(4096)
        );
        let messages = diagnostic
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .expect("ordered Message diagnostics");
        assert_eq!(messages[0]["index"], serde_json::json!(0));
        assert!(
            messages
                .iter()
                .all(|message| message.get("content").is_none()),
            "default diagnostics must not expose prompt bodies"
        );
        assert_eq!(
            diagnostic
                .get("tool_names")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(request.tools.len())
        );
    }
}

#[tokio::test]
async fn running_max_iters_limits_react_loop_and_reports_the_terminal_reason() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_sequences(vec![
        tool_round("tc-0"),
        text_round("summary after max iterations"),
    ])
    .await;

    let events: Vec<_> = runtime
        .stream(request(tmp.path().to_path_buf()))
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().any(|event| matches!(event,
        Ok(rua_core::RuntimeEvent::End { finish_reason })
            if finish_reason.as_deref() == Some("max_iterations"))));
    let text = events
        .iter()
        .filter_map(|event| match event.as_ref().expect("event") {
            rua_core::event::RuntimeEvent::TextDelta { delta, message_id } => {
                assert!(
                    message_id.is_some(),
                    "runtime notices must not fabricate assistant deltas"
                );
                Some(delta.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    assert!(text.contains("summary after max iterations"));
}

#[tokio::test]
async fn iteration_limit_disables_tools_after_capability_projection() {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo_tool".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("ok")),
    );
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![tool_round("first"), text_round("limit summary")],
        Arc::new(toolkit),
    )
    .await;
    let workspace = tempfile::tempdir().unwrap();
    let events = runtime
        .stream(request(workspace.path().to_path_buf()))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].tools.is_empty());
    assert!(
        requests[1].tools.is_empty(),
        "capability projection must honor a tool-free call"
    );
    assert!(events.iter().any(|event| matches!(event,
        Ok(rua_core::RuntimeEvent::End { finish_reason })
            if finish_reason.as_deref() == Some("max_iterations"))));
}

#[tokio::test]
async fn zero_iterations_allows_multiple_tool_rounds_until_the_model_finishes() {
    let runtime = runtime_with_sequences(vec![
        tool_round("first"),
        tool_round("second"),
        text_round("all requested work finished"),
    ])
    .await;
    let workspace = tempfile::tempdir().unwrap();
    let mut input = request(workspace.path().to_path_buf());
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 0,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;
    input.metadata.insert(
        rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(running).unwrap(),
    );
    let events = runtime
        .stream(input)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(rua_core::RuntimeEvent::ToolResult { .. })))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(event,
        Ok(rua_core::RuntimeEvent::TextDelta { delta, .. })
            if delta == "all requested work finished")));
    assert!(events.iter().all(|event| !matches!(event,
        Ok(rua_core::RuntimeEvent::End { finish_reason })
            if finish_reason.as_deref() == Some("max_iterations"))));
}

#[tokio::test]
async fn iteration_limit_note_is_durable_and_visible_to_the_summary_and_next_projection() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    let chat_id = "max-iters-session";
    let turn_id = "limited-turn";
    store
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.into(),
                agent_id: "agent".into(),
                title: "iteration limit".into(),
                created_at: "2026-09-22T00:00:00Z".into(),
                last_activity_at: "2026-09-22T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: turn_id.into(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-09-22T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![rua_session::NewConversationMessage {
                    message_id: "limited-user".into(),
                    role: "user".into(),
                    created_at: "2026-09-22T00:00:00Z".into(),
                    status: "completed".into(),
                    content: serde_json::json!([{"type":"text","text":"finish all issues"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo_tool".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type":"object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("tool finished")),
    );
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![
            tool_round("limit-tool"),
            text_round("unfinished work remains"),
        ],
        Arc::new(toolkit),
    )
    .await;
    let mut input = request(workspace.path().to_path_buf());
    input.metadata.insert("turn_id".into(), turn_id.into());
    input
        .metadata
        .insert("loaded_message_count".into(), 1.into());
    runtime
        .stream(input.clone())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let requests = requests.lock().unwrap().clone();
    assert!(requests[1]
        .messages
        .iter()
        .any(|message| message.text_content().contains("reason: max_iterations")));
    drop(runtime);
    let reloaded = rua_session::WorkspaceSessionStore::new(workspace.path());
    let finalized = rua_session::TurnWriter::new(&reloaded, chat_id, turn_id)
        .finalize(false, None)
        .await
        .unwrap();
    assert_eq!(finalized.lifecycle.status, "interrupted");
    let history = reloaded.read_turn_messages(chat_id, turn_id).await.unwrap();
    let notes = history
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 1);
    assert!(matches!(&notes[0].payload,
        rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(note)
            if note.reason == "max_iterations" && note.terminal_status == "interrupted"
                && note.summary.contains("1 tool rounds")));
    let last_result = history
        .iter()
        .position(|message| message.role == "tool")
        .unwrap();
    let note_index = history
        .iter()
        .position(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_some())
        .unwrap();
    assert!(
        note_index > last_result,
        "the note must follow the complete tool round"
    );
    let projection = reloaded
        .build_context_projection(
            chat_id,
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    let model_messages = rua_agent::projection_preparation::project_message_log_entries_for_model(
        &projection.tail_messages,
    );
    assert!(model_messages
        .iter()
        .any(|message| message.text_content().contains("reason: max_iterations")));
    let hook = rua_agent::turn_persistence::TurnPersistenceHook::from_request(&input).unwrap();
    react_agent::AgentHook::on_iteration_limit(&hook, 1)
        .await
        .unwrap();
    assert_eq!(
        reloaded
            .read_turn_messages(chat_id, turn_id)
            .await
            .unwrap()
            .iter()
            .filter_map(rua_session::RuntimeNote::from_message_log_entry)
            .count(),
        1
    );
}

#[tokio::test]
async fn captured_chat_iterations_override_runtime_defaults_during_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let runtime = runtime_with_sequences(vec![
        tool_round("first"),
        tool_round("second"),
        text_round("completed captured work"),
    ])
    .await;
    let mut input = request(workspace.path().to_path_buf());
    let mut captured = rua_config::AgentsRunningConfig {
        max_iters: 4,
        ..Default::default()
    };
    captured.context_compact.context_compact_enabled = false;
    input.metadata.insert(
        rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.to_string(),
        serde_json::to_value(captured).unwrap(),
    );
    let events = runtime
        .stream(input)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let text = events
        .into_iter()
        .filter_map(|event| match event.unwrap() {
            rua_core::event::RuntimeEvent::TextDelta { delta, .. } => Some(delta),
            _ => None,
        })
        .collect::<String>();
    assert!(text.contains("completed captured work"));
    assert!(!text.contains("Maximum iterations reached"));
}

#[tokio::test]
async fn tools_use_explicit_working_dir_instead_of_agent_workspace() {
    let agent_workspace = tempfile::tempdir().expect("agent workspace");
    let project_root = tempfile::tempdir().expect("project root");
    let observed = Arc::new(Mutex::new(None));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "record_working_dir".to_string(),
            description: "record working dir".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(RecordingWorkingDirTool {
            observed: Arc::clone(&observed),
        }),
    );
    let runtime = runtime_with_sequences_and_toolkit(
        vec![
            named_tool_round("tc-working-dir", "record_working_dir"),
            text_round("done"),
        ],
        Arc::new(toolkit),
    )
    .await;
    let mut request = request(agent_workspace.path().to_path_buf());
    request.working_dir = Some(project_root.path().to_path_buf());

    let events: Vec<_> = runtime
        .stream(request)
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));
    assert_eq!(
        observed.lock().expect("working dir lock").as_deref(),
        Some(project_root.path())
    );
}

#[tokio::test]
async fn project_instructions_are_model_only_and_reapplied_once_per_model_call() {
    let agent_workspace = tempfile::tempdir().expect("agent workspace");
    let project_root = tempfile::tempdir().expect("project root");
    let project_scope = tempfile::tempdir().expect("Project data scope");
    let observed_metadata = Arc::new(Mutex::new(None));
    let observed_tool_result_store_dir = Arc::new(Mutex::new(None));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "record_context".to_string(),
            description: "record ToolContext metadata".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(RecordingToolMetadata {
            observed: Arc::clone(&observed_metadata),
            observed_tool_result_store_dir: Arc::clone(&observed_tool_result_store_dir),
        }),
    );
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![
            named_tool_round("tc-context", "record_context"),
            text_round("done"),
        ],
        Arc::new(toolkit),
    )
    .await;
    let project_instructions =
        "# Project Instructions\n\nRepository instruction marker: project-only";
    let mut request = request(agent_workspace.path().to_path_buf());
    request.working_dir = Some(project_root.path().to_path_buf());
    request.conversation_store_dir = Some(project_scope.path().to_path_buf());
    request
        .metadata
        .insert("channel".to_string(), serde_json::json!("console"));
    request.messages = vec![AgentInputMessage {
        role: "user".to_string(),
        content: vec![ContentPart::Text {
            text: "Implement the change".to_string(),
        }],
    }];
    request.metadata.insert(
        AGENT_PROMPT_FILES_METADATA_KEY.to_string(),
        serde_json::json!("Mandatory protocols"),
    );
    request.metadata.insert(
        AGENT_CUSTOM_PROMPT_METADATA_KEY.to_string(),
        serde_json::json!("Assigned Agent instructions"),
    );
    request.metadata.insert(
        WORKSPACE_CONTEXT_METADATA_KEY.to_string(),
        serde_json::json!("Workspace context marker"),
    );
    request.metadata.insert(
        PROJECT_INSTRUCTIONS_METADATA_KEY.to_string(),
        serde_json::json!(project_instructions),
    );
    request.metadata.insert(
        PROJECT_SKILLS_ROOT_METADATA_KEY.to_string(),
        serde_json::json!(project_root.path()),
    );
    request.metadata.insert(
        EXECUTION_ENVIRONMENT_METADATA_KEY.to_string(),
        serde_json::json!("Runtime environment marker"),
    );

    let events: Vec<_> = runtime
        .stream(request)
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let texts = request
            .messages
            .iter()
            .map(ModelDialogueItem::text_content)
            .collect::<Vec<_>>();
        let prompt_files = request
            .messages
            .iter()
            .position(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("agent_prompt_files")
            })
            .expect("Agent Prompt Files");
        let custom_prompt = request
            .messages
            .iter()
            .position(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("agent_custom_prompt")
            })
            .expect("Agent Custom Prompt");
        let workspace_context = request
            .messages
            .iter()
            .position(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("workspace_context")
            })
            .expect("Workspace Context");
        let project = texts
            .iter()
            .position(|text| text == project_instructions)
            .expect("Project Instructions");
        let environment = request
            .messages
            .iter()
            .position(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("execution_environment")
            })
            .expect("Execution Environment");
        let user = texts
            .iter()
            .position(|text| text == "Implement the change")
            .expect("user input");
        assert!(
            prompt_files < custom_prompt
                && custom_prompt < workspace_context
                && workspace_context < project
                && project < environment
                && environment < user
        );
        assert_eq!(texts[environment], "Runtime environment marker");
        assert_eq!(request.messages[environment].role, Role::Developer);
        assert_eq!(texts[prompt_files], "Mandatory protocols");
        assert_eq!(texts[custom_prompt], "Assigned Agent instructions");
        assert_eq!(texts[workspace_context], "Workspace context marker");
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|message| message.text_content() == project_instructions)
                .count(),
            1
        );
        let project_message = &request.messages[project];
        assert_eq!(project_message.role, Role::Developer);
        assert_eq!(
            project_message
                .metadata
                .get("rua_model_call_context_section")
                .and_then(serde_json::Value::as_str),
            Some("project_instructions")
        );
    }

    let tool_metadata = observed_metadata
        .lock()
        .expect("metadata lock")
        .clone()
        .expect("ToolContext metadata");
    assert!(!tool_metadata.contains_key(PROJECT_INSTRUCTIONS_METADATA_KEY));
    assert!(!tool_metadata.contains_key(PROJECT_SKILLS_ROOT_METADATA_KEY));
    assert!(!tool_metadata.contains_key(AGENT_PROMPT_FILES_METADATA_KEY));
    assert!(!tool_metadata.contains_key(AGENT_CUSTOM_PROMPT_METADATA_KEY));
    assert!(!tool_metadata.contains_key(WORKSPACE_CONTEXT_METADATA_KEY));
    assert_eq!(
        observed_tool_result_store_dir
            .lock()
            .expect("Tool Result Store lock")
            .as_ref(),
        Some(&project_scope.path().join("tool_call_logs"))
    );
}

#[tokio::test]
async fn agent_scoped_sections_are_reapplied_once_per_model_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo_tool".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("ok")),
    );
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![tool_round("tc-agent-scope"), text_round("done")],
        Arc::new(toolkit),
    )
    .await;
    let mut request = request(workspace.path().to_path_buf());
    request.messages = vec![AgentInputMessage {
        role: "user".to_string(),
        content: vec![ContentPart::Text {
            text: "Continue".to_string(),
        }],
    }];
    request
        .metadata
        .insert("channel".to_string(), serde_json::json!("console"));
    request.metadata.insert(
        AGENT_PROMPT_FILES_METADATA_KEY.to_string(),
        serde_json::json!("Mandatory protocols"),
    );
    request.metadata.insert(
        AGENT_CUSTOM_PROMPT_METADATA_KEY.to_string(),
        serde_json::json!("Assigned Agent instructions"),
    );
    request.metadata.insert(
        WORKSPACE_CONTEXT_METADATA_KEY.to_string(),
        serde_json::json!("Workspace context marker"),
    );

    let events: Vec<_> = runtime
        .stream(request)
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    for (call_index, request) in requests.iter().enumerate() {
        let sections = request
            .messages
            .iter()
            .filter_map(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    .map(|section| (section, message))
            })
            .collect::<Vec<_>>();
        let mut expected = vec![
            "agent_prompt_files",
            "agent_custom_prompt",
            "workspace_context",
            "skill_catalog",
        ];
        if call_index == 1 {
            expected.push("runtime_guidance");
        }
        assert_eq!(
            sections
                .iter()
                .map(|(section, _)| *section)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(sections
            .iter()
            .all(|(_, message)| message.role == Role::Developer));
        assert_eq!(sections[0].1.text_content(), "Mandatory protocols");
        assert_eq!(sections[1].1.text_content(), "Assigned Agent instructions");
        assert_eq!(sections[2].1.text_content(), "Workspace context marker");
        assert!(sections[3].1.text_content().contains("<available_skills>"));
        if call_index == 1 {
            assert!(!sections[4].1.text_content().is_empty());
        }
        assert!(request
            .messages
            .iter()
            .all(|message| !message.text_content().contains("# Project Instructions")));
    }
}

#[tokio::test]
async fn execute_collaboration_mode_is_reapplied_once_per_model_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo_tool".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("ok")),
    );
    toolkit.register(
        ToolDefinition {
            name: "chat_send".into(),
            description: "collaboration".into(),
            parameters: serde_json::json!({"type":"object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("unused")),
    );
    let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
        vec![
            tool_round("tc-execute-mode"),
            tool_round("tc-refresh-mode"),
            text_round("done"),
        ],
        Arc::new(toolkit),
    )
    .await;
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 3,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;
    let runtime = runtime
        .with_running_config(Arc::new(running))
        .with_collaboration_runtime(Arc::new(
            rua_tool::extended::collaboration::CollaborationRuntimeHandle::new(Arc::new(
                ChangingInventory(std::sync::atomic::AtomicUsize::new(0)),
            )),
        ));
    let mut request = request(workspace.path().to_path_buf());
    request.metadata.insert(
        "agent_collaboration_mode".to_string(),
        serde_json::json!("execute"),
    );

    let events: Vec<_> = runtime
        .stream(request)
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok));

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 3);
    for (index, request) in requests.iter().enumerate() {
        let modes = request
            .messages
            .iter()
            .filter(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("collaboration_mode")
            })
            .collect::<Vec<_>>();
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].role, Role::Developer);
        let text = modes[0].text_content();
        assert!(text.contains("Execute"));
        assert!(text.contains("concrete unresolved question"));
        assert!(text.contains("highly overlapping retrieval"));
        assert!(text.contains("update_plan"));
        assert!(!text.contains("60 seconds"));
        assert!(text.contains("implementation"));
        assert!(text.contains("verification"));
        assert!(text.contains("delivery"));
        assert!(text.contains("Controlled collaboration"));
        assert!(text.contains("file ownership"));
        assert!(text.contains("Collaboration request progress"));
        assert!(text.contains(if index == 0 {
            "waiting_for_tool"
        } else {
            "started"
        }));
        if index == 2 {
            assert!(text.contains("Collaborator discovery unavailable"));
            assert!(!text.contains("\"agent_id\""));
            continue;
        }
        assert!(text.contains(if index == 0 {
            "\"agent_id\":\"research\""
        } else {
            "\"agent_id\":\"implementation\""
        }));
        if index == 1 {
            assert!(!text.contains("\"agent_id\":\"research\""));
        }
        assert!(text.contains("not authorization"));
    }
}

#[tokio::test]
async fn ordinary_collaborators_report_missing_model_without_substitution() {
    let workspace = tempfile::tempdir().unwrap();
    let runtime = ReactAgentRuntime::new(Arc::new(ProviderManager::new()));
    for agent in ["research", "implementation"] {
        let mut input = request(workspace.path().to_path_buf());
        input.agent_id = agent.into();
        match runtime.stream(input).await {
            Err(rua_core::RuaError::Provider {
                provider,
                message,
                retryable,
            }) => {
                assert_eq!(provider, "active");
                assert_eq!(message, "no active model configured");
                assert!(!retryable);
            }
            _ => panic!("missing configuration must not select a replacement model"),
        }
    }
}

struct ChangingInventory(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl rua_tool::extended::collaboration::CollaborationRuntime for ChangingInventory {
    async fn request_progress(
        &self,
        _source: &rua_core::ChatAddress,
    ) -> Result<serde_json::Value, String> {
        let phase = if self.0.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            "waiting_for_tool"
        } else {
            "started"
        };
        Ok(serde_json::json!([{"request_id":"pending-request","phase":phase}]))
    }
    async fn inventory(&self) -> Result<serde_json::Value, String> {
        let index = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if index >= 2 {
            return Err("discovery unavailable".into());
        }
        Ok(if index == 0 {
            serde_json::json!([{"agent_id":"research","configured_tools":["read"]}])
        } else {
            serde_json::json!([{"agent_id":"implementation","configured_tools":["edit"]}])
        })
    }
    async fn execute(
        &self,
        _: rua_tool::extended::collaboration::CollaborationInvocation,
        _: rua_tool::extended::collaboration::CollaborationRequest,
    ) -> Result<serde_json::Value, String> {
        Err("not used by inventory test".into())
    }
}

#[tokio::test]
async fn default_and_plan_collaboration_modes_have_distinct_complete_guidance() {
    for (mode, required) in [
        (
            "default",
            vec![
                "concrete unresolved question",
                "highly overlapping retrieval",
                "update_plan",
                "implementation",
                "verification",
                "completion audit",
            ],
        ),
        (
            "plan",
            vec![
                "decision-complete",
                "Do not implement",
                "explicitly authorizes execution",
                "concrete blocker",
            ],
        ),
    ] {
        let workspace = tempfile::tempdir().expect("workspace");
        let (runtime, requests) = runtime_with_sequences_and_toolkit_recording(
            vec![text_round("done")],
            Arc::new(AgentToolkit::new()),
        )
        .await;
        let mut request = request(workspace.path().to_path_buf());
        request.metadata.insert(
            "agent_collaboration_mode".to_string(),
            serde_json::json!(mode),
        );

        let events: Vec<_> = runtime
            .stream(request)
            .await
            .expect("stream")
            .collect()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let requests = requests.lock().expect("requests lock");
        let mode_text = requests[0]
            .messages
            .iter()
            .find(|message| {
                message
                    .metadata
                    .get("rua_model_call_context_section")
                    .and_then(serde_json::Value::as_str)
                    == Some("collaboration_mode")
            })
            .expect("Collaboration Mode section")
            .text_content();
        for marker in required {
            assert!(mode_text.contains(marker), "{mode}: {mode_text}");
        }
        for marker in [
            "do independent work outside the collaborator's ownership",
            "report pending background work and normally end this Turn",
            "Do not keep the Turn alive by repeated reads, sleep, or empty tool calls",
            "Do not cancel or take over merely to avoid waiting",
            "Elapsed time alone is not a cancellation reason",
            "confirm termination",
            "not repeating the same call or changing unrelated parameters",
        ] {
            assert!(mode_text.contains(marker), "{mode}: missing {marker}");
        }
    }
}

#[tokio::test]
async fn time_now_uses_timezone_from_runtime_root_config() {
    for (configured_timezone, expected_timezone) in [
        (Some("Pacific/Kiritimati"), "Pacific/Kiritimati"),
        (None, "UTC"),
    ] {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace_dir = root.path().join("workspaces").join("agent");
        std::fs::create_dir_all(&workspace_dir).expect("workspace dir");
        rua_config::save_config(
            &rua_config::RootConfig {
                user_timezone: configured_timezone.map(str::to_string),
                ..Default::default()
            },
            &root.path().join("config.json"),
        )
        .expect("save root config");

        let runtime = runtime_with_sequences_and_toolkit(
            vec![named_tool_round("tc-time", "time_now"), text_round("done")],
            build_default_toolkit(),
        )
        .await
        .with_rua_root(root.path().to_path_buf());

        let events: Vec<_> = runtime
            .stream(request(workspace_dir))
            .await
            .expect("stream")
            .collect()
            .await;
        let time_result = events
            .into_iter()
            .find_map(|event| match event.expect("event") {
                rua_core::event::RuntimeEvent::ToolResult { result } => Some(result.text_content()),
                _ => None,
            });

        assert!(
            time_result
                .as_deref()
                .is_some_and(|text| text.contains(&format!(" {expected_timezone} ("))),
            "time_now should use the root config timezone, got: {time_result:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires staged native driver and real system Chromium"]
async fn bundled_browser_normal_chat_actions_images_and_captured_tool_set() {
    browser_chat_acceptance(false).await;
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires staged agent-browser and system Chromium; Computer executor and model are protocol fixtures"]
async fn bundled_browser_and_computer_complete_one_normal_chat() {
    browser_chat_acceptance(true).await;
}

async fn browser_chat_acceptance(with_computer: bool) {
    use mcp_manager::manager::McpManager;
    use rua_core::runtime_authority::{RuntimeCapability, RuntimeCapabilitySource};
    use rua_mcp::bundled_browser::{BundledBrowser, SERVER_ID};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspaces/agent");
    std::fs::create_dir_all(&workspace).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let cancel = tokio_util::sync::CancellationToken::new();
    let stop = cancel.clone();
    let fixture = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let mut request = [0u8; 4096];
                    let _ = socket.read(&mut request).await;
                    let body = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../rua-mcp/tests/fixtures/browser-form.html"));
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        }
    });
    let manager = rua_mcp::McpManagerImpl::new(rua_mcp::McpMobilePolicy::new(
        rua_mcp::PlatformProfile::Desktop,
    ))
    .with_bundled_browser(BundledBrowser {
        driver: std::env::var_os("RUA_TEST_AGENT_BROWSER")
            .expect("stage driver")
            .into(),
        data_dir: root.path().join("browser"),
        preferences_path: None,
        browser: RuntimeCapability {
            name: "chromium".into(),
            available: true,
            path: Some(
                std::env::var_os("RUA_TEST_CHROMIUM")
                    .expect("system Chromium required")
                    .into(),
            ),
            source: RuntimeCapabilitySource::Configured,
            version: None,
            diagnostics: vec![],
        },
    });
    #[cfg(unix)]
    let manager = if with_computer {
        manager.with_bundled_computer(desktop_protocol_fixture(root.path()))
    } else {
        manager
    };
    #[cfg(not(unix))]
    assert!(!with_computer, "Computer protocol fixture requires Unix");
    let manager = Arc::new(manager);
    manager.load_config(Default::default()).await.unwrap();
    let mut steps = vec![
        (
            "navigate",
            "agent_browser_open",
            serde_json::json!({"url":url}),
        ),
        (
            "read",
            "agent_browser_get_text",
            serde_json::json!({"selector":"body"}),
        ),
        ("observe", "agent_browser_snapshot", serde_json::json!({})),
        (
            "fill",
            "agent_browser_fill",
            serde_json::json!({"selector":"#message","text":"Chat test 731"}),
        ),
        (
            "submit",
            "agent_browser_click",
            serde_json::json!({"selector":"#submit"}),
        ),
        (
            "reobserve",
            "agent_browser_snapshot",
            serde_json::json!({"interactive":false}),
        ),
        (
            "scroll",
            "agent_browser_scroll",
            serde_json::json!({"direction":"down","amount":500}),
        ),
        (
            "result",
            "agent_browser_get_text",
            serde_json::json!({"selector":"#result"}),
        ),
        (
            "image",
            "agent_browser_screenshot",
            serde_json::json!({"selector":"#visual"}),
        ),
        ("close", "agent_browser_close", serde_json::json!({})),
    ];
    if with_computer {
        steps.extend([
            ("desktop-observe", "get_window_state", serde_json::json!({"pid":100,"window_id":200})),
            ("desktop-input", "interact", serde_json::json!({"pid":100,"window_id":200,"observation_id":"$latest-observation","actions":[
                {"tool":"type_text","arguments":{"text":"Chat test 731"}},
                {"tool":"press_key","arguments":{"key":"return"}}
            ]})),
        ]);
    }
    let tool_name = |name: &str| {
        if name.starts_with("agent_browser_") {
            format!("mcp__{SERVER_ID}__{name}")
        } else {
            format!("mcp__rua_computer__{name}")
        }
    };
    let mut rounds: Vec<_> = steps
        .iter()
        .map(|(id, name, args)| {
            vec![
                AgentEvent::ToolCall {
                    call: ToolCall::new(*id, tool_name(name), args.clone()),
                },
                AgentEvent::End {
                    finish_reason: FinishReason::ToolUse,
                    usage: None,
                    metadata: serde_json::Value::Null,
                },
            ]
        })
        .collect();
    rounds.push(text_round("Read complete"));
    let (runtime, requests) =
        runtime_with_model_capability(rounds, Arc::new(AgentToolkit::new()), Some(true)).await;
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            root.path().join("policy"),
        )),
        None,
    ));
    let runtime = runtime
        .with_rua_root(root.path().into())
        .with_security(security)
        .with_mcp_manager(manager.clone());
    let mut req = request(workspace.clone());
    req.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(rua_config::types::security::SecurityConfig::default()).unwrap(),
    );
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 16,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;
    req.metadata.insert(
        rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(running).unwrap(),
    );
    req.metadata
        .insert("turn_id".into(), serde_json::json!("desktop-chat-turn"));
    req.metadata
        .insert("device_id".into(), serde_json::json!("local-device"));
    req.metadata.insert(
        "security_chat_address".into(),
        serde_json::to_value(rua_core::ChatAddress::agent("agent", "max-iters-session")).unwrap(),
    );
    req.metadata.insert(
        rua_agent::runtime::CHAT_TOOL_ALLOWLIST_METADATA_KEY.into(),
        serde_json::json!(steps
            .iter()
            .map(|(_, name, _)| tool_name(name))
            .collect::<Vec<_>>()),
    );
    req.metadata
        .insert("chat_configuration_epoch".into(), serde_json::json!(1));
    let events: Vec<_> = runtime.stream(req).await.unwrap().collect().await;
    let results: Vec<_> = events
        .into_iter()
        .map(|event| event.unwrap())
        .filter_map(|event| match event {
            rua_core::event::RuntimeEvent::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), steps.len(), "{results:?}");
    assert!(results.iter().all(|result| !result.is_error), "{results:?}");
    assert_eq!(results[1].call_id, "read");
    assert!(results[1].text_content().contains("normal-chat-proof-912"));
    assert!(
        format!("{:?}", requests.lock().unwrap()).contains("normal-chat-proof-912"),
        "subsequent model request must receive the real page text"
    );
    let submitted = results
        .iter()
        .find(|result| result.call_id == "result")
        .unwrap();
    assert!(submitted
        .text_content()
        .contains("submissions=1 value=Chat test 731"));
    let screenshot = results
        .iter()
        .find(|result| result.call_id == "image")
        .unwrap();
    let image = screenshot.content.iter().find(|part| matches!(part, ContentPart::Image { media_type: Some(kind), url, .. } if kind == "image/png" && url.starts_with("data:image/png;base64,iVBOR"))).expect("real PNG image must reach the Chat tool event");
    assert!(
        requests.lock().unwrap().iter().any(|request| request
            .messages
            .iter()
            .any(|message| message.content.iter().any(|part| part == image || matches!(part, ContentPart::ToolResult { result } if result.content.contains(image))))),
        "same image must reach a subsequent model request"
    );
    if with_computer {
        let input = results
            .iter()
            .find(|result| result.call_id == "desktop-input")
            .unwrap();
        let observed = input.metadata["mcp_structured_content"]["observation"]["text"]
            .as_str()
            .unwrap_or("");
        assert_eq!(observed, "Chat test 731\n");
        let calls = std::fs::read_to_string(root.path().join("calls.jsonl")).unwrap();
        assert_eq!(
            calls
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|call| call["name"] == "press_key")
                .count(),
            1
        );
        let desktop_image = input
            .content
            .iter()
            .find(|part| matches!(part, ContentPart::Image { .. }))
            .unwrap();
        assert!(requests.lock().unwrap().iter().any(|request| request.messages.iter().any(|message|
            message.content.iter().any(|part| part == desktop_image || matches!(part, ContentPart::ToolResult {result} if result.content.contains(desktop_image))))));
    }
    assert!(manager.shutdown_all().await.unwrap().errors.is_empty());
    cancel.cancel();
    fixture.await.unwrap();
}

#[tokio::test]
async fn unavailable_bundled_browser_does_not_prevent_new_chat_configuration() {
    use mcp_manager::manager::McpManager;
    use rua_core::runtime_authority::{RuntimeCapability, RuntimeCapabilitySource};
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspaces/agent");
    std::fs::create_dir_all(&workspace).unwrap();
    let manager = Arc::new(
        rua_mcp::McpManagerImpl::new(rua_mcp::McpMobilePolicy::new(
            rua_mcp::PlatformProfile::Desktop,
        ))
        .with_bundled_browser(rua_mcp::bundled_browser::BundledBrowser {
            driver: root.path().join("missing-driver"),
            data_dir: root.path().join("browser"),
            preferences_path: None,
            browser: RuntimeCapability {
                name: "chromium".into(),
                available: false,
                path: None,
                version: None,
                source: RuntimeCapabilitySource::Unavailable,
                diagnostics: vec!["No browser installed".into()],
            },
        }),
    );
    manager.load_config(Default::default()).await.unwrap();
    let runtime = runtime_with_sequences(vec![text_round("ok")])
        .await
        .with_mcp_manager(manager.clone());
    runtime
        .capability_resolver()
        .configuration(&workspace, None, "console")
        .await
        .unwrap();
    assert!(manager.shutdown_all().await.unwrap().errors.is_empty());
}

#[cfg(unix)]
fn desktop_protocol_fixture(root: &std::path::Path) -> rua_mcp::bundled_computer::BundledComputer {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let driver = root.join("fixture-driver");
    rua_config::atomic_write(
        &driver,
        format!(
            "#!{}\n{}",
            String::from_utf8(python.stdout).unwrap().trim(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../rua-mcp/tests/fixtures/mock_cua.py"
            ))
        )
        .as_bytes(),
    )
    .unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o700)).unwrap();
    rua_config::atomic_write_json(
        &root.join("permissions.json"),
        &json!({"source":{"attribution":"host"},"accessibility":true,"screen_recording":true}),
    )
    .unwrap();
    let connection_file = root.join("connection.json");
    rua_config::atomic_write_json(&connection_file, &json!({"generation":uuid::Uuid::new_v4().to_string(),"socket_path":"fixture","driver_version":rua_mcp::bundled_computer::DRIVER_VERSION,"host_bundle_id":"test.rua"})).unwrap();
    rua_mcp::bundled_computer::BundledComputer {
        driver,
        connection_file,
    }
}

#[tokio::test]
#[cfg(unix)]
async fn bundled_computer_normal_chat_consumes_observation_and_projects_action_image() {
    use mcp_manager::manager::McpManager;
    use rua_mcp::bundled_computer::SERVER_ID;
    use serde_json::{json, Value};
    let root = tempfile::tempdir().unwrap();
    rua_config::atomic_write_json(&root.path().join("config.json"), &json!({})).unwrap();
    let workspace = root.path().join("workspaces/agent");
    std::fs::create_dir_all(&workspace).unwrap();
    let manager = Arc::new(
        rua_mcp::McpManagerImpl::new(rua_mcp::McpMobilePolicy::new(
            rua_mcp::PlatformProfile::Desktop,
        ))
        .with_bundled_computer(desktop_protocol_fixture(root.path())),
    );
    manager.load_config(Default::default()).await.unwrap();
    manager
        .connect_server(&mcp_manager::types::McpServerId::new(SERVER_ID))
        .await
        .unwrap();
    let steps = [
        (
            "observe",
            "get_window_state",
            json!({"pid":100,"window_id":200}),
        ),
        (
            "input",
            "interact",
            json!({"pid":100,"window_id":200,"observation_id":"$latest-observation","actions":[
                {"tool":"type_text","arguments":{"text":"Chat desktop proof 731"}},
                {"tool":"press_key","arguments":{"key":"return"}}
            ]}),
        ),
    ];
    let mut rounds = steps
        .iter()
        .map(|(id, name, args)| {
            vec![
                AgentEvent::ToolCall {
                    call: ToolCall::new(*id, format!("mcp__{SERVER_ID}__{name}"), args.clone()),
                },
                AgentEvent::End {
                    finish_reason: FinishReason::ToolUse,
                    usage: None,
                    metadata: Value::Null,
                },
            ]
        })
        .collect::<Vec<_>>();
    rounds.push(text_round("Verified the returned desktop observation"));
    let (runtime, requests) =
        runtime_with_model_capability(rounds, Arc::new(AgentToolkit::new()), Some(true)).await;
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            root.path().join("policy"),
        )),
        None,
    ));
    let runtime = runtime
        .with_rua_root(root.path().into())
        .with_security(security)
        .with_mcp_manager(manager.clone());
    let mut req = request(workspace);
    req.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(rua_config::types::security::SecurityConfig::default()).unwrap(),
    );
    req.metadata
        .insert("chat_configuration_epoch".into(), json!(1));
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 4,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;
    req.metadata.insert(
        rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(running).unwrap(),
    );
    req.metadata
        .insert("turn_id".into(), json!("desktop-chat-turn"));
    req.metadata
        .insert("device_id".into(), json!("local-device"));
    req.metadata.insert(
        "security_chat_address".into(),
        serde_json::to_value(rua_core::ChatAddress::agent("agent", "max-iters-session")).unwrap(),
    );
    req.metadata.insert(
        rua_agent::runtime::CHAT_TOOL_ALLOWLIST_METADATA_KEY.into(),
        json!(steps
            .iter()
            .map(|(_, n, _)| format!("mcp__{SERVER_ID}__{n}"))
            .collect::<Vec<_>>()),
    );
    let events = runtime.stream(req).await.unwrap().collect::<Vec<_>>().await;
    let results = events
        .into_iter()
        .map(Result::unwrap)
        .filter_map(|event| match event {
            rua_core::event::RuntimeEvent::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(results.iter().all(|r| !r.is_error), "{results:?}");
    let final_result = &results[1];
    assert!(final_result
        .text_content()
        .contains("Chat desktop proof 731"));
    let image = final_result.content.iter().find(|p|matches!(p,ContentPart::Image{url,..} if url.starts_with("data:image/png;base64,iVBOR"))).unwrap();
    {
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2]
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|p| p == image
                || matches!(p,ContentPart::ToolResult {result} if result.content.contains(image))));
        let calls = std::fs::read_to_string(root.path().join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .filter(|v| matches!(v["name"].as_str(), Some("type_text" | "press_key")))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), "Chat desktop proof 731".chars().count() + 1);
        assert_eq!(
            calls
                .iter()
                .filter_map(|call| call["args"]["text"].as_str())
                .collect::<String>(),
            "Chat desktop proof 731"
        );
        assert_eq!(calls[0]["args"]["session"], calls[1]["args"]["session"]);
    }
    assert!(manager.shutdown_all().await.unwrap().errors.is_empty());
}
