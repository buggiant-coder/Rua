use rua_chat::{
    AgentAssignmentPolicy, AuthorityPlacementPolicy, ChatHistoryQuery, ChatListQuery,
    ChatListUpdate, ChatPolicy, ChatSearchQuery, ChatService, ChatSetting, ResourcePolicy,
    TurnAgentAssignment,
};
use rua_core::{ChatAddress, ChatScopeStore};
use rua_session::{
    chat_index, ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn,
    TurnLifecycle, WorkspaceSessionStore,
};

fn projection_header() -> ChatSessionHeader {
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: "projection".into(),
        agent_id: "agent-a".into(),
        title: "Session title".into(),
        created_at: "2026-08-20T01:00:00+01:00".into(),
        last_activity_at: "2026-08-21T00:00:00Z".into(),
        pinned: true,
        archived: true,
        preview: None,
    });
    header
        .extra
        .insert("extension".into(), serde_json::json!({"current": true}));
    header
}

#[test]
fn header_projection_preserves_listing_fields_and_replaces_session_fields() {
    let header = projection_header();
    let mut previous = rua_chat::project_chat_header(&header, None).unwrap();
    previous.user_id = "owner".into();
    previous.channel = "voice".into();
    previous.status = "running".into();
    previous.pin_order = 7;
    previous.name = "stale title".into();
    previous.pinned = false;
    previous.archived = false;
    previous.meta = serde_json::json!({"extension": "stale", "listing_extension":42, "collaboration": {"stale":true}});
    let projected = rua_chat::project_chat_header(&header, Some(previous)).unwrap();
    assert_eq!(projected.name, "Session title");
    assert!(projected.pinned && projected.archived);
    assert_eq!(projected.user_id, "owner");
    assert_eq!(projected.channel, "voice");
    assert_eq!(projected.status, "running");
    assert_eq!(projected.pin_order, 7);
    assert_eq!(projected.meta["extension"], header.extra["extension"]);
    assert_eq!(projected.meta["listing_extension"], 42);
    assert!(projected.meta.get("collaboration").is_none());
    assert_eq!(
        projected.created_at.to_rfc3339(),
        "2026-08-20T00:00:00+00:00"
    );
}

#[test]
fn header_projection_rejects_invalid_timestamps_and_index_metadata() {
    for creation in [true, false] {
        let mut header = projection_header();
        if creation {
            header.metadata.created_at = "invalid".into();
        } else {
            header.metadata.last_activity_at = "invalid".into();
        }
        assert!(rua_chat::project_chat_header(&header, None).is_err());
    }
    let header = projection_header();
    let mut previous = rua_chat::project_chat_header(&header, None).unwrap();
    previous.meta = serde_json::json!([]);
    assert!(rua_chat::project_chat_header(&header, Some(previous)).is_err());
}

#[tokio::test]
async fn existing_agent_chat_loads_scope_policy_without_recreating_storage() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4359");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Existing chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Session File");

    let session_path = scope_store.evergreen_chat_root().join("session.json");
    let original_bytes = tokio::fs::read(&session_path)
        .await
        .expect("read seeded Session File");

    let chat = ChatService::new(root.path())
        .load(&address)
        .await
        .expect("load existing Chat")
        .expect("existing Chat");

    assert_eq!(chat.address(), &address);
    assert_eq!(
        chat.policy(),
        &ChatPolicy {
            agent_assignment: AgentAssignmentPolicy::Fixed,
            resource: ResourcePolicy::None,
            authority_placement: AuthorityPlacementPolicy::Selectable,
        }
    );
    assert_eq!(
        scope_store.evergreen_chat_root(),
        session_path.parent().unwrap()
    );
    assert_eq!(
        tokio::fs::read(&session_path)
            .await
            .expect("read Session File after load"),
        original_bytes
    );
}

#[tokio::test]
async fn existing_project_chat_loads_project_scope_policy() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f435a");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Existing project chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Project Session File");

    let chat = ChatService::new(root.path())
        .load(&address)
        .await
        .expect("load existing Project Chat")
        .expect("existing Project Chat");

    assert_eq!(
        chat.policy(),
        &ChatPolicy {
            agent_assignment: AgentAssignmentPolicy::Selectable,
            resource: ResourcePolicy::ProjectBinding,
            authority_placement: AuthorityPlacementPolicy::ResourceBound,
        }
    );
    assert_eq!(
        scope_store.evergreen_chat_root(),
        scope_store
            .root()
            .join("sessions")
            .join(address.chat_id().as_str())
    );
}

