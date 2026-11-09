use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, Role, TokenUsage, ToolAnnotations,
    ToolCall, ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::{stream, StreamExt};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo, Provider, ProviderInfo};
use llm_multiplex::LlmError;
use react_agent::{
    AgentError, AgentHook, AgentToolkit, ModelCallProjector, ModelInputTokenEstimate,
    ModelInputTokenEstimateMethod, ToolExecutor, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
use rua_agent::projection_preparation::ContextProjectionProjector;
use rua_agent::runtime::ReactAgentRuntime;
use rua_agent::turn_persistence::TurnPersistenceHook;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::{message::ContentPart, token_estimate::estimate_serialized_tokens, RuntimeEvent};
use rua_provider::manager::ProviderManager;
use rua_session::{
    early_background_compaction_coordinator, CapabilityStateChange, CapabilityStateRuntimeNote,
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, RuntimeNote,
    RuntimeNotePayload, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;
use tokio::sync::Notify;

struct BoundaryProbeModel {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    second_call_started: Arc<Notify>,
    release_second_call: Arc<Notify>,
    first_tool_calls: Vec<ToolCall>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for BoundaryProbeModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: agent_types::ModelDialogueItem::text(Role::Assistant, "unused"),
            usage: None,
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: HashMap::new(),
        })
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.requests.lock().expect("requests").push(request);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let mut events = self
                .first_tool_calls
                .iter()
                .cloned()
                .map(|call| AgentEvent::ToolCall { call })
                .collect::<Vec<_>>();
            events.push(AgentEvent::End {
                finish_reason: FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            });
            return Box::pin(stream::iter(events.into_iter().map(Ok)));
        }

        self.second_call_started.notify_one();
        let release = Arc::clone(&self.release_second_call);
        Box::pin(
            stream::once(async move {
                release.notified().await;
                Ok(AgentEvent::ThinkingDelta {
                    delta: "considering".to_string(),
                    signature: None,
                    metadata: serde_json::Value::Null,
                })
            })
            .chain(stream::once(async {
                Ok(AgentEvent::TextDelta {
                    delta: "done".to_string(),
                    metadata: serde_json::Value::Null,
                })
            }))
            .chain(stream::once(async {
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: Some(TokenUsage::new(10, 2)),
                    metadata: serde_json::Value::Null,
                })
            })),
        )
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct BoundaryProbeProvider {
    info: ProviderInfo,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    second_call_started: Arc<Notify>,
    release_second_call: Arc<Notify>,
    first_tool_calls: Vec<ToolCall>,
    context_window: u64,
}

#[async_trait]
impl Provider for BoundaryProbeProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        model == "boundary-probe"
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(BoundaryProbeModel {
            calls: Arc::clone(&self.calls),
            requests: Arc::clone(&self.requests),
            second_call_started: Arc::clone(&self.second_call_started),
            release_second_call: Arc::clone(&self.release_second_call),
            first_tool_calls: self.first_tool_calls.clone(),
            info: ModelInfo {
                id: "boundary-probe".to_string(),
                name: "Boundary Probe".to_string(),
                context_window: Some(self.context_window),
                max_output_tokens: Some(1024),
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(Vec::new())
    }

    async fn check_connection(&self) -> Result<llm_multiplex::ConnectionStatus, LlmError> {
        Ok(llm_multiplex::ConnectionStatus::Connected)
    }
}

struct BlockingParallelTool {
    blocked_call_started: Arc<Notify>,
    release_blocked_call: Arc<Notify>,
}

struct LargeTool {
    result: String,
}

#[async_trait]
impl ToolExecutor for LargeTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, AgentError> {
        Ok(ToolResult::text(call.id.clone(), self.result.clone()))
    }
}

struct BlockingSummaryManager {
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}

struct SequencedSummaryManager {
    summaries: Mutex<VecDeque<String>>,
    calls: Arc<AtomicUsize>,
}

struct CountingReplicationHook {
    calls: Arc<AtomicUsize>,
    addresses: Arc<Mutex<Vec<rua_core::ChatAddress>>>,
}

#[async_trait]
impl rua_contract::agent::SessionReplicationHook for CountingReplicationHook {
    async fn on_session_state_changed(
        &self,
        chat_address: &rua_core::ChatAddress,
        state: &serde_json::Value,
    ) {
        if state.get("kind").and_then(serde_json::Value::as_str)
            == Some("summary_checkpoint_published")
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.addresses
                .lock()
                .expect("replication addresses")
                .push(chat_address.clone());
        }
    }
}

#[async_trait]
impl ContextManager for SequencedSummaryManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, rua_agent::AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, rua_agent::AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let summary = self
            .summaries
            .lock()
            .expect("summaries")
            .pop_front()
            .expect("configured summary");
        Ok(ContextSummaryResult { summary })
    }
}

#[async_trait]
impl ContextManager for BlockingSummaryManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, rua_agent::AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, rua_agent::AgentError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(ContextSummaryResult {
            summary: "The assistant called large_tool and received its result.".to_string(),
        })
    }
}

#[async_trait]
impl ToolExecutor for BlockingParallelTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, AgentError> {
        if call.id == "call-2" {
            self.blocked_call_started.notify_one();
            self.release_blocked_call.notified().await;
        }
        Ok(ToolResult::text(
            call.id.clone(),
            format!("result:{}", call.id),
        ))
    }
}

