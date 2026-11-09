use std::collections::HashMap;
use std::sync::Arc;

use agent_types::{ChatRequest, ModelDialogueItem, Role};
use react_agent::{
    estimate_model_visible_input_tokens_with_messages, ModelCallProjector, ModelInputTokenEstimate,
    ModelInputTokenEstimateMethod, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};
use rua_contract::agent::AgentRequest;
use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, NewConversationMessage,
    NewConversationTurn, TurnLifecycle, WorkspaceSessionStore,
};

fn apply_delta(base: u64, current: u64, previous: u64) -> u64 {
    if current >= previous {
        base + current - previous
    } else {
        base - (previous - current)
    }
}

fn previous_assistant(
    identity: &CheckpointModelIdentity,
    fast_estimate_tokens: u64,
    conversation_estimate_tokens: u64,
    provider_prompt_tokens: u64,
) -> ModelDialogueItem {
    let mut message = ModelDialogueItem::text(Role::Assistant, "previous response");
    message.metadata.insert(
        MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
        serde_json::to_value(ModelInputTokenEstimate {
            provider_id: Some(identity.provider.clone()),
            model: Some(identity.model.clone()),
            fast_estimate_divisor: 4.0,
            fast_estimate_tokens,
            calibrated_estimate_tokens: fast_estimate_tokens,
            conversation_estimate_tokens,
            method: ModelInputTokenEstimateMethod::Fast,
            calibration_source_provider_id: None,
            calibration_source_model: None,
            previous_fast_estimate_tokens: None,
            previous_provider_prompt_tokens: None,
        })
        .expect("serialize estimate"),
    );
    message.metadata.insert(
        rua_agent::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY.to_string(),
        serde_json::json!({
            "last_call_usage": {
                "prompt_tokens": provider_prompt_tokens,
                "completion_tokens": 10,
                "total_tokens": provider_prompt_tokens + 10
            }
        }),
    );
    message
}

fn stored_message(
    message_id: &str,
    role: &str,
    content: serde_json::Value,
    metadata: serde_json::Map<String, serde_json::Value>,
) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-08-12T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata,
    }
}

#[test]
fn provider_prompt_usage_calibrates_the_next_final_request_estimate() {
    let identity = CheckpointModelIdentity {
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
    };
    let messages = vec![
        previous_assistant(&identity, 800, 500, 1_000),
        ModelDialogueItem::text(Role::User, "current request"),
    ];
    let request = ChatRequest::streaming(messages.clone());
    let fast = estimate_model_visible_input_tokens_with_messages(&request, &messages, 4.0);

    let estimate = rua_agent::projection_preparation::calibrated_model_input_token_estimate(
        &request, &messages, &identity, 4.0,
    );

    assert_eq!(estimate.fast_estimate_tokens, fast);
    assert_eq!(
        estimate.calibrated_estimate_tokens,
        apply_delta(1_000, fast, 800)
    );
    assert_eq!(
        estimate.method,
        ModelInputTokenEstimateMethod::ProviderDelta
    );
    assert_eq!(estimate.previous_fast_estimate_tokens, Some(800));
    assert_eq!(estimate.previous_provider_prompt_tokens, Some(1_000));
}

#[test]
fn latest_api_usage_replaces_stale_request_calibration() {
    let identity = CheckpointModelIdentity {
        provider: "test".into(),
        model: "test".into(),
    };
    let messages = vec![previous_assistant(&identity, 100, 50, 500)];
    let mut request = ChatRequest::streaming(messages.clone());
    let mut stale = messages[0].metadata[MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY].clone();
    stale["calibrated_estimate_tokens"] = serde_json::json!(1);
    request
        .metadata
        .insert(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.into(), stale);
    let estimate = rua_agent::projection_preparation::calibrated_model_input_token_estimate(
        &request, &messages, &identity, 4.0,
    );
    assert_eq!(estimate.previous_provider_prompt_tokens, Some(500));
    assert_eq!(
        estimate.calibrated_estimate_tokens,
        apply_delta(500, estimate.fast_estimate_tokens, 100)
    );
}

#[test]
fn calibration_uses_the_latest_provider_usage_across_model_changes() {
    let previous_identity = CheckpointModelIdentity {
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
    };
    let current_identity = CheckpointModelIdentity {
        provider: "provider-b".to_string(),
        model: "model-a".to_string(),
    };
    let messages = vec![
        previous_assistant(&previous_identity, 800, 500, 1_000),
        ModelDialogueItem::text(Role::User, "current request"),
    ];
    let request = ChatRequest::streaming(messages.clone());

    let estimate = rua_agent::projection_preparation::calibrated_model_input_token_estimate(
        &request,
        &messages,
        &current_identity,
        4.0,
    );

    assert_eq!(
        estimate.calibrated_estimate_tokens,
        apply_delta(1_000, estimate.fast_estimate_tokens, 800)
    );
    assert_eq!(
        estimate.method,
        ModelInputTokenEstimateMethod::ProviderDelta
    );
    assert_eq!(estimate.previous_fast_estimate_tokens, Some(800));
    assert_eq!(estimate.previous_provider_prompt_tokens, Some(1_000));
    assert_eq!(
        estimate.calibration_source_provider_id.as_deref(),
        Some("provider-a")
    );
    assert_eq!(
        estimate.calibration_source_model.as_deref(),
        Some("model-a")
    );
}