#[tokio::test]
async fn model_update_writes_only_session_file_and_preserves_unknown_fields() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4360");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Existing chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "future_field".to_string(),
        serde_json::json!({"keep": true}),
    );
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Session File");
    tokio::fs::create_dir_all(scope_store.root())
        .await
        .expect("create Scope root");
    tokio::fs::write(scope_store.chat_index_path(), b"index-sentinel")
        .await
        .expect("seed Chat Index sentinel");

    let model = serde_json::json!({
        "provider_id": "openai",
        "model": "gpt-5",
        "thinking_strength": "high"
    });
    let chat = ChatService::new(root.path())
        .update_setting(&address, ChatSetting::Model(model.clone()))
        .await
        .expect("update Chat model");

    assert_eq!(chat.model(), Some(&model));
    assert_eq!(
        tokio::fs::read(scope_store.chat_index_path())
            .await
            .expect("read Chat Index sentinel"),
        b"index-sentinel"
    );
    let persisted = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("load updated Session File")
        .expect("updated Session File");
    assert_eq!(persisted.extra.get("model"), Some(&model));
    assert_eq!(
        persisted.extra.get("future_field"),
        Some(&serde_json::json!({"keep": true}))
    );
}

#[tokio::test]
async fn model_selection_uses_current_configuration_for_both_chat_scopes() {
    let root = tempfile::tempdir().unwrap();
    let service = ChatService::new(root.path());
    for address in [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4370"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4371"),
    ] {
        seed_two_turn_chat(root.path(), &address).await;
        let scope = ChatScopeStore::resolve(root.path(), address.clone());
        let store = WorkspaceSessionStore::new(scope.root());
        let chat_id = address.chat_id().as_str();
        let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
        header.extra.insert(
            "model".into(),
            serde_json::json!({"provider_id": "fixture", "model": "stale-header"}),
        );
        header.extra.insert(
            "thinking".into(),
            serde_json::json!({"thinking_strength": "low"}),
        );
        let original_configuration = serde_json::json!({
            "epoch": "configuration-original",
            "running_config": {},
            "model": {"provider_id": "fixture", "model": "original", "thinking_strength": "medium"},
            "thinking": {"thinking_strength": "high"},
        });
        header
            .extra
            .insert("hook_configuration".into(), original_configuration.clone());
        store.save_chat_header(&header).await.unwrap();
        let chat = service.load(&address).await.unwrap().unwrap();
        assert_eq!(chat.model().unwrap()["model"], "original");
        assert_eq!(chat.model().unwrap()["thinking_strength"], "high");
        assert_eq!(chat.thinking(), Some(&original_configuration["thinking"]));

        let mut current = original_configuration.clone();
        current["epoch"] = serde_json::json!("configuration-current");
        current["model"]["model"] = serde_json::json!("current");
        current.as_object_mut().unwrap().remove("thinking");
        store
            .replace_chat_configuration(chat_id, "configuration-original", current)
            .await
            .unwrap();
        // A late header save cannot replace the published configuration head.
        store.save_chat_header(&header).await.unwrap();
        let header_bytes = std::fs::read(store.chat_header_path(chat_id)).unwrap();
        let page = service
            .read_history(&address, ChatHistoryQuery::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.chat.model().unwrap()["model"], "current");
        assert_eq!(page.chat.model().unwrap()["thinking_strength"], "medium");
        assert_eq!(page.chat.thinking(), None);
        assert_eq!(
            std::fs::read(store.chat_header_path(chat_id)).unwrap(),
            header_bytes
        );
        assert_eq!(
            store
                .load_configuration_snapshot(chat_id, "configuration-original")
                .await
                .unwrap(),
            Some(original_configuration)
        );
    }
}

