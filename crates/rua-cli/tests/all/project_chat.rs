use clap::Parser;
use rua_cli::project::{
    build_project_chat_request, project_chat_history_endpoint, project_chats_endpoint,
};
use rua_cli::{Cli, Commands};
use serde_json::json;

#[test]
fn project_chat_command_series_is_registered() {
    for args in [
        vec!["rua", "project", "chat", "list", "project-1"],
        vec!["rua", "project", "chat", "show", "project-1", "chat-1"],
        vec!["rua", "project", "chat", "send", "project-1", "hello"],
        vec![
            "rua",
            "project",
            "chat",
            "stream",
            "project-1",
            "hello",
            "--mode",
            "execute",
        ],
        vec![
            "rua",
            "project",
            "chat",
            "update",
            "project-1",
            "chat-1",
            "--name",
            "Renamed",
            "--mode",
            "plan",
        ],
        vec!["rua", "project", "chat", "delete", "project-1", "chat-1"],
        vec!["rua", "project", "chat", "stop", "project-1", "chat-1"],
    ] {
        let cli = Cli::try_parse_from(args).expect("Project Chat command should parse");
        assert!(matches!(cli.command, Commands::Project(_)));
    }
}

#[test]
fn project_is_a_list_filter_and_history_uses_an_encoded_chat_id() {
    assert_eq!(
        project_chats_endpoint("project/one", 20, 10, Some("fix race")),
        "/api/chats?project_id=project%2Fone&offset=20&limit=10&search=fix+race"
    );
    assert_eq!(
        project_chat_history_endpoint("existing", None, None),
        "/api/chats/existing"
    );
    assert_eq!(
        project_chat_history_endpoint("chat/one", Some(10), Some(42)),
        "/api/chats/chat%2Fone?turn_limit=10&before_turn_ordinal=42"
    );
}

#[test]
fn project_chat_request_uses_chat_identity_and_execution_agent() {
    let request = build_project_chat_request(
        "inspect this repository",
        "code",
        "photos",
        Some("00000000-0000-4000-8000-000000000403".into()),
        Some("gpt-5"),
        Some(rua_core::AgentCollaborationMode::Execute),
    );

    assert_eq!(request.chat_id, "00000000-0000-4000-8000-000000000403");
    assert_eq!(
        request.body,
        json!({
            "agent_id": "code",
            "project_id": "photos",
            "chat_id": "00000000-0000-4000-8000-000000000403",
            "message": "inspect this repository",
            "model": "gpt-5",
            "collaboration_mode": "execute"
        })
    );
}

#[test]
fn project_chat_request_generates_uuid_when_chat_id_is_omitted() {
    let request = build_project_chat_request("hello", "default", "photos", None, None, None);

    assert!(uuid::Uuid::parse_str(&request.chat_id).is_ok());
    assert_eq!(request.body["chat_id"], request.chat_id);
}