fn conversation_message(
    message_id: &str,
    role: &str,
    content: serde_json::Value,
) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-07-24T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn model_call_projection_uses_durable_message_order() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "durable-message-order";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Durable message order".to_string(),
            created_at: "2026-08-29T00:00:00Z".to_string(),
            last_activity_at: "2026-08-29T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    let mut aborted = conversation_message(
        "turn-aborted-turn-1",
        "user",
        json!([{"type": "text", "text": "<turn_aborted>stopped here</turn_aborted>"}]),
    );
    aborted
        .metadata
        .insert("model_visible_marker".to_string(), json!("turn_aborted"));
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-29T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-29T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-29T00:00:02Z".to_string()),
                    status: "interrupted".to_string(),
                },
                selected_variant_id: Some("turn-aborted-turn-1".to_string()),
                messages: vec![
                    conversation_message(
                        "user-1",
                        "user",
                        json!([{"type": "text", "text": "start work"}]),
                    ),
                    aborted,
                ],
            },
        )
        .await
        .expect("interrupted turn");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-29T00:00:03Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-2",
                    "user",
                    json!([{"type": "text", "text": "continue work"}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let running = Arc::new(rua_config::AgentsRunningConfig::default());
    let agent_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!("turn-2")),
            ("loaded_message_count".to_string(), json!(3)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: Some(workspace.path().to_path_buf()),
        working_dir: None,
    };
    let projector = ContextProjectionProjector::from_request(
        &agent_request,
        None,
        running,
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "test".to_string(),
        },
        Some(1_000_000),
        None,
    )
    .expect("projection projector");
    let request = ChatRequest::streaming(vec![
        agent_types::ModelDialogueItem::text(Role::User, "start work"),
        agent_types::ModelDialogueItem::text(Role::User, "continue work"),
        agent_types::ModelDialogueItem::text(
            Role::User,
            "<turn_aborted>stopped here</turn_aborted>",
        ),
    ]);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);

    let projected = projector.project(request, &event_tx).await.unwrap();

    assert_eq!(
        projected
            .messages
            .iter()
            .map(agent_types::ModelDialogueItem::text_content)
            .collect::<Vec<_>>(),
        vec![
            "start work",
            "<turn_aborted>stopped here</turn_aborted>",
            "continue work",
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_tool_messages_are_durable_before_next_model_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "model-call-boundary-chat";
    let turn_id = "turn-model-call-boundary";
    let capture_dir = tempfile::tempdir().expect("capture directory");
    let capture_address = rua_core::ChatAddress::agent("agent-1", chat_id);
    llm_multiplex::debug_capture::register_capture(
        capture_address.clone(),
        llm_multiplex::debug_capture::ActiveCapture {
            capture_id: uuid::Uuid::new_v4().to_string(),
            capture_path: capture_dir.path().to_path_buf(),
        },
    );
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Boundary persistence".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-1",
                    "user",
                    json!([{"type": "text", "text": "use the tool"}]),
                )],
            },
        )
        .await
        .expect("submitted user turn");

    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let second_call_started = Arc::new(Notify::new());
    let release_second_call = Arc::new(Notify::new());
    let blocked_call_started = Arc::new(Notify::new());
    let release_blocked_call = Arc::new(Notify::new());
    let first_tool_calls = vec![
        ToolCall::new(
            "call-1",
            "parallel_tool",
            serde_json::json!({"value": "first"}),
        ),
        ToolCall::new(
            "call-2",
            "parallel_tool",
            serde_json::json!({"value": "second"}),
        ),
    ];
    let provider_manager = ProviderManager::new();
    provider_manager.register(Arc::new(BoundaryProbeProvider {
        info: ProviderInfo {
            id: "boundary-provider".to_string(),
            name: "Boundary Provider".to_string(),
            provider_type: Some("mock".to_string()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        calls: Arc::clone(&calls),
        requests,
        second_call_started: Arc::clone(&second_call_started),
        release_second_call: Arc::clone(&release_second_call),
        first_tool_calls,
        context_window: 4096,
    }));
    provider_manager
        .store_default_model(rua_provider::config_loader::DefaultModel {
            provider_id: "boundary-provider".to_string(),
            model: "boundary-probe".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;

    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "parallel_tool".to_string(),
            description: "returns stable parallel test results".to_string(),
            parameters: json!({"type": "object"}),
            strict: None,
            annotations: ToolAnnotations {
                read_only: Some(true),
                destructive: Some(false),
                idempotent: Some(true),
                requires_approval: Some(false),
                ..Default::default()
            },
            metadata: Default::default(),
        },
        Arc::new(BlockingParallelTool {
            blocked_call_started: Arc::clone(&blocked_call_started),
            release_blocked_call: Arc::clone(&release_blocked_call),
        }),
    );
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 4,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = false;
    let runtime = ReactAgentRuntime::new(Arc::new(provider_manager))
        .with_running_config(Arc::new(running))
        .with_toolkit(Arc::new(toolkit));

    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "use the tool".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("security_chat_address".into(), json!(capture_address)),
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
            ("device_id".to_string(), json!("device-1")),
            ("authority_device_id".to_string(), json!("device-1")),
            ("authority_epoch".to_string(), json!(1)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };

    let mut events = runtime.stream(request).await.expect("runtime stream");
    let drain = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(event) = events.next().await {
            collected.push(event.expect("runtime event"));
        }
        collected
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        blocked_call_started.notified(),
    )
    .await
    .expect("blocked parallel tool");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the next model call must not start while one parallel tool is incomplete"
    );
    let partial_messages = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("partial turn messages");
    assert_eq!(
        partial_messages.len(),
        3,
        "each completed Tool Use Message is durable before Tool Results finish"
    );
    assert_eq!(partial_messages[1].role, "assistant");
    assert_eq!(partial_messages[1].content[0]["id"], "call-1");
    assert_eq!(partial_messages[2].role, "assistant");
    assert_eq!(partial_messages[2].content[0]["id"], "call-2");

    release_blocked_call.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        second_call_started.notified(),
    )
    .await
    .expect("second model call");

    let messages = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("turn messages");
    assert_eq!(
        messages.len(),
        5,
        "submitted user, assistant tool request, and both tool results must be durable before call two"
    );
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[3].role, "tool");
    assert_eq!(messages[4].role, "tool");

    let turn = store
        .read_turn_range(chat_id, 1, 1)
        .await
        .expect("turn index")
        .pop()
        .expect("active turn");
    assert_eq!(turn.lifecycle.status, "in_progress");
    assert!(turn.metadata.get("model_call_boundaries").is_none());

    release_second_call.notify_one();
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), drain)
        .await
        .expect("runtime completion")
        .expect("drain task");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let completed_messages = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("completed turn messages");
    assert_eq!(
        completed_messages.len(),
        7,
        "reasoning, Tool Uses, Tool Results, and final text must be distinct Messages"
    );
    let live_reasoning_message_id = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ThinkingDelta {
                message_id: Some(message_id),
                ..
            } => Some(message_id.to_string()),
            _ => None,
        })
        .expect("canonical live reasoning message id");
    let live_text_message_id = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::TextDelta {
                message_id: Some(message_id),
                ..
            } => Some(message_id.to_string()),
            _ => None,
        })
        .expect("canonical live text message id");
    let reasoning = completed_messages
        .iter()
        .find(|message| message.content[0]["thinking"] == "considering")
        .expect("persisted reasoning message");
    assert_eq!(reasoning.message_id, live_reasoning_message_id);
    let final_text = completed_messages
        .iter()
        .find(|message| message.content[0]["text"] == "done")
        .expect("persisted final assistant text");
    assert_eq!(final_text.role, "assistant");
    assert_eq!(final_text.message_id, live_text_message_id);
    for call_id in ["call-1", "call-2"] {
        let tool_use_messages = completed_messages
            .iter()
            .filter(|message| {
                message.content[0]["type"] == "tool_use"
                    && message.content[0]["id"].as_str() == Some(call_id)
            })
            .collect::<Vec<_>>();
        let tool_result_messages = completed_messages
            .iter()
            .filter(|message| {
                message.content[0]["type"] == "tool_result"
                    && message.content[0]["id"].as_str() == Some(call_id)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_use_messages.len(),
            1,
            "{call_id} must have one durable Tool Use Message"
        );
        assert_eq!(
            tool_result_messages.len(),
            1,
            "{call_id} must have one durable Tool Result Message"
        );
        assert_eq!(
            tool_use_messages[0].message_id,
            agent_types::conversation_message_identity::tool_use_message_id(call_id)
        );
        assert_eq!(
            tool_result_messages[0].message_id,
            agent_types::conversation_message_identity::tool_result_message_id(call_id)
        );
    }
    assert_eq!(
        final_text.metadata["rua_token_usage"]["usage"]["total_tokens"],
        12
    );
    assert_eq!(
        final_text.metadata["rua_token_usage"]["last_call_usage"]["prompt_tokens"],
        10
    );
    assert_eq!(
        final_text.metadata["rua_token_usage"]["max_input_length"],
        4_096
    );
    let input_estimate: ModelInputTokenEstimate = serde_json::from_value(
        final_text.metadata[MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY].clone(),
    )
    .expect("persisted model input estimate");
    assert!(input_estimate.fast_estimate_tokens > 0);
    assert_eq!(
        input_estimate.calibrated_estimate_tokens,
        input_estimate.fast_estimate_tokens
    );
    assert_eq!(input_estimate.method, ModelInputTokenEstimateMethod::Fast);

    llm_multiplex::debug_capture::unregister_capture(&capture_address);
    llm_multiplex::debug_capture::flush_capture(
        capture_dir.path(),
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    let mut persistence_traces = Vec::<serde_json::Value>::new();
    for call in std::fs::read_dir(capture_dir.path().join("calls")).unwrap() {
        for file in std::fs::read_dir(call.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("persistence-timing-")
            {
                persistence_traces
                    .push(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap());
            }
        }
    }
    assert_eq!(
        persistence_traces.len(),
        3,
        "assistant, Tool Results, and final response saves"
    );
    for trace in &persistence_traces {
        assert_eq!(trace["outcome"], "completed");
        assert!(trace["data"]["parent_trace_id"].is_string());
        let marks = &trace["milestones_ms"];
        assert!(
            marks["message_conversion_finished"].as_f64().unwrap()
                >= marks["message_conversion_started"].as_f64().unwrap()
        );
        assert!(
            marks["turn_writer_finished"].as_f64().unwrap()
                >= marks["turn_writer_started"].as_f64().unwrap()
        );
        assert!(
            marks["runtime_notes_flush_finished"].as_f64().unwrap()
                >= marks["runtime_notes_flush_started"].as_f64().unwrap()
        );
    }
    let tools_save = persistence_traces
        .iter()
        .find(|trace| trace["data"]["message_role"] == "Tool")
        .unwrap();
    assert_eq!(tools_save["data"]["message_count"], 2);
}

#[tokio::test]
async fn display_image_is_archived_before_the_assistant_message_is_persisted() {
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("preview.png"), b"image bytes").unwrap();
    let store = WorkspaceSessionStore::new(workspace.path());
    let chat_id = "image-archive";
    let turn_id = "turn-image";
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.into(),
            agent_id: "agent-1".into(),
            title: "image".into(),
            created_at: "2026-07-24T00:00:00Z".into(),
            last_activity_at: "2026-07-24T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.into(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-image",
                    "user",
                    json!([{"type":"text","text":"show image"}]),
                )],
            },
        )
        .await
        .unwrap();
    let request = AgentRequest {
        agent_id: "agent-1".into(),
        session_id: chat_id.into(),
        user_id: "user-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".into(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().into()),
        conversation_store_dir: Some(workspace.path().into()),
        working_dir: Some(source.path().into()),
        metadata: HashMap::from([
            ("turn_id".into(), json!(turn_id)),
            ("loaded_message_count".into(), json!(1)),
            ("device_id".into(), json!("desktop")),
            ("authority_device_id".into(), json!("desktop")),
        ]),
    };
    let hook = TurnPersistenceHook::from_request(&request).unwrap();
    let mut image_message =
        agent_types::ModelDialogueItem::text(Role::Assistant, "![preview](preview.png)");
    image_message.metadata.insert(
        agent_types::conversation_message_identity::TEXT_MESSAGE_ID_METADATA_KEY.into(),
        json!("assistant-image"),
    );
    hook.after_model_call(&[image_message], 1, &TokenUsage::default(), None, true)
        .await
        .unwrap();
    let persisted = store.read_turn_messages(chat_id, turn_id).await.unwrap();
    let assistant = persisted
        .iter()
        .find(|message| message.role == "assistant")
        .unwrap();
    let reference = assistant.metadata["display_media"][0]["media_ref"]
        .as_str()
        .unwrap();
    assert!(assistant.content.to_string().contains(reference));
    assert!(!assistant.content.to_string().contains("preview.png"));
    assert_eq!(
        assistant.metadata["display_media"][0]["origin"]["device_id"],
        "desktop"
    );
    assert_eq!(
        std::fs::read(workspace.path().join(reference)).unwrap(),
        b"image bytes"
    );
}