#[tokio::test]
async fn model_selection_does_not_resurrect_legacy_fields_when_configuration_has_no_model() {
    let root = tempfile::tempdir().unwrap();
    let address = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4372");
    seed_two_turn_chat(root.path(), &address).await;
    let scope = ChatScopeStore::resolve(root.path(), address.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let chat_id = address.chat_id().as_str();
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    header.extra.insert(
        "model".into(),
        serde_json::json!({"provider_id": "fixture", "model": "stale-header"}),
    );
    header.extra.insert(
        "thinking".into(),
        serde_json::json!({"thinking_strength": "high"}),
    );
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::json!({
            "epoch": "configuration-no-model", "running_config": {},
        }),
    );
    store.save_chat_header(&header).await.unwrap();
    let chat = ChatService::new(root.path())
        .load(&address)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chat.model(), None);
    assert_eq!(chat.thinking(), None);
}

#[tokio::test]
async fn non_list_settings_accumulate_in_session_file_without_touching_index() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4361");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Project chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Session File");
    tokio::fs::write(scope_store.chat_index_path(), b"index-sentinel")
        .await
        .expect("seed Chat Index sentinel");

    let service = ChatService::new(root.path());
    let policy = ChatPolicy {
        agent_assignment: AgentAssignmentPolicy::Selectable,
        resource: ResourcePolicy::ProjectBinding,
        authority_placement: AuthorityPlacementPolicy::ResourceBound,
    };
    let thinking = serde_json::json!({"strength": "high"});
    let authority = serde_json::json!({"device_id": "device-a", "authority_epoch": 7});
    let harness = serde_json::json!({"schema_version": 1, "modules": {}});
    for setting in [
        ChatSetting::SelectedAgent("agent-b".to_string()),
        ChatSetting::Thinking(thinking.clone()),
        ChatSetting::Policy(policy),
        ChatSetting::AuthorityPlacement(authority.clone()),
        ChatSetting::HarnessConfiguration(harness.clone()),
    ] {
        service
            .update_setting(&address, setting)
            .await
            .expect("update non-list Chat setting");
    }

    let chat = service
        .load(&address)
        .await
        .expect("load updated Chat")
        .expect("updated Chat");
    assert_eq!(chat.selected_agent_id(), Some("agent-b"));
    assert_eq!(chat.thinking(), Some(&thinking));
    assert_eq!(chat.policy(), &policy);
    assert_eq!(chat.authority_placement(), Some(&authority));
    assert_eq!(chat.harness_configuration(), Some(&harness));
    assert_eq!(
        tokio::fs::read(scope_store.chat_index_path())
            .await
            .expect("read Chat Index sentinel"),
        b"index-sentinel"
    );
}

#[tokio::test]
async fn agent_scoped_chat_rejects_selecting_a_different_agent() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4364");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Fixed Agent chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    rua_chat::apply_agent_scope_defaults(&mut header, "agent-a");
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed Agent-Scoped Session File");

    let error = ChatService::new(root.path())
        .update_setting(&address, ChatSetting::SelectedAgent("agent-b".to_string()))
        .await
        .expect_err("fixed Agent selection must reject another Agent");

    assert!(error.to_string().contains("fixed"));
    let persisted = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("load Session File")
        .expect("Session File");
    assert_eq!(
        persisted.extra.get("selected_agent_id"),
        Some(&serde_json::json!("agent-a"))
    );
}

