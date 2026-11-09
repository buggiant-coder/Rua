use std::collections::HashMap;
use std::path::PathBuf;

use rua_contract::agent::{AgentInputMessage, AgentRequest};
use rua_core::message::ContentPart;

fn request(workspace_dir: PathBuf) -> AgentRequest {
    AgentRequest {
        session_id: "session-1".into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace_dir),
        conversation_store_dir: None,
        working_dir: None,
    }
}

#[test]
fn runtime_paths_fall_back_to_agent_workspace() {
    let request = request(PathBuf::from("/agent-workspace"));

    assert_eq!(
        request.conversation_store_dir(),
        Some(std::path::Path::new("/agent-workspace"))
    );
    assert_eq!(
        request.working_dir(),
        Some(std::path::Path::new("/agent-workspace"))
    );
}

#[test]
fn explicit_runtime_paths_override_agent_workspace() {
    let mut request = request(PathBuf::from("/agent-workspace"));
    request.conversation_store_dir = Some(PathBuf::from("/conversation-store"));
    request.working_dir = Some(PathBuf::from("/project-root"));

    assert_eq!(
        request.conversation_store_dir(),
        Some(std::path::Path::new("/conversation-store"))
    );
    assert_eq!(
        request.working_dir(),
        Some(std::path::Path::new("/project-root"))
    );
}

#[test]
fn runtime_paths_are_not_serialized() {
    let mut request = request(PathBuf::from("/agent-workspace"));
    request.conversation_store_dir = Some(PathBuf::from("/conversation-store"));
    request.working_dir = Some(PathBuf::from("/project-root"));

    let value = serde_json::to_value(&request).expect("serialize request");
    let object = value.as_object().expect("request object");
    assert!(!object.contains_key("workspace_dir"));
    assert!(!object.contains_key("conversation_store_dir"));
    assert!(!object.contains_key("working_dir"));

    let decoded: AgentRequest = serde_json::from_value(value).expect("deserialize request");
    assert!(decoded.workspace_dir.is_none());
    assert!(decoded.conversation_store_dir.is_none());
    assert!(decoded.working_dir.is_none());
}