#[tokio::test]
async fn compacted_projection_still_persists_the_latest_messages() {
    let workspace = tempfile::tempdir().expect("workspace");
    let agent_workspace = tempfile::tempdir().expect("agent workspace");
    let chat_id = "compacted-boundary-persistence";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Compacted boundary persistence".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "continue after compaction"}]),
                )],
            },
        )
        .await
        .expect("active turn");
    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(20)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(agent_workspace.path().to_path_buf()),
        conversation_store_dir: Some(workspace.path().to_path_buf()),
        working_dir: None,
    };
    let hook = TurnPersistenceHook::from_request(&request).expect("turn persistence hook");
    let call = ToolCall::new("call-after-compact", "large_tool", json!({}));
    let messages = vec![
        agent_types::ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse { call: call.clone() }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        agent_types::ModelDialogueItem {
            id: None,
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                result: ToolResult::text(call.id, "complete result"),
            }],
            name: Some("large_tool".to_string()),
            created_at: None,
            metadata: serde_json::Map::new(),
        },
    ];

    hook.after_model_call(
        &messages,
        1,
        &agent_types::TokenUsage::default(),
        None,
        false,
    )
    .await
    .expect("persist compacted output");

    let persisted = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("turn messages");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[1].role, "assistant");
    assert_eq!(persisted[2].role, "tool");
    let turn = store
        .read_turn_range(chat_id, 1, 1)
        .await
        .expect("turn index")
        .pop()
        .expect("turn");
    assert!(turn.metadata.get("model_call_boundaries").is_none());
    assert!(
        WorkspaceSessionStore::new(agent_workspace.path())
            .load_chat_header(chat_id)
            .await
            .expect("agent workspace chat read")
            .is_none(),
        "Turn persistence must not write into the Agent workspace"
    );
}