#[tokio::test]
async fn rename_and_pin_commit_session_then_update_scope_index_projection() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4362");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Old title".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Session File");
    let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
        .expect("valid timestamp")
        .with_timezone(&chrono::Utc);
    chat_index::upsert(
        scope_store.root(),
        chat_index::ChatIndexRecord {
            id: address.chat_id().to_string(),
            name: "Old title".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
            meta: serde_json::json!({"preserve": true}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 11,
            tag: rua_chat::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed Scope Chat Index");

    let chat = ChatService::new(root.path())
        .update_listing(
            &address,
            ChatListUpdate {
                title: Some("New title".to_string()),
                pinned: Some(true),
                tag: Some(rua_chat::ChatTag::Yellow),
                archived: None,
            },
        )
        .await
        .expect("update list-visible Chat metadata");

    assert_eq!(chat.title(), "New title");
    assert!(chat.pinned());
    assert_eq!(chat.tag(), rua_chat::ChatTag::Yellow);
    let persisted = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("load updated Session File")
        .expect("updated Session File");
    assert_eq!(persisted.metadata.title, "New title");
    assert!(persisted.metadata.pinned);
    assert_eq!(persisted.tag, rua_chat::ChatTag::Yellow);
    let projected = chat_index::get(scope_store.root(), address.chat_id().as_str())
        .await
        .expect("read Scope Chat Index")
        .expect("projected Chat");
    assert_eq!(projected.name, "New title");
    assert!(projected.pinned);
    assert_eq!(projected.tag, rua_chat::ChatTag::Yellow);
    assert_eq!(projected.meta, serde_json::json!({"preserve": true}));
    assert_eq!(projected.pin_order, 11);
}

#[tokio::test]
async fn first_input_replaces_placeholder_title_for_every_chat_scope() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let service = ChatService::new(root.path());
    let agent = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4390");
    let project = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4391");

    for address in [&agent, &project] {
        let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
        let session_store = WorkspaceSessionStore::new(scope_store.root());
        let mut header = ChatSessionHeader::new(ChatMetadata {
            chat_id: address.chat_id().to_string(),
            agent_id: "agent-a".to_string(),
            title: "New Chat".to_string(),
            created_at: "2026-08-20T00:00:00Z".to_string(),
            last_activity_at: "2026-08-20T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        });
        if address.agent_id().is_some() {
            rua_chat::apply_agent_scope_defaults(&mut header, "agent-a");
        }
        session_store
            .save_chat_header(&header)
            .await
            .expect("seed placeholder Session File");

        let chat = service
            .replace_placeholder_title(address, "First user input")
            .await
            .expect("replace placeholder title");

        assert_eq!(chat.title(), "First user input");
        let persisted = session_store
            .load_chat_header(address.chat_id().as_str())
            .await
            .expect("load Session File")
            .expect("Session File");
        assert_eq!(persisted.metadata.title, "First user input");
        let projected = chat_index::get(scope_store.root(), address.chat_id().as_str())
            .await
            .expect("read Scope Chat Index")
            .expect("projected Chat");
        assert_eq!(projected.name, "First user input");
    }
}

#[tokio::test]
async fn failed_projection_leaves_session_committed_and_public_repair_rebuilds_index() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4363");
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "agent-a".to_string(),
        title: "Old title".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed existing Session File");
    tokio::fs::create_dir(scope_store.chat_index_path())
        .await
        .expect("occupy Chat Index path with a directory");

    let service = ChatService::new(root.path());
    let error = service
        .update_listing(
            &address,
            ChatListUpdate {
                title: Some("Committed title".to_string()),
                pinned: Some(true),
                tag: None,
                archived: None,
            },
        )
        .await
        .expect_err("projection must fail");
    assert!(error.to_string().contains("Chat Index"));
    let committed = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("load authoritative Session File")
        .expect("authoritative Session File");
    assert_eq!(committed.metadata.title, "Committed title");
    assert!(committed.metadata.pinned);

    tokio::fs::remove_dir(scope_store.chat_index_path())
        .await
        .expect("remove projection fault");
    service
        .repair_projection(&address)
        .await
        .expect("repair Chat Index from Session File");

    let repaired = chat_index::get(scope_store.root(), address.chat_id().as_str())
        .await
        .expect("read repaired Chat Index")
        .expect("repaired Chat projection");
    assert_eq!(repaired.name, "Committed title");
    assert!(repaired.pinned);
}

#[tokio::test]
async fn branch_before_has_identical_agent_and_project_scope_behavior() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4370"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4371"),
    ];
    let service = ChatService::new(root.path());

    for address in addresses {
        seed_two_turn_chat(root.path(), &address).await;

        let branch = service
            .branch_before(&address, "turn-second")
            .await
            .expect("branch Chat before second Turn");

        assert_eq!(branch.address().scope(), address.scope());
        assert_ne!(branch.address().chat_id(), address.chat_id());
        assert_eq!(branch.title(), "Parity chat (branch)");
        assert!(!branch.pinned());

        let scope_store = ChatScopeStore::resolve(root.path(), branch.address().clone());
        let turns = WorkspaceSessionStore::new(scope_store.root())
            .read_turn_range(branch.address().chat_id().as_str(), 1, usize::MAX)
            .await
            .expect("read branched Turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-first");
        assert_eq!(
            turns[0].selected_variant_id.as_deref(),
            Some("assistant-first")
        );
        let projected = chat_index::get(scope_store.root(), branch.address().chat_id().as_str())
            .await
            .expect("read branch projection")
            .expect("branch projection");
        assert_eq!(projected.meta, serde_json::json!({"preserve": true}));
        assert_eq!(projected.pin_order, 0);
        assert_eq!(projected.status, "idle");
    }
}

