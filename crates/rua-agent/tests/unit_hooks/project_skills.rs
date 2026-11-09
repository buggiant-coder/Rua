use std::collections::HashMap;

use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use react_agent::AgentHook;
use rua_agent::turn_persistence::{TurnPersistenceHook, LOADED_MESSAGE_COUNT_METADATA_KEY};
use rua_contract::agent::{AgentInputMessage, AgentRequest};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};

#[tokio::test]
async fn project_skill_tool_use_and_result_enter_conversation_storage() {
    let storage = tempfile::tempdir().unwrap();
    let chat_id = "project-skill-chat";
    let turn_id = "turn-1";
    let store = WorkspaceSessionStore::new(storage.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent".to_string(),
            title: "Project Skill".to_string(),
            created_at: "2026-08-01T00:00:00Z".to_string(),
            last_activity_at: "2026-08-01T00:00:00Z".to_string(),
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
                    user_submitted_at: "2026-08-01T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-08-01T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "use the skill"}]),
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
                text: "use the skill".to_string(),
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
    let assistant = ModelDialogueItem {
        id: Some("assistant-1".to_string()),
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new(
                "skill-call",
                "skill",
                serde_json::json!({"name": "project:review"}),
            ),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let result = ToolResult::text(
        "skill-call",
        "PROJECT_SKILL_BODY\n\nSource: project\nBase directory: /repo/.agents/skills/review",
    );
    let tool_message = ModelDialogueItem {
        id: Some("tool-1".to_string()),
        role: Role::Tool,
        content: vec![ContentPart::ToolResult { result }],
        name: Some("skill".to_string()),
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
    let serialized = serde_json::to_string(&stored).unwrap();
    assert!(serialized.contains("PROJECT_SKILL_BODY"));
    assert!(serialized.contains("Source: project"));
    assert!(serialized.contains("/repo/.agents/skills/review"));
    assert!(serialized.contains("skill-call"));
}
