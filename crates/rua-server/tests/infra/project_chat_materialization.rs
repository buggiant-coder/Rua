use rua_chat::{ChatService, TurnAgentAssignment, TURN_AGENT_ASSIGNMENT_META_KEY, TURN_MODEL_KEY};
use rua_core::{ChatAddress, ChatScopeStore};
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot,
};
use rua_server::services::console_chat::turn_agent_assignment_from_agent;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationDriverKind, ConversationInputAcceptanceMode,
    ConversationInputPayload, FirstConversationSubmissionPersistenceContext,
};
use rua_server::services::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
    ConversationAuthorityPlacement, CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, ChatHookConfiguration, ProductHookService,
    NATIVE_TERMINAL_DEFINITION_ID,
};
use rua_server::services::project_chat::{
    materialize_first_project_turn, materialize_project_chat_draft, project_chat_agent_state,
    selection_from_assignment, switch_project_chat_agent,
    switch_project_chat_agent_with_configuration, ProjectAgentSwitchBlocker,
    ProjectChatMaterializationSnapshot, PROJECT_AGENT_SWITCH_EVENTS_META_KEY,
};
use rua_server::services::scope_composition::{HookBinding, SelectionSource};
use rua_session::WorkspaceSessionStore;

fn configuration(running_config: rua_config::ChatRunningConfig) -> ChatHookConfiguration {
    ChatHookConfiguration {
        epoch: format!("config-{}", uuid::Uuid::new_v4()),
        running_config,
        security_config: Default::default(),
        agent_assignment: None,
        prompt_file_content: None,
        tool_config_snapshot: Default::default(),
        capability_configuration: None,
        selected_agent_id: "default".into(),
        model: None,
        thinking: None,
        collaboration_mode: serde_json::json!("default"),
        harness_configuration: None,
        authority_device_id: Some("resource-device".into()),
        definition_revisions: vec![],
        effective_hooks: vec![],
        warnings: vec![],
        execution_requirements: vec![],
        extra: Default::default(),
    }
}

#[tokio::test]
async fn project_chat_draft_materializes_scoped_metadata_without_a_turn() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "chat-acp-draft");
    let scope_root = ChatScopeStore::resolve(temp.path(), address.clone())
        .root()
        .to_path_buf();
    let placement = placement("resource-device");
    let selection = selection_from_assignment(&assignment());
    let running_config = rua_config::ChatRunningConfig {
        max_iters: 17,
        ..Default::default()
    };

    materialize_project_chat_draft(
        &scope_root,
        &address,
        &selection,
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(running_config.clone()),
        },
    )
    .await
    .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.agent_id, selection.agent_id);
    assert_eq!(header.turn_index.latest_turn, 0);
    assert_eq!(header.message_log.latest_sequence, 0);
    assert_eq!(header.extra["selected_agent_id"], selection.agent_id);
    assert_eq!(
        store
            .load_chat_running_config(address.chat_id().as_str())
            .await
            .unwrap(),
        Some(running_config)
    );
    assert_eq!(
        header.extra["chat_policy"]["agent_assignment"],
        "selectable"
    );
    assert_eq!(header.extra["chat_policy"]["resource"], "project_binding");
    assert_eq!(
        header.extra["chat_policy"]["authority_placement"],
        "resource_bound"
    );
    assert_eq!(
        header.extra["authority_placement"]["device_id"],
        "resource-device"
    );
    let indexed = rua_server::services::chat_index::get(&scope_root, address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(indexed.id, address.chat_id().as_str());
    assert_eq!(
        indexed.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        serde_json::to_value(&placement).unwrap()
    );

    ChatService::for_resolved_scope(scope_root.clone(), address.clone())
        .replace_placeholder_title(&address, "First project input")
        .await
        .unwrap();
    let renamed_header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed_header.metadata.title, "First project input");
    assert_eq!(renamed_header.turn_index.latest_turn, 0);
    let renamed_index =
        rua_server::services::chat_index::get(&scope_root, address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(renamed_index.name, "First project input");

    let error = materialize_project_chat_draft(
        &scope_root,
        &address,
        &selection,
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(rua_config::ChatRunningConfig::default()),
        },
    )
    .await
    .unwrap_err();
    assert!(error.contains("already exists"));
}