#[tokio::test]
async fn rollback_before_has_identical_agent_and_project_scope_behavior() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4372"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4373"),
    ];
    let service = ChatService::new(root.path());

    for address in addresses {
        seed_two_turn_chat(root.path(), &address).await;

        let rolled_back = service
            .rollback_before(&address, "turn-second")
            .await
            .expect("rollback Chat before second Turn");

        assert_eq!(rolled_back.address(), &address);
        let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
        let turns = WorkspaceSessionStore::new(scope_store.root())
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
            .expect("read rolled-back Turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-first");
        assert_eq!(
            turns[0].selected_variant_id.as_deref(),
            Some("assistant-first")
        );
        let projected = chat_index::get(scope_store.root(), address.chat_id().as_str())
            .await
            .expect("read rollback projection")
            .expect("rollback projection");
        assert_eq!(projected.name, "Parity chat");
    }
}

#[tokio::test]
async fn rollback_stopped_user_only_turn_stays_removed_after_reload() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    for address in [
        ChatAddress::agent("agent-a", "rollback-stopped"),
        ChatAddress::project("project-a", "rollback-stopped"),
    ] {
        seed_two_turn_chat(root.path(), &address).await;
        let service = ChatService::new(root.path());
        let scope = ChatScopeStore::resolve(root.path(), address.clone());
        let store = WorkspaceSessionStore::new(scope.root());
        for keep_previous in [true, false] {
            if !keep_previous {
                service
                    .rollback_before(&address, "turn-first")
                    .await
                    .unwrap();
            }
            store
                .append_conversation_turn(
                    address.chat_id().as_str(),
                    NewConversationTurn {
                        turn_id: "turn-stopped".to_string(),
                        lifecycle: TurnLifecycle {
                            user_submitted_at: "2026-08-20T00:03:00Z".to_string(),
                            assistant_started_at: None,
                            assistant_completed_at: Some("2026-08-20T00:03:01Z".to_string()),
                            status: "interrupted".to_string(),
                        },
                        selected_variant_id: None,
                        messages: vec![NewConversationMessage {
                            message_id: "user-stopped".to_string(),
                            role: "user".to_string(),
                            status: "completed".to_string(),
                            created_at: "2026-08-20T00:03:00Z".to_string(),
                            content: serde_json::json!([{"type": "text", "text": "wrong input"}]),
                            provider: None,
                            tool: None,
                            metadata: serde_json::Map::new(),
                        }],
                    },
                )
                .await
                .unwrap();
            service
                .rollback_before(&address, "turn-stopped")
                .await
                .unwrap();
            let reloaded = ChatService::new(root.path())
                .read_history(&address, ChatHistoryQuery::default())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reloaded.turns.len(), if keep_previous { 2 } else { 0 });
            assert!(reloaded
                .messages
                .iter()
                .all(|message| message.message_id != "user-stopped"));
            assert_eq!(reloaded.messages.len(), if keep_previous { 4 } else { 0 });
        }
    }
}

#[tokio::test]
async fn selected_variant_has_identical_agent_and_project_scope_behavior() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4374"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4375"),
    ];
    let service = ChatService::new(root.path());

    for address in addresses {
        seed_two_turn_chat(root.path(), &address).await;
        let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
        let session_store = WorkspaceSessionStore::new(scope_store.root());
        session_store
            .append_messages_to_turn_as_new_variant(
                address.chat_id().as_str(),
                "turn-first",
                TurnLifecycle {
                    user_submitted_at: "2026-08-20T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-20T00:00:03Z".to_string()),
                    assistant_completed_at: Some("2026-08-20T00:00:04Z".to_string()),
                    status: "completed".to_string(),
                },
                Some("assistant-regenerated".to_string()),
                vec![NewConversationMessage {
                    message_id: "assistant-regenerated".to_string(),
                    role: "assistant".to_string(),
                    created_at: "2026-08-20T00:00:04Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{
                        "type": "text",
                        "text": "regenerated answer"
                    }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            )
            .await
            .expect("append regenerated Variant");

        let updated = service
            .select_variant(&address, "turn-first", "assistant-regenerated")
            .await
            .expect("select regenerated Variant");

        assert_eq!(updated.address(), &address);
        let turns = session_store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
            .expect("read selected Variant");
        assert_eq!(
            turns[0].selected_variant_id.as_deref(),
            Some("assistant-regenerated")
        );
        assert!(
            chat_index::get(scope_store.root(), address.chat_id().as_str())
                .await
                .expect("read Variant projection")
                .is_some()
        );
    }
}