#[tokio::test]
async fn regenerated_response_is_appended_incrementally_as_one_new_variant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "regenerate-boundary-chat";
    let turn_id = "turn-regenerate-boundary";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Regenerate persistence".to_string(),
            created_at: "2026-08-04T00:00:00Z".to_string(),
            last_activity_at: "2026-08-04T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-04T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-04T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-04T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-original".to_string()),
                messages: vec![
                    conversation_message(
                        "user-original",
                        "user",
                        json!([{"type": "text", "text": "try again"}]),
                    ),
                    conversation_message(
                        "assistant-original",
                        "assistant",
                        json!([{"type": "text", "text": "original"}]),
                    ),
                ],
            },
        )
        .await
        .expect("original turn");

    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("regenerate_turn_id".to_string(), json!(turn_id)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = TurnPersistenceHook::from_request(&request).expect("turn persistence hook");
    let call = ToolCall::new("call-regenerate", "read", json!({"path": "README.md"}));
    hook.after_model_call(
        &[
            agent_types::ModelDialogueItem {
                id: Some("assistant-regenerated-tool".to_string()),
                role: Role::Assistant,
                content: vec![ContentPart::ToolUse { call: call.clone() }],
                name: None,
                created_at: None,
                metadata: serde_json::Map::new(),
            },
            agent_types::ModelDialogueItem {
                id: Some("tool-regenerated".to_string()),
                role: Role::Tool,
                content: vec![ContentPart::ToolResult {
                    result: ToolResult::text(call.id, "contents"),
                }],
                name: Some("read".to_string()),
                created_at: None,
                metadata: serde_json::Map::new(),
            },
        ],
        0,
        &TokenUsage::default(),
        None,
        false,
    )
    .await
    .expect("first regenerated group");

    let after_first = store
        .read_turn_range(chat_id, 1, 1)
        .await
        .expect("turn index")
        .pop()
        .expect("turn");
    assert_eq!(after_first.lifecycle.status, "in_progress");
    assert_eq!(after_first.assistant_response_variants.len(), 2);

    hook.after_model_call(
        &[agent_types::ModelDialogueItem {
            id: Some("assistant-regenerated-final".to_string()),
            role: Role::Assistant,
            content: vec![ContentPart::Text {
                text: "regenerated".to_string(),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::from_iter([(
                agent_types::conversation_message_identity::TEXT_MESSAGE_ID_METADATA_KEY
                    .to_string(),
                json!("assistant-regenerated-final"),
            )]),
        }],
        1,
        &TokenUsage::new(20, 4),
        Some(&TokenUsage::new(10, 2)),
        true,
    )
    .await
    .expect("final regenerated group");

    let messages = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("turn messages");
    assert_eq!(messages.len(), 5);
    let turn = store
        .read_turn_range(chat_id, 1, 1)
        .await
        .expect("turn index")
        .pop()
        .expect("turn");
    assert_eq!(turn.assistant_response_variants.len(), 2);
    assert_eq!(
        turn.assistant_response_variants[1]
            .assistant_message_sequences
            .len(),
        2
    );
    assert_eq!(
        messages[4].metadata["rua_token_usage"]["usage"]["total_tokens"],
        24
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_tool_result_blocks_next_model_call_until_checkpoint_is_published() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "mid-turn-compaction-chat";
    let turn_id = "turn-mid-turn-compaction";
    let original_user = "inspect the complete tool result";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Mid-turn compaction".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-1",
                    "user",
                    json!([{"type": "text", "text": original_user}]),
                )],
            },
        )
        .await
        .expect("submitted user turn");

    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let second_call_started = Arc::new(Notify::new());
    let release_second_call = Arc::new(Notify::new());
    let summary_started = Arc::new(Notify::new());
    let release_summary = Arc::new(Notify::new());
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let provider_manager = ProviderManager::new();
    provider_manager.register(Arc::new(BoundaryProbeProvider {
        info: ProviderInfo {
            id: "boundary-provider".to_string(),
            name: "Boundary Provider".to_string(),
            provider_type: Some("mock".to_string()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        calls: Arc::clone(&calls),
        requests: Arc::clone(&requests),
        second_call_started: Arc::clone(&second_call_started),
        release_second_call: Arc::clone(&release_second_call),
        first_tool_calls: vec![ToolCall::new(
            "call-large",
            "large_tool",
            serde_json::json!({}),
        )],
        context_window: 4096,
    }));
    provider_manager
        .store_default_model(rua_provider::config_loader::DefaultModel {
            provider_id: "boundary-provider".to_string(),
            model: "boundary-probe".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;

    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "large_tool".to_string(),
            description: "returns a deliberately large result".to_string(),
            parameters: json!({"type": "object"}),
            strict: None,
            annotations: ToolAnnotations {
                read_only: Some(true),
                destructive: Some(false),
                idempotent: Some(true),
                requires_approval: Some(false),
                ..Default::default()
            },
            metadata: Default::default(),
        },
        Arc::new(LargeTool {
            result: "x".repeat(16_384),
        }),
    );
    let mut running = rua_config::AgentsRunningConfig {
        max_iters: 4,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.5;
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.recent_n = 0;
    running.tool_result_compact.old_max_bytes = 12_000;
    let early_compaction_lease = early_background_compaction_coordinator()
        .start_job(format!("{}::{chat_id}", workspace.path().display()));
    let runtime = ReactAgentRuntime::new(Arc::new(provider_manager))
        .with_running_config(Arc::new(running))
        .with_context_manager(Arc::new(BlockingSummaryManager {
            started: Arc::clone(&summary_started),
            release: Arc::clone(&release_summary),
            calls: Arc::clone(&summary_calls),
        }))
        .with_toolkit(Arc::new(toolkit));

    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: original_user.to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
            ("device_id".to_string(), json!("device-1")),
            ("authority_device_id".to_string(), json!("device-1")),
            ("authority_epoch".to_string(), json!(1)),
            ("agent_collaboration_mode".to_string(), json!("execute")),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };

    let mut stream = runtime.stream(request).await.expect("runtime stream");
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let drain = tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            event_tx.send(event.expect("runtime event")).expect("event");
        }
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        summary_started.notified(),
    )
    .await
    .expect("blocking summary started");
    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        early_compaction_lease.cancelled(),
    )
    .await
    .expect("blocking compaction must cancel early background compaction");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the second provider call must wait for blocking compaction"
    );
    let mut saw_started = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            RuntimeEvent::ContextCompaction { ref status, .. } if status == "started"
        ) {
            saw_started = true;
        }
    }
    assert!(saw_started, "started must be visible while summary blocks");

    release_summary.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        second_call_started.notified(),
    )
    .await
    .expect("second model call");
    let checkpoint = store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read")
        .expect("active checkpoint");
    assert_eq!(checkpoint.frontier.end_sequence, 3);

    let second_request = requests
        .lock()
        .expect("requests")
        .get(1)
        .cloned()
        .expect("second request");
    assert_eq!(
        second_request
            .messages
            .iter()
            .filter(|message| message.text_content().contains(original_user))
            .count(),
        1,
        "the original user request must survive projection rebuild exactly once"
    );
    assert!(
        second_request
            .messages
            .iter()
            .all(|message| !message.text_content().contains(&"x".repeat(512))),
        "the large raw tool result must be replaced by the checkpoint projection"
    );
    let collaboration_modes = second_request
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
    assert_eq!(collaboration_modes.len(), 1);
    assert_eq!(collaboration_modes[0].role, Role::Developer);
    assert!(collaboration_modes[0].text_content().contains("Execute"));
    release_second_call.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), drain)
        .await
        .expect("runtime completion")
        .expect("drain task");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn blocking_compaction_bounds_summary_input_and_preserves_recent_tail() {
    let workspace = tempfile::tempdir().expect("workspace");
    let agent_workspace = tempfile::tempdir().expect("agent workspace");
    let chat_id = "iterative-projection-fit";
    let active_turn_id = "turn-active";
    let active_user = "continue from the exact active request";
    let history_text_len = 4000;
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Iterative projection fit".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    for ordinal in 1..=2 {
        let assistant_id = format!("assistant-{ordinal}");
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{ordinal}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: format!("2026-07-24T00:00:0{ordinal}Z"),
                        assistant_started_at: Some(format!("2026-07-24T00:00:0{ordinal}Z")),
                        assistant_completed_at: Some(format!("2026-07-24T00:00:0{ordinal}Z")),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: Some(assistant_id.clone()),
                    messages: vec![
                        conversation_message(
                            &format!("user-{ordinal}"),
                            "user",
                            json!([{"type": "text", "text": "u".repeat(history_text_len)}]),
                        ),
                        conversation_message(
                            &assistant_id,
                            "assistant",
                            json!([{"type": "text", "text": "a".repeat(history_text_len)}]),
                        ),
                    ],
                },
            )
            .await
            .expect("complete turn");
    }
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: active_turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:03Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": active_user}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let summary_calls = Arc::new(AtomicUsize::new(0));
    let replication_calls = Arc::new(AtomicUsize::new(0));
    let replication_addresses = Arc::new(Mutex::new(Vec::new()));
    let context_manager: Arc<dyn ContextManager> = Arc::new(SequencedSummaryManager {
        summaries: Mutex::new(VecDeque::from(["bounded summary".to_string()])),
        calls: Arc::clone(&summary_calls),
    });
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.5;
    running.context_compact.context_reserve_ratio = 0.05;
    let project_chat_address = rua_core::ChatAddress::project("project-1", chat_id);
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(active_turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
            (
                rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.to_string(),
                serde_json::to_value(&project_chat_address).expect("project chat address"),
            ),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(agent_workspace.path().to_path_buf()),
        conversation_store_dir: Some(workspace.path().to_path_buf()),
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(context_manager),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "iterative-fit".to_string(),
        },
        Some(8_000),
        Some(Arc::new(CountingReplicationHook {
            calls: Arc::clone(&replication_calls),
            addresses: Arc::clone(&replication_addresses),
        })),
    )
    .expect("projection hook");
    let mut messages = Vec::new();
    for _ in 1..=2 {
        messages.push(agent_types::ModelDialogueItem::text(
            Role::User,
            "u".repeat(history_text_len),
        ));
        messages.push(agent_types::ModelDialogueItem::text(
            Role::Assistant,
            "a".repeat(history_text_len),
        ));
    }
    messages.push(agent_types::ModelDialogueItem::text(
        Role::User,
        active_user,
    ));
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);

    let prepared = hook
        .project(ChatRequest::streaming(messages), &event_tx)
        .await
        .expect("iterative projection preparation");

    assert_eq!(summary_calls.load(Ordering::SeqCst), 1);
    assert_eq!(replication_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *replication_addresses.lock().expect("replication addresses"),
        vec![project_chat_address]
    );
    let checkpoint = store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read");
    assert!(
        WorkspaceSessionStore::new(agent_workspace.path())
            .load_active_summary_checkpoint(chat_id)
            .await
            .expect("agent workspace checkpoint read")
            .is_none(),
        "projection state must not be written into the Agent workspace"
    );
    assert_eq!(
        checkpoint.expect("active checkpoint").frontier.end_sequence,
        3
    );
    assert_eq!(
        prepared
            .messages
            .iter()
            .filter(|message| message.text_content().contains(active_user))
            .count(),
        1
    );
    assert_eq!(
        prepared
            .messages
            .iter()
            .filter(|message| message
                .text_content()
                .contains(&"u".repeat(history_text_len)))
            .count(),
        1,
        "the oldest turn must be folded while the recent tail stays visible"
    );
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "started"
    ));
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "completed"
    ));
}