fn placement(device_id: &str) -> ConversationAuthorityPlacement {
    ConversationAuthorityPlacement {
        device_id: device_id.to_string(),
        authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [7; 8]),
        base_manifest_hash: format!("sha256:{}", "1".repeat(64)),
    }
}

fn first_turn_input(placement: &ConversationAuthorityPlacement) -> ConversationInputPayload {
    let mut extra = serde_json::Map::new();
    extra.insert(
        "device_delivery".to_string(),
        serde_json::json!({
            "device_delivery_id": "delivery-1",
            "device_delivery_sequence": 1,
            "source_device_id": "source-device",
            "target_device_id": placement.device_id,
        }),
    );
    ConversationInputPayload {
        message: "Inspect this repository".to_string(),
        model_override: None,
        input: Some(vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "Inspect this repository"},
                {
                    "type": "image",
                    "image_url": "/api/files/project-diagram.png",
                    "mime_type": "image/png",
                    "name": "project-diagram.png"
                }
            ]
        })]),
        messages: None,
        extra,
        attachments: Vec::new(),
        enqueued_at: std::time::Instant::now(),
    }
}

fn persistence_context() -> FirstConversationSubmissionPersistenceContext {
    FirstConversationSubmissionPersistenceContext {
        turn_id: "turn-first".to_string(),
        input_id: "input-first".to_string(),
        submission_id: "submission-1".to_string(),
        mode: ConversationInputAcceptanceMode::Started,
    }
}

fn assignment() -> TurnAgentAssignment {
    TurnAgentAssignment {
        agent_id: "default".to_string(),
        agent_name: "Default Agent".to_string(),
        assigned_at: "2026-08-01T00:00:00Z".to_string(),
        system_prompt: Some("Default system prompt".to_string()),
        system_prompt_files: vec!["SYSTEM.md".to_string()],
        disabled_tools: vec!["Bash".to_string()],
    }
}

fn agent_detail(id: &str, name: &str) -> rua_server::endpoints::dto::AgentDetail {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "enabled": true,
        "workspace_dir": format!("/agents/{id}"),
        "system_prompt_files": ["SYSTEM.md"],
        "systemPrompt": "Code carefully",
        "tools": {
            "builtin_tools": {
                "Bash": {"enabled": false},
                "Read": {"enabled": true}
            }
        },
        "mcp": {
            "clients": {
                "private": {
                    "api_key": "secret-value",
                    "disabled_tools": ["danger"],
                },
            },
        },
    }))
    .unwrap()
}

#[test]
fn turn_agent_assignment_keeps_only_compact_execution_inputs() {
    let assignment = turn_agent_assignment_from_agent(&agent_detail("code", "Code Agent")).unwrap();

    assert_eq!(assignment.agent_id, "code");
    assert_eq!(assignment.agent_name, "Code Agent");
    assert_eq!(assignment.system_prompt.as_deref(), Some("Code carefully"));
    assert_eq!(assignment.system_prompt_files, vec!["SYSTEM.md"]);
    assert_eq!(
        assignment.disabled_tools,
        vec!["Bash", "mcp__private__danger"]
    );
    let persisted = serde_json::to_value(assignment).unwrap();
    assert!(persisted.get("configuration").is_none());
    assert!(persisted.get("default_model").is_none());
    assert!(persisted.get("channels").is_none());
    assert!(persisted.get("mcp").is_none());
}