#[tokio::test]
async fn projection_recovers_usage_from_the_closing_part_of_a_split_assistant_message() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "split-assistant-usage";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Split assistant usage".to_string(),
            created_at: "2026-08-12T00:00:00Z".to_string(),
            last_activity_at: "2026-08-12T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    let previous_identity = CheckpointModelIdentity {
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
    };
    let estimate = serde_json::to_value(ModelInputTokenEstimate {
        provider_id: Some(previous_identity.provider.clone()),
        model: Some(previous_identity.model.clone()),
        fast_estimate_divisor: 4.0,
        fast_estimate_tokens: 800,
        calibrated_estimate_tokens: 800,
        conversation_estimate_tokens: 500,
        method: ModelInputTokenEstimateMethod::Fast,
        calibration_source_provider_id: None,
        calibration_source_model: None,
        previous_fast_estimate_tokens: None,
        previous_provider_prompt_tokens: None,
    })
    .expect("estimate");
    let dialogue_item_id =
        agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY;
    let shared_metadata = serde_json::Map::from_iter([
        (
            dialogue_item_id.to_string(),
            serde_json::json!("previous-call"),
        ),
        (
            MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
            estimate,
        ),
    ]);
    let mut closing_metadata = shared_metadata.clone();
    closing_metadata.insert(
        rua_agent::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY.to_string(),
        serde_json::json!({
            "last_call_usage": {
                "prompt_tokens": 1_000,
                "completion_tokens": 10,
                "total_tokens": 1_010
            }
        }),
    );
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-12T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-12T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-12T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    stored_message(
                        "user-1",
                        "user",
                        serde_json::json!([{"type": "text", "text": "first"}]),
                        serde_json::Map::new(),
                    ),
                    stored_message(
                        "assistant-reasoning",
                        "assistant",
                        serde_json::json!([{"type": "thinking", "thinking": "reasoning"}]),
                        shared_metadata,
                    ),
                    stored_message(
                        "assistant-text",
                        "assistant",
                        serde_json::json!([{"type": "text", "text": "done"}]),
                        closing_metadata,
                    ),
                ],
            },
        )
        .await
        .expect("previous turn");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-12T00:00:03Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![stored_message(
                    "user-2",
                    "user",
                    serde_json::json!([{"type": "text", "text": "second"}]),
                    serde_json::Map::new(),
                )],
            },
        )
        .await
        .expect("active turn");

    let mut running = rua_config::AgentsRunningConfig {
        max_input_length: 100_000,
        ..Default::default()
    };
    running.context_compact.context_compact_enabled = true;
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
            ("turn_id".to_string(), serde_json::json!("turn-2")),
            ("loaded_message_count".to_string(), serde_json::json!(0)),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };
    let current_identity = CheckpointModelIdentity {
        provider: "provider-b".to_string(),
        model: "model-b".to_string(),
    };
    let projector = rua_agent::projection_preparation::ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(running),
        current_identity.clone(),
        Some(100_000),
        None,
    )
    .expect("projector");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);
    let projected = projector
        .project(
            ChatRequest::streaming(vec![ModelDialogueItem::text(Role::User, "second")]),
            &event_tx,
        )
        .await
        .expect("project request");
    let estimate: ModelInputTokenEstimate =
        serde_json::from_value(projected.metadata[MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY].clone())
            .expect("calibrated estimate");

    assert_eq!(
        estimate.method,
        ModelInputTokenEstimateMethod::ProviderDelta
    );
    assert_eq!(estimate.previous_provider_prompt_tokens, Some(1_000));
    assert_eq!(
        estimate,
        rua_agent::projection_preparation::calibrated_model_input_token_estimate(
            &projected,
            &projected.messages,
            &current_identity,
            4.0,
        ),
        "the stored measurement must describe the final assembled request"
    );
    assert_eq!(
        estimate.conversation_estimate_tokens,
        react_agent::estimate_model_visible_conversation_tokens(&projected.messages, 4.0)
    );
    assert_eq!(
        estimate.calibration_source_provider_id.as_deref(),
        Some("provider-a")
    );
}
