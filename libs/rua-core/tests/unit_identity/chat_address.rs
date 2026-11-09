use std::collections::{HashMap, HashSet};

use rua_core::{ChatAddress, ChatId, ChatScope, ProjectId};

#[test]
fn agent_and_project_addresses_with_the_same_chat_id_are_distinct() {
    let agent = ChatAddress::agent("agent-a", "shared-chat");
    let project = ChatAddress::project("project-a", "shared-chat");

    assert_ne!(agent, project);

    let mut addresses = HashSet::new();
    addresses.insert(agent.clone());
    addresses.insert(project.clone());
    assert_eq!(addresses.len(), 2);

    let mut values = HashMap::new();
    values.insert(agent, "agent");
    values.insert(project, "project");
    assert_eq!(values.len(), 2);
}

#[test]
fn address_round_trip_preserves_unknown_fields_without_changing_identity() {
    let value = serde_json::json!({
        "scope": {
            "kind": "agent",
            "agent_id": "agent-a",
            "future_scope_field": { "enabled": true }
        },
        "chat_id": "chat-a",
        "future_address_field": ["future"]
    });

    let address: ChatAddress = serde_json::from_value(value.clone()).expect("deserialize address");
    assert_eq!(address, ChatAddress::agent("agent-a", "chat-a"));
    assert_eq!(
        serde_json::to_value(address).expect("serialize address"),
        value
    );
}

#[test]
fn typed_accessors_expose_scope_owner_and_chat_id() {
    let agent = ChatAddress::agent("agent-a", "chat-a");
    assert_eq!(agent.chat_id(), &ChatId::new("chat-a"));
    assert_eq!(agent.agent_id().map(|id| id.as_str()), Some("agent-a"));
    assert_eq!(agent.project_id(), None);
    assert_eq!(agent.scope().kind(), "agent");
    assert_eq!(agent.scope().owner_id(), "agent-a");
    assert!(matches!(agent.scope(), ChatScope::Agent { .. }));

    let project = ChatAddress::project("project-a", "chat-a");
    assert_eq!(project.chat_id(), &ChatId::new("chat-a"));
    assert_eq!(project.agent_id(), None);
    assert_eq!(project.project_id(), Some(&ProjectId::new("project-a")));
    assert_eq!(project.scope().kind(), "project");
    assert_eq!(project.scope().owner_id(), "project-a");
    assert!(matches!(project.scope(), ChatScope::Project { .. }));
}