async fn materialize(
    data_dir: &std::path::Path,
    address: &ChatAddress,
    placement: &ConversationAuthorityPlacement,
) {
    let scope = ChatScopeStore::resolve(data_dir, address.clone());
    materialize_first_project_turn(
        scope.root(),
        address,
        &assignment(),
        &first_turn_input(placement),
        &persistence_context(),
        placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(rua_config::ChatRunningConfig::default()),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn first_project_turn_commits_complete_project_owned_storage() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000401");
    let placement = placement("resource-device");
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    let mut initial_configuration = configuration(rua_config::ChatRunningConfig::default());
    initial_configuration.model = Some(serde_json::json!({"provider":"openai","model":"gpt-4.1"}));

    materialize_first_project_turn(
        scope.root(),
        &address,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: Some(serde_json::json!({
                "provider": "openai",
                "model": "gpt-4.1"
            })),
            configuration: initial_configuration,
        },
    )
    .await
    .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.agent_id, "default");
    assert_eq!(
        header
            .extra
            .get(rua_server::services::conversation_authority_placement::CHAT_MODEL_META_KEY,),
        Some(&serde_json::json!({
            "provider": "openai",
            "model": "gpt-4.1"
        }))
    );
    assert_eq!(header.turn_index.latest_turn, 1);
    assert_eq!(header.message_log.latest_sequence, 1);

    let turns = store
        .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-first");
    assert_eq!(
        turns[0].metadata[TURN_AGENT_ASSIGNMENT_META_KEY]["agent_id"],
        "default"
    );
    let messages = store
        .read_turn_messages(address.chat_id().as_str(), "turn-first")
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].content,
        serde_json::json!([
            {"type": "text", "text": "Inspect this repository"},
            {
                "type": "image",
                "url": "/api/files/project-diagram.png",
                "media_type": "image/png",
                "alt": "project-diagram.png"
            }
        ])
    );
    assert_eq!(
        messages[0]
            .metadata
            .get("input_id")
            .and_then(serde_json::Value::as_str),
        Some("input-first")
    );
    assert_eq!(
        messages[0]
            .metadata
            .get("device_delivery_id")
            .and_then(serde_json::Value::as_str),
        Some("delivery-1")
    );

    let indexed = rua_server::services::chat_index::get(scope.root(), address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parse_conversation_authority_placement(&indexed.meta).unwrap(),
        placement
    );
    assert!(!temp.path().join("workspaces/default").exists());
}