#[tokio::test]
async fn oversized_request_without_a_safe_frontier_fails_before_model_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "no-safe-frontier";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "No safe frontier".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "x".repeat(4_000)}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.5;
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(Arc::new(SequencedSummaryManager {
            summaries: Mutex::new(VecDeque::new()),
            calls: Arc::clone(&summary_calls),
        })),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "fail-closed".to_string(),
        },
        Some(1_000),
        None,
    )
    .expect("projection hook");
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);

    let error = hook
        .project(
            ChatRequest::streaming(vec![agent_types::ModelDialogueItem::text(
                Role::User,
                "x".repeat(4_000),
            )]),
            &event_tx,
        )
        .await
        .expect_err("oversized request must fail closed");

    assert!(error
        .to_string()
        .contains("context recovery made no progress"));
    assert_eq!(summary_calls.load(Ordering::SeqCst), 0);
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read")
        .is_none());
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "started"
    ));
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "failed"
    ));
}

#[tokio::test]
async fn emergency_projection_uses_the_latest_durable_tail_when_no_frontier_exists() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "emergency-tail";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Emergency tail".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    let latest = "<capability_state>";
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    conversation_message(
                        "user-active",
                        "user",
                        json!([{"type": "text", "text": "old durable message"}]),
                    ),
                    RuntimeNote {
                        runtime_note_id: "runtime-note".to_string(),
                        created_at: "2026-07-24T00:00:01Z".to_string(),
                        payload: RuntimeNotePayload::CapabilityState(CapabilityStateRuntimeNote {
                            previous_generation: 1,
                            current_generation: 2,
                            changes: vec![CapabilityStateChange {
                                capability_type: "mcp".to_string(),
                                name: "desktop".to_string(),
                                state: "unavailable".to_string(),
                                reason: Some("desktop command is unavailable".to_string()),
                            }],
                        }),
                    }
                    .into_new_conversation_message()
                    .expect("runtime note message"),
                ],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.5;
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        None,
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "emergency-tail".to_string(),
        },
        Some(1_000),
        None,
    )
    .expect("projection hook");
    let request = ChatRequest::streaming(vec![
        agent_types::ModelDialogueItem::text(Role::User, "x".repeat(4_000)),
        agent_types::ModelDialogueItem::text(Role::System, latest),
    ]);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);

    let prepared = hook
        .project(request, &event_tx)
        .await
        .expect("emergency tail should fit");

    let prepared_text = prepared
        .messages
        .iter()
        .map(agent_types::ModelDialogueItem::text_content)
        .collect::<Vec<_>>();
    assert!(
        prepared_text.iter().any(|message| message.contains(latest)),
        "prepared emergency messages: {prepared_text:#?}"
    );
    assert!(prepared
        .messages
        .iter()
        .all(|message| !message.text_content().contains(&"x".repeat(200))));
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read")
        .is_none());
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn input_below_blocking_threshold_reaches_model_without_compaction() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "below-threshold-first-call";
    let turn_id = "turn-active";
    let user_input = "x".repeat(3_500);
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Below threshold".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": user_input.clone()}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig {
        max_input_length: 1_000,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.95;
    let estimate_divisor = running.context_compact.token_count_estimate_divisor;
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(Arc::new(SequencedSummaryManager {
            summaries: Mutex::new(VecDeque::new()),
            calls: Arc::clone(&summary_calls),
        })),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "below-threshold".to_string(),
        },
        Some(1_000),
        None,
    )
    .expect("projection hook");
    let request = ChatRequest::streaming(vec![agent_types::ModelDialogueItem::text(
        Role::User,
        &user_input,
    )]);
    let input_tokens = estimate_serialized_tokens(&request.messages, estimate_divisor);
    assert!(
        (900..950).contains(&input_tokens),
        "fixture must remain below the 95% blocking threshold, got {input_tokens}"
    );
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);

    let prepared = hook
        .project(request.clone(), &event_tx)
        .await
        .expect("first model call");

    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].text_content(), user_input);
    assert_eq!(summary_calls.load(Ordering::SeqCst), 0);
    assert!(event_rx.try_recv().is_err());
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read")
        .is_none());
}

