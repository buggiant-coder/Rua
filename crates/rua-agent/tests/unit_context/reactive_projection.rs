use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use async_trait::async_trait;
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::{
    AgentContext, ModelCallProjector, ModelCallSection, ModelCallSectionKind, ReactConfig,
};
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
use rua_agent::projection_preparation::ContextProjectionProjector;
use rua_config::AgentsRunningConfig;
use rua_contract::agent::AgentRequest;
use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, NewConversationMessage,
    NewConversationTurn, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;

const OVERFLOW: &str =
    "Your input exceeds the context window of this model. Please adjust your input and try again.";

struct InputLimitedModel {
    info: ModelInfo,
    requests: Mutex<Vec<ChatRequest>>,
    input_limit: u64,
}

#[async_trait]
impl ChatModel for InputLimitedModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only model")
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        let tokens = react_agent::estimate_model_visible_input_tokens_with_messages(
            &request,
            &request.messages,
            4.0,
        );
        self.requests.lock().unwrap().push(request);
        let event = if tokens > self.input_limit {
            AgentEvent::Error {
                message: OVERFLOW.into(),
                code: Some("context_length_exceeded".into()),
                retryable: false,
                metadata: serde_json::Value::Null,
            }
        } else {
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            }
        };
        Box::pin(futures::stream::iter([Ok(event)]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct TestContextManager {
    calls: AtomicU32,
    mode: SummaryMode,
}

#[derive(Clone, Copy)]
enum SummaryMode {
    Success,
    Error,
    Timeout,
    OverflowThenSuccess,
    AlwaysOverflow,
}

#[async_trait]
impl ContextManager for TestContextManager {
    async fn compact_context(
        &self,
        _request: CompactContextRequest,
    ) -> Result<CompactContextResult, rua_agent::AgentError> {
        panic!("overflow recovery must use the checkpoint publisher");
    }

    async fn summarize_context(
        &self,
        request: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, rua_agent::AgentError> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(request.max_tokens, Some(4_096));
        assert_eq!(request.model.as_ref().unwrap().model, "offline");
        assert!(!serde_json::to_string(&request.messages)
            .unwrap()
            .contains("current-project-instructions"));
        match self.mode {
            SummaryMode::Timeout => std::future::pending().await,
            SummaryMode::Error => Err(rua_agent::AgentError::provider("summary unavailable")),
            SummaryMode::OverflowThenSuccess if call == 0 => {
                Err(rua_agent::AgentError::provider(OVERFLOW))
            }
            SummaryMode::AlwaysOverflow => Err(rua_agent::AgentError::provider(OVERFLOW)),
            _ => Ok(ContextSummaryResult {
                summary: "durable checkpoint summary".into(),
            }),
        }
    }
}

#[tokio::test]
async fn provider_overflow_retry_uses_reduced_durable_projection() {
    assert_overflow_recovers(SummaryMode::Error, false, 0.9).await;
}

#[tokio::test]
async fn provider_overflow_publishes_checkpoint_before_retry() {
    assert_overflow_recovers(SummaryMode::Success, false, 0.9).await;
}

#[tokio::test(start_paused = true)]
async fn provider_overflow_summary_timeout_uses_bounded_tail() {
    assert_overflow_recovers(SummaryMode::Timeout, false, 0.9).await;
}

#[tokio::test]
async fn provider_overflow_reduces_rejected_summary_source() {
    assert_overflow_recovers(SummaryMode::OverflowThenSuccess, false, 0.9).await;
}

#[tokio::test]
async fn provider_overflow_keeps_active_user_and_tool_pairs_when_summary_fails() {
    assert_overflow_recovers(SummaryMode::Error, true, 0.9).await;
}

#[tokio::test]
async fn provider_overflow_fallback_can_fit_emergency_limit_above_maintenance_target() {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        assert_overflow_recovers(SummaryMode::Error, false, 0.5),
    )
    .await
    .expect("a bounded fallback must not loop on the same failed summary");
}

#[tokio::test]
async fn summary_overflow_before_hard_limit_keeps_active_user_recent_tool_round_and_plan() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "summary-fallback".into(),
            agent_id: "agent-1".into(),
            title: "fallback".into(),
            created_at: "2026-09-22T00:00:00Z".into(),
            last_activity_at: "2026-09-22T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    let make = |id: &str, role: &str, content| NewConversationMessage {
        message_id: id.into(),
        role: role.into(),
        content,
        created_at: "2026-09-22T00:00:00Z".into(),
        status: "completed".into(),
        provider: None,
        tool: None,
        metadata: Default::default(),
    };
    store.append_conversation_turn("summary-fallback", NewConversationTurn {
        turn_id: "active".into(), lifecycle: TurnLifecycle {
            user_submitted_at: "2026-09-22T00:00:00Z".into(), assistant_started_at: None,
            assistant_completed_at: None, status: "in_progress".into(),
        }, selected_variant_id: None, messages: vec![
            make("user", "user", json!([{"type":"text","text":"finish Issue 11"}])),
            make("old", "assistant", json!([{"type":"text","text":"earlier-work ".repeat(1700)}])),
            make("plan-use", "assistant", json!([{"type":"tool_use","id":"plan-1","name":"update_plan","input":{"plan":[{"step":"finish Issue 11","status":"in_progress"}]}}])),
            make("plan-result", "tool", json!([{"type":"tool_result","id":"plan-1","output":[{"type":"text","text":"Plan updated"}]}])),
            make("intermediate", "assistant", json!([{"type":"text","text":"verified earlier work ".repeat(100)}])),
            make("use", "assistant", json!([{"type":"tool_use","id":"edit-1","name":"edit","input":{"patch":"current edit"}}])),
            make("result", "tool", json!([{"type":"tool_result","id":"edit-1","output":[{"type":"text","text":"Issue 11 edited"}]}])),
        ],
    }).await.unwrap();
    store
        .save_active_work_plan(
            "summary-fallback",
            Some("active"),
            Some(3),
            rua_session::ActiveWorkPlanContent {
                explanation: Some("Current Issue 11 progress".into()),
                plan: vec![rua_session::ActiveWorkPlanStep {
                    step: "finish Issue 11".into(),
                    status: rua_session::ActiveWorkPlanStepStatus::InProgress,
                }],
            },
        )
        .await
        .unwrap();
    let plan = store
        .load_active_work_plan("summary-fallback")
        .await
        .unwrap()
        .unwrap();
    store
        .mark_active_work_plan_injected("summary-fallback", &plan.checksum, Some(4), None)
        .await
        .unwrap();
    let mut running = AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = false;
    running.context_compact.active_work_plan_injection_enabled = true;
    running.tool_result_compact.projection_truncation_enabled = false;
    let projection = store
        .build_context_projection(
            "summary-fallback",
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    let messages = rua_agent::projection_preparation::project_message_log_entries_for_model(
        &projection.tail_messages,
    );
    let tokens = react_agent::estimate_model_visible_input_tokens_with_messages(
        &ChatRequest::new(messages.clone()),
        &messages,
        4.0,
    );
    running.max_input_length = (tokens as f64 / 0.95).ceil() as u32;
    let input = AgentRequest {
        session_id: "summary-fallback".into(),
        agent_id: "agent-1".into(),
        user_id: "user".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".into(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".into(), json!("active")),
            ("loaded_message_count".into(), json!(7)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(dir.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let manager = Arc::new(TestContextManager {
        calls: AtomicU32::new(0),
        mode: SummaryMode::AlwaysOverflow,
    });
    let projector = ContextProjectionProjector::from_request(
        &input,
        Some(manager.clone()),
        Arc::new(running),
        CheckpointModelIdentity {
            provider: "offline".into(),
            model: "offline".into(),
        },
        None,
        None,
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let request = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        projector.project(ChatRequest::new(messages), &tx),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(manager.calls.load(Ordering::Relaxed) > 2);
    assert!(request
        .messages
        .iter()
        .any(|message| message.text_content().contains("finish Issue 11")));
    assert!(request
        .messages
        .iter()
        .any(|message| message.tool_calls().iter().any(|call| call.id == "edit-1")));
    assert!(request.messages.iter().any(|message| message
        .tool_results()
        .iter()
        .any(|result| result.call_id == "edit-1")));
    assert!(request
        .messages
        .iter()
        .any(|message| message.text_content().contains("<active_work_plan>")));
}

async fn assert_overflow_recovers(mode: SummaryMode, long_active_turn: bool, blocking_ratio: f64) {
    assert_overflow_recovers_with_instructions(mode, long_active_turn, blocking_ratio, false).await;
}

#[tokio::test]
async fn recovery_does_not_require_halving_non_compressible_instructions() {
    assert_overflow_recovers_with_instructions(SummaryMode::Success, false, 0.9, true).await;
}

async fn assert_overflow_recovers_with_instructions(
    mode: SummaryMode,
    long_active_turn: bool,
    blocking_ratio: f64,
    large_instructions: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "reactive-overflow";
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.into(),
            agent_id: "agent-1".into(),
            title: "overflow".into(),
            created_at: "2026-09-21T00:00:00Z".into(),
            last_activity_at: "2026-09-21T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    for i in 0..if long_active_turn { 0 } else { 8 } {
        let messages = [
            ("user", format!("request-{i}")),
            ("assistant", "old-history ".repeat(1_000)),
        ]
        .into_iter()
        .map(|(role, text)| NewConversationMessage {
            message_id: format!("{role}-{i}"),
            role: role.into(),
            created_at: "2026-09-21T00:00:00Z".into(),
            status: "completed".into(),
            content: json!([{"type":"text", "text":text}]),
            provider: None,
            tool: None,
            metadata: Default::default(),
        })
        .collect();
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{i}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-09-21T00:00:00Z".into(),
                        assistant_started_at: Some("2026-09-21T00:00:00Z".into()),
                        assistant_completed_at: Some("2026-09-21T00:00:01Z".into()),
                        status: "completed".into(),
                    },
                    selected_variant_id: None,
                    messages,
                },
            )
            .await
            .unwrap();
    }
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "current-turn".into(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-09-21T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "current-user".into(),
                    role: "user".into(),
                    created_at: "2026-09-21T00:00:00Z".into(),
                    status: "completed".into(),
                    content: json!([{"type":"text", "text":"continue-current-request"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
    if long_active_turn {
        let messages = (0..8).flat_map(|i| {
            [("assistant", json!([{"type":"tool_use", "id":format!("call-{i}"), "name":"read", "input":{"path":"source.txt"}}])),
             ("tool", json!([{"type":"tool_result", "id":format!("call-{i}"), "name":"read", "output":[{"type":"text", "text":"old-history ".repeat(1_000)}]}]))]
                .into_iter().map(move |(role,content)| NewConversationMessage {
                    message_id: format!("{role}-{i}"), role: role.into(), created_at: "2026-09-21T00:00:01Z".into(),
                    status: "completed".into(), content, provider: None, tool: None, metadata: Default::default(),
                })
        }).collect();
        store
            .append_messages_to_turn(
                chat_id,
                "current-turn",
                TurnLifecycle {
                    user_submitted_at: "2026-09-21T00:00:00Z".into(),
                    assistant_started_at: Some("2026-09-21T00:00:01Z".into()),
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                None,
                messages,
            )
            .await
            .unwrap();
    }
    let mut running = AgentsRunningConfig {
        max_input_length: 100_000,
        ..Default::default()
    };
    running.context_compact.blocking_compact_ratio = blocking_ratio;
    running.tool_result_compact.projection_truncation_enabled = false;
    let running = Arc::new(running);
    let agent_request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "test".into(),
        agent_id: "agent-1".into(),
        messages: Vec::new(),
        model_override: None,
        tool_mode: "auto".into(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".into(), json!("current-turn")),
            ("loaded_message_count".into(), json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(dir.path().into()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let manager = Arc::new(TestContextManager {
        calls: AtomicU32::new(0),
        mode,
    });
    let projector = Arc::new(
        ContextProjectionProjector::from_request(
            &agent_request,
            Some(manager.clone()),
            running,
            CheckpointModelIdentity {
                provider: "offline".into(),
                model: "offline".into(),
            },
            Some(100_000),
            None,
        )
        .unwrap(),
    );
    let model = Arc::new(InputLimitedModel {
        info: ModelInfo {
            id: "offline".into(),
            name: "offline".into(),
            context_window: Some(100_000),
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: Default::default(),
        },
        requests: Mutex::new(Vec::new()),
        input_limit: if large_instructions { 42_000 } else { 16_000 },
    });
    let project_instructions = if large_instructions {
        format!("current-project-instructions{}", "s".repeat(160_000))
    } else {
        "current-project-instructions".to_string()
    };
    let ctx = AgentContext::new(
        model.clone(),
        Arc::new(react_agent::AgentToolkit::new()),
        ReactConfig::default(),
        Arc::new(react_agent::hooks::NoopHook),
        Default::default(),
    )
    .with_model_call_projector(projector.clone())
    .with_model_call_sections(vec![ModelCallSection::new(
        ModelCallSectionKind::ProjectInstructions,
        project_instructions.clone(),
    )]);
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = react_agent::react_loop::run_react_loop(
        &ctx,
        Vec::<ModelDialogueItem>::new(),
        tx,
        &AtomicU32::new(0),
    )
    .await;
    let requests = model.requests.lock().unwrap().clone();
    let sizes = requests
        .iter()
        .map(|request| {
            react_agent::estimate_model_visible_input_tokens_with_messages(
                request,
                &request.messages,
                4.0,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        result.is_ok(),
        "compacted retry must recover: {result:?}; request sizes: {sizes:?}"
    );
    assert_eq!(requests.len(), 2);
    assert!(sizes[1] < sizes[0]);
    if large_instructions {
        assert!(
            sizes[1] > sizes[0] / 2,
            "fixed instructions must not create an artificial half-input limit: {sizes:?}"
        );
    }
    let checkpoint = store.load_active_summary_checkpoint(chat_id).await.unwrap();
    let summary_succeeded = matches!(
        mode,
        SummaryMode::Success | SummaryMode::OverflowThenSuccess
    );
    assert_eq!(checkpoint.is_some(), summary_succeeded);
    assert!(manager.calls.load(Ordering::Relaxed) >= 1);
    if matches!(mode, SummaryMode::OverflowThenSuccess) {
        assert!(manager.calls.load(Ordering::Relaxed) >= 2);
    }
    let mut statuses = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ContextCompaction { status, .. } = event {
            statuses.push(status);
        }
    }
    assert!(statuses.iter().any(|status| status == "started"));
    assert!(statuses.iter().all(|status| status != "failed"));
    let estimate = &requests[1].metadata[react_agent::MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY];
    assert_eq!(estimate["method"], "fast");
    assert_eq!(estimate["calibrated_estimate_tokens"], sizes[1]);
    let raw = store.read_message_range(chat_id, 1, 32).await.unwrap();
    assert_eq!(raw.len(), 17, "recovery must not mutate raw history");
    let (next_tx, _next_rx) = tokio::sync::mpsc::channel(64);
    let repeated = projector
        .project(
            ChatRequest::streaming(requests[1].messages.clone()),
            &next_tx,
        )
        .await
        .unwrap();
    assert_eq!(
        repeated.messages, requests[1].messages,
        "the next call must keep the recovered projection"
    );
    let mut calibrated_request = requests[1].clone();
    let baseline = calibrated_request
        .metadata
        .get_mut(react_agent::MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
        .unwrap();
    baseline["calibrated_estimate_tokens"] = json!(sizes[1] + 1_000);
    baseline["method"] = json!("provider_delta");
    baseline["previous_provider_prompt_tokens"] = json!(sizes[1] + 1_000);
    let calibrated = projector
        .project(calibrated_request, &next_tx)
        .await
        .unwrap();
    let estimate = &calibrated.metadata[react_agent::MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY];
    assert_eq!(estimate["method"], "provider_delta");
    assert_eq!(
        estimate["calibrated_estimate_tokens"],
        sizes[1] + 1_000,
        "successful usage calibration must survive recovery and durable tail rebuild"
    );
    for request in requests.iter() {
        let calls = request
            .messages
            .iter()
            .flat_map(|message| message.tool_calls())
            .map(|call| call.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let results = request
            .messages
            .iter()
            .flat_map(|message| message.tool_results())
            .map(|result| result.call_id.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(calls, results, "fallback must preserve tool pairing");
        assert!(request
            .messages
            .iter()
            .any(|message| message.role == Role::Developer
                && message.text_content() == project_instructions));
        assert!(request
            .messages
            .iter()
            .any(|message| message.role == Role::User
                && message.text_content() == "continue-current-request"));
    }
}