#[tokio::test]
async fn project_chat_user_message_branch_points_support_empty_conversations() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000430");
    let placement = placement("resource-device");
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    materialize(temp.path(), &address, &placement).await;
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let running_config = rua_config::ChatRunningConfig {
        max_iters: 23,
        ..Default::default()
    };
    store
        .update_chat_running_config(address.chat_id().as_str(), running_config.clone())
        .await
        .unwrap();
    store
        .append_messages_to_turn(
            address.chat_id().as_str(),
            "turn-first",
            rua_session::TurnLifecycle {
                user_submitted_at: "2026-08-01T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-08-01T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-08-01T00:00:02Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-first".to_string()),
            vec![rua_session::NewConversationMessage {
                message_id: "assistant-first".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-08-01T00:00:02Z".to_string(),
                status: "completed".to_string(),
                content: serde_json::json!([{"type": "text", "text": "first answer"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            address.chat_id().as_str(),
            rua_session::NewConversationTurn {
                turn_id: "turn-second".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-08-01T00:01:00Z".to_string(),
                    assistant_started_at: Some("2026-08-01T00:01:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-01T00:01:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-second".to_string()),
                messages: vec![
                    rua_session::NewConversationMessage {
                        message_id: "user-second".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-08-01T00:01:00Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "second input"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    rua_session::NewConversationMessage {
                        message_id: "assistant-second".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-08-01T00:01:02Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "second answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();

    let chat_service = ChatService::for_resolved_scope(scope.root(), address.clone());
    let before_second = chat_service
        .resolve_history_boundary(&address, "turn-second")
        .await
        .unwrap();
    let branch = chat_service
        .branch_before(&address, &before_second.before_turn_id)
        .await
        .unwrap();
    assert_eq!(
        store
            .load_chat_running_config(branch.address().chat_id().as_str())
            .await
            .unwrap(),
        Some(running_config.clone())
    );
    assert_eq!(
        store
            .read_turn_range(branch.address().chat_id().as_str(), 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        1
    );

    let before_first = chat_service
        .resolve_history_boundary(&address, "turn-first")
        .await
        .unwrap();
    let empty_branch = chat_service
        .branch_before(&address, &before_first.before_turn_id)
        .await
        .unwrap();
    assert_eq!(
        store
            .load_chat_running_config(empty_branch.address().chat_id().as_str())
            .await
            .unwrap(),
        Some(running_config)
    );
    assert!(store
        .read_turn_range(empty_branch.address().chat_id().as_str(), 1, usize::MAX,)
        .await
        .unwrap()
        .is_empty());

    chat_service
        .rollback_before(&address, &before_first.before_turn_id)
        .await
        .unwrap();
    assert!(store
        .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn first_project_turn_records_override_without_replacing_chat_default() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000410");
    let placement = placement("resource-device");
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    let mut input = first_turn_input(&placement);
    input.extra.insert(
        rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY.to_string(),
        serde_json::json!("execute"),
    );

    materialize_first_project_turn(
        scope.root(),
        &address,
        &assignment(),
        &input,
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(rua_config::ChatRunningConfig::default()),
        },
    )
    .await
    .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        header
            .extra
            .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
            .and_then(serde_json::Value::as_str),
        Some("default")
    );
    let turns = store
        .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        turns[0]
            .metadata
            .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
            .and_then(serde_json::Value::as_str),
        Some("execute")
    );
}

#[tokio::test]
async fn later_project_first_candidate_uses_the_committed_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000411");
    let placement = placement("resource-device");
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    let mut winner = configuration(rua_config::ChatRunningConfig::default());
    winner.agent_assignment = Some(assignment());
    winner.model = Some(serde_json::json!({
        "provider_id": "fixture",
        "model": "winner-model"
    }));
    let mut loser = configuration(rua_config::ChatRunningConfig::default());
    loser.agent_assignment = Some(assignment());
    loser.model = Some(serde_json::json!({
        "provider_id": "fixture",
        "model": "loser-model"
    }));

    rua_server::services::project_chat::persist_first_project_submission(
        scope.root(),
        &address,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: winner.model.clone(),
            configuration: winner,
        },
    )
    .await
    .unwrap();
    let mut follower_context = persistence_context();
    follower_context.turn_id = "turn-follower".to_string();
    follower_context.input_id = "input-follower".to_string();
    follower_context.submission_id = "submission-follower".to_string();
    rua_server::services::project_chat::persist_first_project_submission(
        scope.root(),
        &address,
        &assignment(),
        &first_turn_input(&placement),
        &follower_context,
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: loser.model.clone(),
            configuration: loser,
        },
    )
    .await
    .unwrap();

    let store = WorkspaceSessionStore::new(scope.root());
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        header.extra["hook_configuration"]["model"]["model"],
        "winner-model"
    );
    let turns = store
        .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].metadata[TURN_MODEL_KEY]["model"], "winner-model");
}

#[tokio::test]
async fn equal_chat_ids_under_different_projects_do_not_share_storage() {
    let temp = tempfile::tempdir().unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000402";
    let placement = placement("resource-device");
    let first = ChatAddress::project("project-a", chat_id);
    let second = ChatAddress::project("project-b", chat_id);
    let first_scope = ChatScopeStore::resolve(temp.path(), first.clone());
    let second_scope = ChatScopeStore::resolve(temp.path(), second.clone());

    materialize_first_project_turn(
        first_scope.root(),
        &first,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(rua_config::ChatRunningConfig::default()),
        },
    )
    .await
    .unwrap();

    assert!(rua_session::WorkspaceSessionStore::new(first_scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .is_some());
    assert!(rua_session::WorkspaceSessionStore::new(second_scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .is_none());
    assert!(
        rua_server::services::chat_index::get(second_scope.root(), chat_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn chat_index_commit_failure_rolls_back_the_complete_staged_session() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000407");
    let placement = placement("resource-device");
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    std::fs::create_dir_all(scope.chat_index_path()).unwrap();

    let error = materialize_first_project_turn(
        scope.root(),
        &address,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(rua_config::ChatRunningConfig::default()),
        },
    )
    .await
    .unwrap_err();

    assert!(error.contains("sqlite") || error.contains("database"));
    assert!(!scope.evergreen_chat_root().exists());
}

#[tokio::test]
async fn failed_project_staging_cleans_draft_and_first_turn_without_publishing() {
    for draft in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let address = ChatAddress::project("project-a", "failed-staging");
        let scope = ChatScopeStore::resolve(temp.path(), address.clone());
        let placement = placement("resource-device");
        let mut invalid = configuration(Default::default());
        invalid.epoch.clear();
        let snapshot = ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: invalid,
        };
        let result = if draft {
            materialize_project_chat_draft(
                scope.root(),
                &address,
                &selection_from_assignment(&assignment()),
                &placement,
                snapshot,
            )
            .await
        } else {
            materialize_first_project_turn(
                scope.root(),
                &address,
                &assignment(),
                &first_turn_input(&placement),
                &persistence_context(),
                &placement,
                snapshot,
            )
            .await
        };
        assert!(
            result.is_err(),
            "invalid snapshot must fail before publication"
        );
        assert!(!scope.evergreen_chat_root().exists());
        assert!(
            rua_server::services::chat_index::get(scope.root(), address.chat_id().as_str())
                .await
                .unwrap()
                .is_none()
        );
        let staging = scope.root().join(".materializing");
        if staging.exists() {
            assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
        }
    }
}

#[tokio::test]
async fn existing_project_storage_is_preserved_by_both_creation_paths() {
    for draft in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let address = ChatAddress::project("project-a", "existing-storage");
        let scope = ChatScopeStore::resolve(temp.path(), address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let chat_dir = store.chat_dir(address.chat_id().as_str());
        std::fs::create_dir_all(&chat_dir).unwrap();
        let sentinel = chat_dir.join("user-content");
        std::fs::write(&sentinel, "preserve").unwrap();
        let placement = placement("resource-device");
        let snapshot = ProjectChatMaterializationSnapshot {
            initial_model: None,
            configuration: configuration(Default::default()),
        };
        let result = if draft {
            materialize_project_chat_draft(
                scope.root(),
                &address,
                &selection_from_assignment(&assignment()),
                &placement,
                snapshot,
            )
            .await
        } else {
            materialize_first_project_turn(
                scope.root(),
                &address,
                &assignment(),
                &first_turn_input(&placement),
                &persistence_context(),
                &placement,
                snapshot,
            )
            .await
        };
        assert!(result.unwrap_err().contains("storage already exists"));
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "preserve");
        assert!(!scope.root().join(".materializing").exists());
    }
}

#[tokio::test]
async fn agent_switch_is_chat_local_and_preserves_historical_turn_assignment() {
    let temp = tempfile::tempdir().unwrap();
    let placement = placement("resource-device");
    let running = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000408");
    let idle = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000409");
    materialize(temp.path(), &running, &placement).await;
    materialize(temp.path(), &idle, &placement).await;
    let idle_scope = ChatScopeStore::resolve(temp.path(), idle.clone());
    rua_chat::ChatService::for_resolved_scope(idle_scope.root(), idle.clone())
        .update_setting(
            &idle,
            rua_chat::ChatSetting::Model(serde_json::json!({
                "provider_id": "fixture",
                "model": "fixture-model",
                "reasoning_effort": "high",
            })),
        )
        .await
        .unwrap();

    let authority = rua_server::services::conversation_authority::ConversationAuthority::new();
    let approvals = ApprovalAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: running.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;

    let running_scope = ChatScopeStore::resolve(temp.path(), running.clone());
    let running_state = switch_project_chat_agent(
        running_scope.root(),
        &running,
        "default",
        &agent_detail("code", "Code Agent"),
        "controller",
        &authority,
        &approvals,
    )
    .await
    .unwrap();
    assert_eq!(running_state.selection.agent_id, "code");
    assert!(!running_state.switchable);
    assert_eq!(
        running_state.blockers,
        vec![ProjectAgentSwitchBlocker::ActiveTurn]
    );
    let running_store = rua_session::WorkspaceSessionStore::new(running_scope.root());
    let running_turns = running_store
        .read_turn_range(running.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        running_turns[0].metadata[TURN_AGENT_ASSIGNMENT_META_KEY]["agent_id"],
        "default"
    );
    let running_header = running_store
        .load_chat_header(running.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running_header.extra["selected_agent_id"], "code");

    let state = switch_project_chat_agent(
        idle_scope.root(),
        &idle,
        "default",
        &agent_detail("code", "Code Agent"),
        "controller",
        &authority,
        &approvals,
    )
    .await
    .unwrap();
    assert_eq!(state.selection.agent_id, "code");
    assert_eq!(state.selection.revision, 2);
    let reloaded = rua_server::services::project_chat::load_project_chat_agent_selection(
        idle_scope.root(),
        idle.chat_id().as_str(),
    )
    .await
    .unwrap();
    assert_eq!(reloaded, state.selection);

    let store = rua_session::WorkspaceSessionStore::new(idle_scope.root());
    let header = store
        .load_chat_header(idle.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        header.extra[PROJECT_AGENT_SWITCH_EVENTS_META_KEY][0]["previous_agent_id"],
        "default"
    );
    assert_eq!(
        header.extra[PROJECT_AGENT_SWITCH_EVENTS_META_KEY][0]["selected_agent_id"],
        "code"
    );
    assert_eq!(header.extra["selected_agent_id"], "code");
    assert_eq!(header.extra["model"]["model"], "fixture-model");
    assert_eq!(header.extra["model"]["reasoning_effort"], "high");
    let second = switch_project_chat_agent(
        idle_scope.root(),
        &idle,
        "code",
        &agent_detail("default", "Default Agent"),
        "controller",
        &authority,
        &approvals,
    )
    .await
    .unwrap();
    assert_eq!(second.selection.revision, 3);
    let reloaded = rua_server::services::project_chat::load_project_chat_agent_selection(
        idle_scope.root(),
        idle.chat_id().as_str(),
    )
    .await
    .unwrap();
    assert_eq!(reloaded, second.selection);
    let turns = store
        .read_turn_range(idle.chat_id().as_str(), 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        turns[0].metadata[TURN_AGENT_ASSIGNMENT_META_KEY]["agent_id"],
        "default"
    );
}

#[tokio::test]
async fn project_agent_switch_replaces_stale_agent_hook_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let placement = placement("resource-device");
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000411");
    materialize(temp.path(), &address, &placement).await;
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    let hooks = ProductHookService::new();
    hooks
        .set_agent_activation("default", NATIVE_TERMINAL_DEFINITION_ID, true)
        .await
        .unwrap();
    hooks
        .set_project_bindings(
            "project-a",
            vec![HookBinding {
                definition_revision: "project:project-a/project-only@v1".to_string(),
                event: "turn.terminal".to_string(),
                source: SelectionSource::Project,
                configured_order: 1,
                parameters: Default::default(),
                before: Vec::new(),
                after: Vec::new(),
                requires: Vec::new(),
            }],
        )
        .await;
    let materialization = |agent_id: &str| ChatConfigurationMaterialization {
        selected_agent_id: agent_id.to_string(),
        running_config: Default::default(),
        security_config: Default::default(),
        agent_assignment: None,
        prompt_file_content: None,
        tool_config_snapshot: Default::default(),
        capability_configuration: None,
        model: Some(serde_json::json!({"provider_id": "fixture", "model": "stable"})),
        thinking: Some(serde_json::json!({"thinking_strength": "high"})),
        collaboration_mode: serde_json::json!("execute"),
        harness_configuration: Some(serde_json::json!({"mode": "strict"})),
        authority_device_id: placement.device_id.clone(),
    };
    let previous = hooks
        .resolve_complete_chat_configuration(&address, materialization("default"))
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&address, previous.clone())
        .await;
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let mut header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    header.extra.insert(
        "hook_configuration".to_string(),
        serde_json::to_value(&previous).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();

    let candidate = hooks
        .resolve_complete_chat_configuration(&address, materialization("code"))
        .await
        .unwrap();
    assert_eq!(
        candidate.definition_revisions,
        vec!["project:project-a/project-only@v1"]
    );
    assert_eq!(
        hooks.chat_configuration(&address).await,
        Some(previous.clone())
    );

    switch_project_chat_agent_with_configuration(
        scope.root(),
        &address,
        "default",
        &agent_detail("code", "Code Agent"),
        "controller",
        &rua_server::services::conversation_authority::ConversationAuthority::new(),
        &ApprovalAuthority::new(),
        Some(&candidate),
    )
    .await
    .unwrap();
    hooks
        .adopt_chat_configuration(&address, candidate.clone())
        .await;

    let switched = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let persisted: ChatHookConfiguration =
        serde_json::from_value(switched.extra["hook_configuration"].clone()).unwrap();
    assert_eq!(persisted, candidate);
    assert_eq!(persisted.selected_agent_id, "code");
    assert!(!persisted
        .definition_revisions
        .iter()
        .any(|revision| revision == &format!("{NATIVE_TERMINAL_DEFINITION_ID}@v1")));
    assert_eq!(hooks.chat_configuration(&address).await, Some(persisted));
}