#[tokio::test]
async fn history_page_has_identical_agent_and_project_scope_behavior() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4376"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4377"),
    ];
    let service = ChatService::new(root.path());

    for address in addresses {
        seed_two_turn_chat(root.path(), &address).await;

        let page = service
            .read_history(
                &address,
                ChatHistoryQuery {
                    before_turn_ordinal: None,
                    turn_limit: Some(1),
                    around_turn_id: None,
                },
            )
            .await
            .expect("read Chat History")
            .expect("materialized Chat History");

        assert_eq!(page.chat.address(), &address);
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].turn_id, "turn-second");
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].message_id, "user-second");
        assert_eq!(page.messages[1].message_id, "assistant-second");
        assert_eq!(page.oldest_turn_ordinal, Some(2));
        assert_eq!(page.newest_turn_ordinal, Some(2));
        assert!(page.has_more_before);
        assert!(page.includes_latest_turn);
    }
}

#[tokio::test]
async fn search_has_identical_agent_and_project_scope_behavior() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4378"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4379"),
    ];
    let service = ChatService::new(root.path());

    for address in addresses {
        seed_two_turn_chat(root.path(), &address).await;

        let result = service
            .search(
                &address,
                ChatSearchQuery {
                    query: "assistant-second".to_string(),
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("search saved Chat History");

        assert_eq!(result.total, 1);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].chat_id, address.chat_id().as_str());
        assert_eq!(result.results[0].message.message_id, "assistant-second");
    }
}

#[tokio::test]
async fn list_uses_each_chat_scope_index_projection() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let addresses = [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4380"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4381"),
    ];
    let service = ChatService::new(root.path());
    for address in &addresses {
        seed_two_turn_chat(root.path(), address).await;
    }

    for address in addresses {
        let page = service
            .list(
                &address,
                ChatListQuery {
                    user_id: None,
                    channel: Some("console".to_string()),
                    search: Some("Parity".to_string()),
                    offset: 0,
                    limit: 10,
                    ..ChatListQuery::default()
                },
            )
            .await
            .expect("list Scope Chat Index");

        assert_eq!(page.total, 1);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].id, address.chat_id().as_str());
    }
}

#[tokio::test]
async fn repair_projection_does_not_rewrite_conversation_history() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let address = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4382");
    seed_two_turn_chat(root.path(), &address).await;
    let scope_store = ChatScopeStore::resolve(root.path(), address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header_before = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("read Session File before repair")
        .expect("Session File before repair");
    let turns_before = session_store
        .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .expect("read Turns before repair");
    let messages_before = session_store
        .read_message_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .expect("read Messages before repair");
    chat_index::delete(scope_store.root(), address.chat_id().as_str())
        .await
        .expect("delete stale projection");

    ChatService::new(root.path())
        .repair_projection(&address)
        .await
        .expect("repair missing projection");

    let header_after = session_store
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("read Session File after repair")
        .expect("Session File after repair");
    assert_eq!(header_after.turn_index, header_before.turn_index);
    assert_eq!(header_after.message_log, header_before.message_log);
    assert_eq!(
        session_store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
            .expect("read Turns after repair"),
        turns_before
    );
    assert_eq!(
        session_store
            .read_message_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
            .expect("read Messages after repair"),
        messages_before
    );
}

#[test]
fn media_root_is_resolved_by_common_chat_ownership() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let service = ChatService::new(root.path());
    let agent = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4383");
    let project = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4384");

    assert_eq!(
        service.media_root(&agent).unwrap(),
        root.path().join("workspaces").join("agent-a").join("media")
    );
    assert_eq!(
        service.media_root(&project).unwrap(),
        root.path().join("projects").join("project-a").join("media")
    );
}

