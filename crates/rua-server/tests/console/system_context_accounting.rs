use std::sync::Arc;

use agent_types::ToolDefinition;
use react_agent::tool_disclosure::ToolDisclosureMode;
use react_agent::toolkit::{
    AgentToolkit, DisclosureClass, MockToolExecutor, ToolDisclosureMeta, ToolSourceKind,
};
use react_agent::{
    ModelInputTokenEstimate, ModelInputTokenEstimateMethod, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};
use rua_session::{CheckpointModelIdentity, ContextProjection, MessageLogEntry};

fn message_with_usage(
    identity: &CheckpointModelIdentity,
    fast_estimate_tokens: u64,
    conversation_estimate_tokens: u64,
    prompt_tokens: u64,
) -> MessageLogEntry {
    MessageLogEntry {
        message_id: "assistant-usage".to_string(),
        sequence: 1,
        turn_id: "turn-1".to_string(),
        role: "assistant".to_string(),
        created_at: "2026-08-12T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: serde_json::json!([{"type": "text", "text": "done"}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::from_iter([
            (
                rua_agent::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY.to_string(),
                serde_json::json!({
                    "last_call_usage": {
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": 10,
                        "total_tokens": prompt_tokens + 10
                    }
                }),
            ),
            (
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
            ),
        ]),
    }
}

fn projection(tail_messages: Vec<MessageLogEntry>) -> ContextProjection {
    ContextProjection {
        chat_id: "chat-1".to_string(),
        agent_id: "agent-1".to_string(),
        active_checkpoint_id: None,
        summary: None,
        summary_model: None,
        tail_start_sequence: tail_messages.first().map(|message| message.sequence),
        tail_end_sequence: tail_messages.last().map(|message| message.sequence),
        retained_messages: Vec::new(),
        tail_messages,
        current_user_input: None,
        stable_token_estimate: 500,
        history_replacement_evidence: Vec::new(),
    }
}

fn tool_definition(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            }
        }),
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn meta(class: DisclosureClass, tokens: usize) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class,
        source_kind: ToolSourceKind::Builtin,
        source_name: Some("test".into()),
        category: Some("test".into()),
        schema_tokens: Some(tokens),
    }
}

fn toolkit(deferred_name: &str, deferred_description: &str) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition("read", "Read files"),
        Arc::new(MockToolExecutor::new("ok")),
        meta(DisclosureClass::Direct, 8),
    );
    toolkit.register_with_disclosure(
        tool_definition(deferred_name, deferred_description),
        Arc::new(MockToolExecutor::new("ok")),
        meta(DisclosureClass::DeferredCandidate, 100),
    );
    toolkit
}

#[test]
fn early_compaction_applies_conversation_delta_to_the_last_provider_prompt_usage() {
    let identity = CheckpointModelIdentity {
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
    };
    let with_usage = projection(vec![message_with_usage(&identity, 600, 400, 500)]);

    assert_eq!(
        rua_server::services::context_projection::early_compaction_total_tokens(
            &with_usage,
            550,
            100,
        ),
        650
    );
    assert_eq!(
        rua_server::services::context_projection::early_compaction_total_tokens(
            &projection(Vec::new()),
            500,
            100,
        ),
        600
    );
}

#[test]
fn disclosure_plan_changes_system_context_hash_and_tokens() {
    let running = rua_config::AgentsRunningConfig {
        tool_disclosure: rua_config::types::ToolDisclosureRunningConfig {
            enabled: rua_config::types::ToolDisclosureEnabled::On,
            ..Default::default()
        },
        ..Default::default()
    };
    let base = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        None,
        Some(&toolkit("task_create", "Create a task")),
        Some(&running),
        Some(1_000),
    );
    let changed = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        None,
        Some(&toolkit("task_cancel", "Cancel a task")),
        Some(&running),
        Some(1_000),
    );

    assert_ne!(base.system_context_hash, changed.system_context_hash);
    assert!(base.system_context_tokens > 0);
    assert!(base.visible_tool_schema_tokens > 0);
    assert!(base.capability_index_tokens > 0);
}

#[test]
fn skill_prompt_block_changes_system_context_hash_and_tokens() {
    let without_skill = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        None,
        None,
        None,
        Some(1_000),
    );
    let with_skill = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        Some("## Available Skills\n- diagnose: debug failures".into()),
        None,
        None,
        Some(1_000),
    );

    assert_ne!(
        without_skill.system_context_hash,
        with_skill.system_context_hash
    );
    assert!(with_skill.system_context_tokens > without_skill.system_context_tokens);
}

#[test]
fn disclosure_accounting_is_deterministic_and_off_preserves_legacy_hash() {
    let running = rua_config::AgentsRunningConfig {
        tool_disclosure: rua_config::types::ToolDisclosureRunningConfig {
            enabled: rua_config::types::ToolDisclosureEnabled::Off,
            ..Default::default()
        },
        ..Default::default()
    };
    let first = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        None,
        Some(&toolkit("task_create", "Create a task")),
        Some(&running),
        Some(1_000),
    );
    let second = rua_server::services::context_projection::build_system_context_accounting(
        vec!["system prompt".into()],
        None,
        Some(&toolkit("task_create", "Create a task")),
        Some(&running),
        Some(1_000),
    );

    assert_eq!(first.system_context_hash, second.system_context_hash);
    assert_eq!(first.disclosure_mode, ToolDisclosureMode::Off);
    assert!(!first.disclosure_activated);
    assert_eq!(first.capability_index_tokens, 0);
}