#[tokio::test]
async fn agent_switch_boundary_reports_every_chat_local_blocker() {
    let temp = tempfile::tempdir().unwrap();
    let placement = placement("resource-device");
    let address = ChatAddress::project("project-a", "00000000-0000-4000-8000-000000000410");
    materialize(temp.path(), &address, &placement).await;
    let scope = ChatScopeStore::resolve(temp.path(), address.clone());
    let authority = rua_server::services::conversation_authority::ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "queued".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();

    let approvals = ApprovalAuthority::new();
    approvals
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-active".to_string(),
            tool_call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash".to_string(),
                args: serde_json::json!({"command": "pwd"}),
            },
        })
        .await;
    rua_tool::extended::ask_user::AskUserService::global()
        .create_pending(address.chat_id().as_str(), Vec::new())
        .await;
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let mut header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    header.extra.insert(
        "acp_binding".to_string(),
        serde_json::json!({"runner": "codex", "device_id": "resource-device"}),
    );
    store.save_chat_header(&header).await.unwrap();

    let state = project_chat_agent_state(scope.root(), &address, true, &authority, &approvals)
        .await
        .unwrap();
    assert!(!state.switchable);
    assert_eq!(
        state.blockers,
        vec![
            ProjectAgentSwitchBlocker::ActiveTurn,
            ProjectAgentSwitchBlocker::QueuedInput,
            ProjectAgentSwitchBlocker::UnresolvedApproval,
            ProjectAgentSwitchBlocker::UnansweredPrompt,
            ProjectAgentSwitchBlocker::ActiveAcpBinding,
        ]
    );

    rua_tool::extended::ask_user::AskUserService::global()
        .clear_by_session(address.chat_id().as_str())
        .await;
}