#[tokio::test]
async fn combined_agent_and_project_lifecycle_uses_one_chat_service() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let service = ChatService::new(root.path());
    let agent = ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4385");
    let project = ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4386");
    for address in [&agent, &project] {
        seed_two_turn_chat(root.path(), address).await;
        let selected_agent = address
            .agent_id()
            .map(ToString::to_string)
            .unwrap_or_else(|| "agent-a".to_string());
        match address.scope() {
            rua_core::ChatScope::Agent { .. } => {
                service
                    .complete_agent_scope_materialization(address)
                    .await
                    .unwrap();
            }
            rua_core::ChatScope::Project { .. } => {
                let scope = ChatScopeStore::resolve(root.path(), address.clone());
                WorkspaceSessionStore::new(scope.root())
                    .update_chat_header(address.chat_id().as_str(), |header| {
                        rua_chat::apply_project_scope_defaults(header, &selected_agent);
                    })
                    .await
                    .unwrap();
            }
        }
        service
            .update_setting(
                address,
                ChatSetting::Model(serde_json::json!({
                    "provider_id": "fixture",
                    "model": "unified-model"
                })),
            )
            .await
            .unwrap();
        service
            .update_listing(
                address,
                ChatListUpdate {
                    title: Some("Unified lifecycle".to_string()),
                    pinned: Some(true),
                    tag: None,
                    archived: None,
                },
            )
            .await
            .unwrap();
        service
            .replace_placeholder_title(address, "Ignored first input")
            .await
            .unwrap();
        assert_eq!(
            service.load(address).await.unwrap().unwrap().title(),
            "Unified lifecycle"
        );
        let scope = ChatScopeStore::resolve(root.path(), address.clone());
        WorkspaceSessionStore::new(scope.root())
            .append_messages_to_turn_as_new_variant(
                address.chat_id().as_str(),
                "turn-first",
                TurnLifecycle {
                    user_submitted_at: "2026-08-20T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-20T00:00:03Z".to_string()),
                    assistant_completed_at: Some("2026-08-20T00:00:04Z".to_string()),
                    status: "completed".to_string(),
                },
                Some("assistant-unified".to_string()),
                vec![NewConversationMessage {
                    message_id: "assistant-unified".to_string(),
                    role: "assistant".to_string(),
                    created_at: "2026-08-20T00:00:04Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "unified"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            )
            .await
            .unwrap();
        service
            .select_variant(address, "turn-first", "assistant-unified")
            .await
            .unwrap();
        let branch = service.branch_before(address, "turn-second").await.unwrap();
        assert_eq!(branch.address().scope(), address.scope());
        service
            .rollback_before(address, "turn-second")
            .await
            .unwrap();
        let history = service
            .read_history(address, ChatHistoryQuery::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(history.turns.len(), 1);
        assert_eq!(
            service
                .load_turn_agent_assignment(address, "turn-first")
                .await
                .unwrap()
                .agent_id,
            selected_agent
        );
        assert!(
            service
                .search(
                    address,
                    ChatSearchQuery {
                        query: "unified".to_string(),
                        offset: 0,
                        limit: 10,
                    },
                )
                .await
                .unwrap()
                .total
                >= 1
        );
        assert!(
            service
                .list(
                    address,
                    ChatListQuery {
                        limit: 10,
                        ..ChatListQuery::default()
                    },
                )
                .await
                .unwrap()
                .total
                >= 2
        );
    }

    assert_eq!(
        service.load(&agent).await.unwrap().unwrap().policy(),
        &ChatPolicy::for_scope(agent.scope())
    );
    assert_eq!(
        service.load(&project).await.unwrap().unwrap().policy(),
        &ChatPolicy::for_scope(project.scope())
    );
    assert!(service
        .update_setting(&agent, ChatSetting::SelectedAgent("agent-b".to_string()))
        .await
        .is_err());
    assert_eq!(
        service
            .update_setting(&project, ChatSetting::SelectedAgent("agent-b".to_string()))
            .await
            .unwrap()
            .selected_agent_id(),
        Some("agent-b")
    );
    assert_ne!(
        service.storage_root(&agent).unwrap(),
        service.storage_root(&project).unwrap()
    );
}

async fn seed_two_turn_chat(rua_root: &std::path::Path, address: &ChatAddress) {
    let scope_store = ChatScopeStore::resolve(rua_root, address.clone());
    let session_store = WorkspaceSessionStore::new(scope_store.root());
    let header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: address
            .agent_id()
            .map(ToString::to_string)
            .unwrap_or_else(|| "agent-a".to_string()),
        title: "Parity chat".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:02:00Z".to_string(),
        pinned: true,
        archived: false,
        preview: None,
    });
    session_store
        .save_chat_header(&header)
        .await
        .expect("seed parity Session File");
    let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
        .expect("valid timestamp")
        .with_timezone(&chrono::Utc);
    chat_index::upsert(
        scope_store.root(),
        chat_index::ChatIndexRecord {
            id: address.chat_id().to_string(),
            name: "Parity chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
            meta: serde_json::json!({"preserve": true}),
            status: "idle".to_string(),
            pinned: true,
            pin_order: 3,
            tag: rua_chat::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed parity Chat Index");
    for (turn_id, user_id, assistant_id, minute) in [
        ("turn-first", "user-first", "assistant-first", "00"),
        ("turn-second", "user-second", "assistant-second", "01"),
    ] {
        let mut user_metadata = serde_json::Map::new();
        user_metadata.insert(
            "turn_agent_assignment".to_string(),
            serde_json::to_value(TurnAgentAssignment {
                agent_id: address
                    .agent_id()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "agent-a".to_string()),
                agent_name: "Agent A".to_string(),
                assigned_at: format!("2026-08-20T00:{minute}:00Z"),
                system_prompt: Some("Follow the project instructions.".to_string()),
                system_prompt_files: vec!["AGENTS.md".to_string()],
                disabled_tools: vec!["dangerous-tool".to_string()],
            })
            .unwrap(),
        );
        session_store
            .append_conversation_turn(
                address.chat_id().as_str(),
                NewConversationTurn {
                    turn_id: turn_id.to_string(),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: format!("2026-08-20T00:{minute}:00Z"),
                        assistant_started_at: Some(format!("2026-08-20T00:{minute}:01Z")),
                        assistant_completed_at: Some(format!("2026-08-20T00:{minute}:02Z")),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: Some(assistant_id.to_string()),
                    messages: vec![
                        NewConversationMessage {
                            message_id: user_id.to_string(),
                            role: "user".to_string(),
                            created_at: format!("2026-08-20T00:{minute}:00Z"),
                            status: "completed".to_string(),
                            content: serde_json::json!([{"type": "text", "text": turn_id}]),
                            provider: None,
                            tool: None,
                            metadata: user_metadata,
                        },
                        NewConversationMessage {
                            message_id: assistant_id.to_string(),
                            role: "assistant".to_string(),
                            created_at: format!("2026-08-20T00:{minute}:02Z"),
                            status: "completed".to_string(),
                            content: serde_json::json!([{"type": "text", "text": assistant_id}]),
                            provider: None,
                            tool: None,
                            metadata: serde_json::Map::new(),
                        },
                    ],
                },
            )
            .await
            .expect("seed parity Turn");
    }
}

#[tokio::test]
async fn history_locates_a_complete_turn_without_reading_intervening_history() {
    let root = tempfile::tempdir().expect("temporary Rua root");
    let service = ChatService::new(root.path());
    for address in [
        ChatAddress::agent("agent-a", "018f5f61-c461-7a11-b24d-8f52af0f4376"),
        ChatAddress::project("project-a", "018f5f61-c461-7a11-b24d-8f52af0f4377"),
    ] {
        seed_two_turn_chat(root.path(), &address).await;
        let page = service
            .read_history(
                &address,
                ChatHistoryQuery {
                    around_turn_id: Some("turn-first".to_string()),
                    turn_limit: Some(1),
                    ..Default::default()
                },
            )
            .await
            .expect("locate history")
            .expect("history exists");
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].turn_id, "turn-first");
        assert_eq!(page.messages.len(), 2);
        assert!(!page.includes_latest_turn);
        assert!(service
            .read_history(
                &address,
                ChatHistoryQuery {
                    around_turn_id: Some("missing".to_string()),
                    turn_limit: Some(1),
                    ..Default::default()
                }
            )
            .await
            .is_err());
    }
}