#[tokio::test]
async fn internal_message_metadata_does_not_consume_model_context_budget() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "metadata-does-not-consume-context";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Internal metadata".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "small visible input"}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig {
        max_input_length: 200,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.95;
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(Arc::new(SequencedSummaryManager {
            summaries: Mutex::new(VecDeque::new()),
            calls: Arc::clone(&summary_calls),
        })),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "metadata-budget".to_string(),
        },
        Some(200),
        None,
    )
    .expect("projection hook");
    let mut message = agent_types::ModelDialogueItem::text(Role::User, "small visible input");
    message
        .metadata
        .insert("tool_output".to_string(), json!("x".repeat(20_000)));
    let request = ChatRequest::streaming(vec![message]);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);

    let prepared = hook
        .project(request, &event_tx)
        .await
        .expect("internal metadata must not trip the model context guard");

    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].text_content(), "small visible input");
    assert_eq!(summary_calls.load(Ordering::SeqCst), 0);
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn projection_tool_truncation_is_accounted_before_checkpoint_planning() {
    assert_projection_tool_truncation_budget(1, false).await;
}

#[tokio::test]
async fn projection_tool_truncation_batch_boundary_is_accounted_before_checkpoint_planning() {
    assert_projection_tool_truncation_budget(4, false).await;
}

