use std::collections::HashMap;
use std::sync::Arc;

use agent_types::{ChatRequest, ContentPart, ModelDialogueItem, Role, ToolCall};
use react_agent::{AgentHook, ModelCallProjector};
use rua_agent::projection_preparation::ContextProjectionProjector;
use rua_agent::turn_persistence::{TurnPersistenceHook, LOADED_MESSAGE_COUNT_METADATA_KEY};
use rua_contract::{
    agent::{AgentInputMessage, AgentRequest},
    Tool,
};
use rua_core::tool::ToolContext;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use rua_tool::extended::update_plan::UpdatePlanTool;

#[tokio::test]
async fn update_plan_tool_use_and_result_enter_conversation_storage() {
    let storage = tempfile::tempdir().unwrap();
    let chat_id = "work-checklist-chat";
    let turn_id = "turn-1";
    let store = WorkspaceSessionStore::new(storage.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent".to_string(),
            title: "Work checklist".to_string(),
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
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
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
                    content: serde_json::json!([{
                        "type": "text",
                        "text": "implement the harness change"
                    }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user".to_string(),
        agent_id: "agent".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "implement the harness change".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::from([
            ("turn_id".to_string(), serde_json::json!(turn_id)),
            (
                LOADED_MESSAGE_COUNT_METADATA_KEY.to_string(),
                serde_json::json!(1),
            ),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: None,
        conversation_store_dir: Some(storage.path().to_path_buf()),
        working_dir: None,
    };
    let hook = TurnPersistenceHook::from_request(&request).unwrap();
    let checklist = serde_json::json!({
        "explanation": "Investigation is complete.",
        "plan": [
            {"step": "Implement the change", "status": "in_progress"},
            {"step": "Verify the result", "status": "pending"}
        ]
    });
    let assistant = ModelDialogueItem {
        id: Some("assistant-1".to_string()),
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new("plan-call", "update_plan", checklist.clone()),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let mut result = UpdatePlanTool::new()
        .call(
            checklist,
            ToolContext {
                agent_id: "agent".to_string(),
                session_id: chat_id.to_string(),
                conversation_store_dir: Some(storage.path().to_path_buf()),
                metadata: HashMap::from([("turn_id".to_string(), serde_json::json!(turn_id))]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    result.call_id = "plan-call".to_string();
    result.name = Some("update_plan".to_string());
    let tool_message = ModelDialogueItem {
        id: Some("tool-1".to_string()),
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some("update_plan".to_string()),
        created_at: None,
        metadata: serde_json::Map::new(),
    };

    hook.after_model_call(
        &[assistant, tool_message],
        1,
        &agent_types::TokenUsage::default(),
        None,
        false,
    )
    .await
    .unwrap();

    let stored = store.read_turn_messages(chat_id, turn_id).await.unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[1].role, "assistant");
    assert_eq!(stored[2].role, "tool");
    assert_eq!(
        stored[1].content[0]["name"],
        serde_json::json!("update_plan")
    );
    assert_eq!(
        stored[1].content[0]["input"]["plan"][0]["status"],
        serde_json::json!("in_progress")
    );
    assert_eq!(
        stored[2].content[0]["metadata"]["work_checklist"]["plan"][1]["step"],
        serde_json::json!("Verify the result")
    );

    let active_plan = store
        .load_active_work_plan(chat_id)
        .await
        .unwrap()
        .expect("active work plan");
    assert_eq!(active_plan.source_turn_id.as_deref(), Some(turn_id));
    assert_eq!(active_plan.source_sequence, Some(1));
    assert_eq!(active_plan.plan[0].step, "Implement the change");

    let disabled_projector = ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(rua_config::AgentsRunningConfig::default()),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "work-checklist-disabled".to_string(),
        },
        Some(1_000_000),
        None,
    )
    .expect("disabled context projection projector");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);
    let projected_without_injection = disabled_projector
        .project(ChatRequest::streaming(Vec::new()), &event_tx)
        .await
        .unwrap();
    assert!(projected_without_injection.messages.iter().all(|message| {
        message
            .metadata
            .get("rua_active_work_plan")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    }));

    let mut running = rua_config::AgentsRunningConfig {
        max_input_length: 1_000,
        ..Default::default()
    };
    running.context_compact.active_work_plan_injection_enabled = true;
    running
        .context_compact
        .active_work_plan_injection_interval_ratio = 0.01;
    let projector = ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(running),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "work-checklist".to_string(),
        },
        Some(1_000_000),
        None,
    )
    .expect("context projection projector");
    let projected = projector
        .project(ChatRequest::streaming(Vec::new()), &event_tx)
        .await
        .unwrap();
    let injected = projected.messages.last().expect("injected active plan");
    assert_eq!(injected.role, Role::Assistant);
    assert_eq!(
        injected
            .metadata
            .get("rua_active_work_plan")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        injected.text_content(),
        "<active_work_plan>\nExplanation: Investigation is complete.\n- [in_progress] Implement the change\n- [pending] Verify the result\n</active_work_plan>"
    );

    assert_eq!(
        store
            .read_turn_messages(chat_id, turn_id)
            .await
            .unwrap()
            .len(),
        3,
        "projection-only plan message must not enter the Message Log"
    );
    let anchored_plan = store
        .load_active_work_plan(chat_id)
        .await
        .unwrap()
        .expect("anchored active work plan");
    assert_eq!(anchored_plan.last_injected_sequence, Some(3));

    let projected_before_interval = projector
        .project(ChatRequest::streaming(Vec::new()), &event_tx)
        .await
        .unwrap();
    assert!(projected_before_interval.messages.iter().all(|message| {
        message
            .metadata
            .get("rua_active_work_plan")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    }));

    tokio::fs::remove_file(store.chat_dir(chat_id).join("active_work_plan.json"))
        .await
        .unwrap();
    let projected_without_snapshot = projector
        .project(ChatRequest::streaming(Vec::new()), &event_tx)
        .await
        .unwrap();
    assert!(
        projected_without_snapshot.messages.iter().all(|message| {
            message
                .metadata
                .get("rua_active_work_plan")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        }),
        "historical update_plan messages must not rebuild the projection snapshot"
    );
}
