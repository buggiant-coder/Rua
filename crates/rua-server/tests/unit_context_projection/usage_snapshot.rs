use rua_config::AgentsRunningConfig;
use rua_server::services::context_projection::{
    build_context_usage_snapshot_from_projection, SystemContextAccounting,
};
use rua_session::ContextProjection;

#[test]
fn projection_context_usage_snapshot_includes_system_and_projection_breakdown() {
    let running = AgentsRunningConfig {
        max_input_length: 1_000,
        ..Default::default()
    };
    let system_context = SystemContextAccounting {
        system_context_hash: "hash".to_string(),
        system_context_tokens: 120,
        visible_tool_schema_tokens: 80,
        capability_index_tokens: 20,
        disclosure_mode: react_agent::tool_disclosure::ToolDisclosureMode::On,
        disclosure_activated: true,
    };
    let projection = ContextProjection {
        chat_id: "chat-a".to_string(),
        agent_id: "agent-a".to_string(),
        active_checkpoint_id: Some("checkpoint-a".to_string()),
        summary: Some("compact summary text".to_string()),
        summary_model: None,
        tail_start_sequence: Some(10),
        tail_end_sequence: Some(20),
        retained_messages: Vec::new(),
        tail_messages: Vec::new(),
        current_user_input: None,
        stable_token_estimate: 340,
        history_replacement_evidence: Vec::new(),
    };

    let snapshot = build_context_usage_snapshot_from_projection(
        470,
        Some(&running),
        &system_context,
        &projection,
        Some("hello user"),
        true,
    )
    .expect("snapshot");

    assert_eq!(snapshot["source"], "projection");
    assert_eq!(snapshot["estimated_tokens"], 470);
    assert_eq!(snapshot["max_input_length"], 1000);
    assert_eq!(snapshot["context_usage_ratio"], 47.0);
    assert_eq!(snapshot["breakdown"]["system_context_tokens"], 120);
    assert_eq!(snapshot["breakdown"]["visible_tool_schema_tokens"], 80);
    assert_eq!(snapshot["breakdown"]["capability_index_tokens"], 20);
    assert_eq!(snapshot["breakdown"]["projected_conversation_tokens"], 340);
    assert_eq!(
        snapshot["breakdown"]["used_emergency_bounded_projection"],
        true
    );
}