#[tokio::test]
async fn projection_tool_truncation_waiting_for_batch_does_not_bypass_context_limit() {
    assert_projection_tool_truncation_budget(10, true).await;
}

async fn assert_projection_tool_truncation_budget(batch_size: u32, expect_overflow: bool) {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "projection-tool-truncation-fit";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Projection tool truncation fit".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "inspect"}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = !expect_overflow;
    running.context_compact.blocking_compact_ratio = 0.5;
    running.tool_result_compact.projection_truncation_enabled = true;
    running.tool_result_compact.projection_truncation_batch_size = batch_size;
    running.tool_result_compact.recent_n = 1;
    running.tool_result_compact.old_max_bytes = 32;
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(Arc::new(SequencedSummaryManager {
            summaries: Mutex::new(VecDeque::new()),
            calls: Arc::clone(&summary_calls),
        })),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "projection-tool-truncation".to_string(),
        },
        Some(2_000),
        None,
    )
    .expect("projection hook");

    let mut messages = vec![agent_types::ModelDialogueItem::text(Role::User, "inspect")];
    for index in 1..=4 {
        messages.push(agent_types::ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse {
                call: ToolCall::new(
                    format!("call-{index}"),
                    "bash",
                    json!({"command": format!("echo {index}")}),
                ),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        });
        let output = if index == 4 {
            "recent".to_string()
        } else {
            format!("old-{index}-{}", "x".repeat(4_000))
        };
        messages.push(agent_types::ModelDialogueItem {
            id: None,
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                result: ToolResult::text(format!("call-{index}"), output),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        });
    }
    store
        .append_messages_to_turn(
            chat_id,
            turn_id,
            TurnLifecycle {
                user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                assistant_completed_at: None,
                status: "in_progress".to_string(),
            },
            None,
            messages[1..]
                .iter()
                .enumerate()
                .map(|(index, message)| NewConversationMessage {
                    message_id: format!("persisted-{index}"),
                    role: match message.role {
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                        _ => "system",
                    }
                    .to_string(),
                    created_at: "2026-07-24T00:00:01Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::to_value(&message.content).unwrap(),
                    provider: None,
                    tool: None,
                    metadata: message.metadata.clone(),
                })
                .collect(),
        )
        .await
        .expect("persist tool rounds");
    let request = ChatRequest::streaming(messages);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);

    let prepared = hook.project(request.clone(), &event_tx).await;
    if expect_overflow {
        let error = prepared.expect_err("unshortened input must still respect the context limit");
        assert!(error
            .to_string()
            .contains("final model input exceeds max_input_length"));
    } else {
        let prepared = prepared.expect("truncated projection should fit without checkpoint");
        let tool_outputs = prepared
            .messages
            .iter()
            .filter_map(|message| message.tool_results().first().cloned())
            .map(|result| result.text_content())
            .collect::<Vec<_>>();
        assert_eq!(tool_outputs.len(), 4);
        assert!(tool_outputs[..3]
            .iter()
            .all(|output| !output.is_empty() && output.len() <= 32));
        assert_eq!(tool_outputs[3], "recent");
    }
    assert_eq!(summary_calls.load(Ordering::SeqCst), 0);
    assert!(event_rx.try_recv().is_err());
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .expect("checkpoint read")
        .is_none());
}