#[tokio::test]
async fn concurrent_project_chats_keep_turn_queue_and_approval_state_isolated() {
    let placement = placement("resource-device");
    let first = ChatAddress::project("project-concurrent", "00000000-0000-4000-8000-000000000414");
    let second = ChatAddress::project("project-concurrent", "00000000-0000-4000-8000-000000000415");
    let authority = rua_server::services::conversation_authority::ConversationAuthority::new();
    for address in [&first, &second] {
        authority
            .start_active_turn(ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: placement.device_id.clone(),
                authority_epoch: placement.authority_epoch,
                driver_detail: None,
            })
            .await;
    }
    authority
        .enqueue_input(
            &first,
            ConversationInputPayload {
                message: "queued only on first".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();
    let approvals = ApprovalAuthority::new();
    approvals
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(first.clone()),
            turn_id: "turn-first".to_string(),
            tool_call_id: "call-first".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash".to_string(),
                args: serde_json::json!({"command": "pwd"}),
            },
        })
        .await;

    assert!(authority.active_turn(&first).await.is_some());
    assert!(authority.active_turn(&second).await.is_some());
    assert_eq!(authority.queue_snapshot(&first).await.len(), 1);
    assert!(authority.queue_snapshot(&second).await.is_empty());
    assert_eq!(approvals.list_pending_for_address(&first).await.len(), 1);
    assert!(approvals.list_pending_for_address(&second).await.is_empty());

    authority.clear_queue(&first).await;
    approvals.clear_by_address(&first).await;
    authority.finish_active_turn(&first).await;
    assert!(authority.active_turn(&first).await.is_none());
    assert!(authority.active_turn(&second).await.is_some());
    authority.finish_active_turn(&second).await;
}

#[test]
fn persisted_project_directory_survives_primary_change_and_missing_default() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir(&old).unwrap();
    std::fs::create_dir(&new).unwrap();
    let resources = rua_project::ProjectResources::create(&[old.clone(), new.clone()], 1).unwrap();
    let binding = rua_project::ProjectResourceBinding {
        project_id: "project".into(),
        resource_device_id: "device".into(),
        resources,
        created_at: chrono::Utc::now(),
        extra: Default::default(),
    };
    let header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: "chat".into(),
        agent_id: "default".into(),
        title: "Chat".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_activity_at: chrono::Utc::now().to_rfc3339(),
        pinned: false,
        archived: false,
        preview: None,
    });
    assert_eq!(
        rua_server::services::project_chat::working_directory(&binding, Some(&header), None)
            .unwrap(),
        old
    );
    assert_eq!(
        rua_server::services::project_chat::working_directory(&binding, None, None).unwrap(),
        new
    );
    std::fs::remove_dir(&new).unwrap();
    assert!(
        rua_server::services::project_chat::working_directory(&binding, Some(&header), None)
            .is_ok()
    );
    assert!(rua_server::services::project_chat::working_directory(&binding, None, None).is_err());
}