#[tokio::test]
async fn model_visible_message_envelope_is_measured_before_provider_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "message-envelope-fit";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Message envelope fit".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-complete".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-24T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-complete".to_string()),
                messages: vec![
                    conversation_message(
                        "user-complete",
                        "user",
                        json!([{"type": "text", "text": "small user message"}]),
                    ),
                    conversation_message(
                        "assistant-complete",
                        "assistant",
                        json!([{"type": "text", "text": "small assistant message"}]),
                    ),
                ],
            },
        )
        .await
        .expect("complete turn");

    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = true;
    running.context_compact.blocking_compact_ratio = 0.5;
    let summary_calls = Arc::new(AtomicUsize::new(0));
    let hook_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!("turn-active")),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let hook = ContextProjectionProjector::from_request(
        &hook_request,
        Some(Arc::new(SequencedSummaryManager {
            summaries: Mutex::new(VecDeque::from(["bounded summary".to_string()])),
            calls: Arc::clone(&summary_calls),
        })),
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "message-envelope-fit".to_string(),
        },
        Some(1_000),
        None,
    )
    .expect("projection hook");
    let mut system_message = agent_types::ModelDialogueItem::text(Role::Developer, "small rules");
    system_message.name = Some("n".repeat(4_000));
    system_message.metadata.insert(
        react_agent::MODEL_CALL_CONTEXT_SECTION_METADATA_KEY.to_string(),
        serde_json::json!("agent_prompt_files"),
    );
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);

    let error = hook
        .project(ChatRequest::streaming(vec![system_message]), &event_tx)
        .await
        .expect_err("the oversized model-visible message envelope must fail closed");

    assert!(
        error
            .to_string()
            .contains("context recovery made no progress"),
        "unexpected error: {error}"
    );
    assert_eq!(
        summary_calls.load(Ordering::SeqCst),
        0,
        "oversized static context cannot be reduced by summarizing conversation history"
    );
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "started"
    ));
    assert!(matches!(
        event_rx.recv().await,
        Some(AgentEvent::ContextCompaction { status, .. }) if status == "failed"
    ));
}

#[tokio::test]
async fn projected_model_call_reaggregates_split_tool_use_records() {
    use agent_types::conversation_message_identity::{
        reasoning_message_id, text_message_id, MODEL_DIALOGUE_ITEM_ID_METADATA_KEY,
        REASONING_MESSAGE_ID_METADATA_KEY, TEXT_MESSAGE_ID_METADATA_KEY,
    };

    let workspace = tempfile::tempdir().expect("workspace");
    let agent_workspace = tempfile::tempdir().expect("agent workspace");
    let chat_id = "reaggregate-split-tool-use";
    let turn_id = "turn-active";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Re-aggregate split Tool Use".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-active",
                    "user",
                    json!([{"type": "text", "text": "find the duplicate device logic"}]),
                )],
            },
        )
        .await
        .expect("active turn");

    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            ("loaded_message_count".to_string(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(agent_workspace.path().to_path_buf()),
        conversation_store_dir: Some(workspace.path().to_path_buf()),
        working_dir: None,
    };

    let hook = TurnPersistenceHook::from_request(&request).expect("turn persistence hook");
    let model_call_id = "mc-regression";
    let call_a = ToolCall::new("call-a", "skill", json!({}));
    let call_b = ToolCall::new("call-b", "grep", json!({}));
    let mut assistant_metadata = serde_json::Map::new();
    assistant_metadata.insert(
        MODEL_DIALOGUE_ITEM_ID_METADATA_KEY.to_string(),
        json!(model_call_id),
    );
    assistant_metadata.insert(
        REASONING_MESSAGE_ID_METADATA_KEY.to_string(),
        json!(reasoning_message_id(model_call_id)),
    );
    assistant_metadata.insert(
        TEXT_MESSAGE_ID_METADATA_KEY.to_string(),
        json!(text_message_id(model_call_id)),
    );
    let messages = vec![
        agent_types::ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::Thinking {
                    text: "planning the search".to_string(),
                    signature: None,
                },
                ContentPart::Text {
                    text: "I will locate the duplicate device logic.".to_string(),
                },
                ContentPart::ToolUse {
                    call: call_a.clone(),
                },
                ContentPart::ToolUse {
                    call: call_b.clone(),
                },
            ],
            name: None,
            created_at: None,
            metadata: assistant_metadata,
        },
        agent_types::ModelDialogueItem {
            id: None,
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                result: ToolResult::text(call_a.id.clone(), "skill body"),
            }],
            name: Some("skill".to_string()),
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        agent_types::ModelDialogueItem {
            id: None,
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                result: ToolResult::text(call_b.id.clone(), "grep matches"),
            }],
            name: Some("grep".to_string()),
            created_at: None,
            metadata: serde_json::Map::new(),
        },
    ];
    hook.after_model_call(
        &messages,
        1,
        &agent_types::TokenUsage::default(),
        None,
        false,
    )
    .await
    .expect("persist split assistant output");

    let persisted = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("turn messages");
    let split_assistant_records = persisted
        .iter()
        .filter(|message| message.role == "assistant")
        .count();
    assert_eq!(
        split_assistant_records, 4,
        "reasoning, text, and each Tool Use persist as separate assistant records"
    );

    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = false;
    let projector = ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "reaggregate".to_string(),
        },
        Some(8_192),
        None,
    )
    .expect("projection hook");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let prepared = projector
        .project(
            ChatRequest::streaming(vec![agent_types::ModelDialogueItem::text(
                Role::User,
                "find the duplicate device logic",
            )]),
            &event_tx,
        )
        .await
        .expect("projection preparation");

    let assistant_messages = prepared
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_messages.len(),
        1,
        "split assistant records must re-aggregate into one assistant model message"
    );
    let assistant = assistant_messages[0];
    assert!(
        matches!(
            assistant.content.first(),
            Some(ContentPart::Thinking { .. })
        ),
        "aggregated assistant keeps reasoning first"
    );
    assert!(
        matches!(assistant.content.get(1), Some(ContentPart::Text { .. })),
        "aggregated assistant keeps visible text"
    );
    let tool_use_ids = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolUse { call } => Some(call.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_use_ids,
        vec!["call-a", "call-b"],
        "both Tool Uses ride in the single aggregated assistant message"
    );
    let tool_result_ids = prepared
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            ContentPart::ToolResult { result } => Some(result.call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_result_ids,
        vec!["call-a", "call-b"],
        "each aggregated Tool Use still pairs with its durable Tool Result"
    );
}
