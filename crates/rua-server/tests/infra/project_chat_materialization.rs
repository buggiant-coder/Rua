use rua_chat::{ChatService, TurnAgentAssignment, TURN_AGENT_ASSIGNMENT_META_KEY, TURN_MODEL_KEY};
use rua_core::{ChatAddress, ChatStore};
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
    chat_agent_state, materialize_first_project_turn, materialize_project_chat_draft,
    selection_from_assignment, ChatAgentSwitch, ChatAgentSwitchBlocker,
    ProjectChatMaterializationSnapshot,
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
    let address = ChatAddress::new("chat-acp-draft");
    let scope_root = ChatStore::resolve(temp.path(), address.clone())
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
            project_id: "project-a".into(),
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
    assert_eq!(header.agent_id, selection.agent_id);
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
    assert_eq!(header.extra["chat_policy"]["authority_placement"], "fixed");
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

    ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope_root.clone()))
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
            project_id: "project-a".into(),
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
    assert!(assignment.disabled_tools.is_empty());
    let persisted = serde_json::to_value(assignment).unwrap();
    assert!(persisted.get("configuration").is_none());
    assert!(persisted.get("default_model").is_none());
    assert!(persisted.get("channels").is_none());
    assert!(persisted.get("mcp").is_none());
    assert!(persisted.get("disabled_tools").is_none());
}

async fn materialize(
    data_dir: &std::path::Path,
    address: &ChatAddress,
    placement: &ConversationAuthorityPlacement,
) {
    let scope = ChatStore::resolve(data_dir, address.clone());
    materialize_first_project_turn(
        scope.root(),
        address,
        &assignment(),
        &first_turn_input(placement),
        &persistence_context(),
        placement,
        ProjectChatMaterializationSnapshot {
            project_id: "project-a".into(),
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
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000401");
    let placement = placement("resource-device");
    let scope = ChatStore::resolve(temp.path(), address.clone());
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
            project_id: "project-a".into(),
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
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000430");
    let placement = placement("resource-device");
    let scope = ChatStore::resolve(temp.path(), address.clone());
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

    let chat_service = ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()));
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
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000410");
    let placement = placement("resource-device");
    let scope = ChatStore::resolve(temp.path(), address.clone());
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
            project_id: "project-a".into(),
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
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000411");
    let placement = placement("resource-device");
    let scope = ChatStore::resolve(temp.path(), address.clone());
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
            project_id: "project-a".into(),
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
            project_id: "project-a".into(),
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
async fn chat_id_resolves_the_same_header_and_index_without_project_input() {
    let temp = tempfile::tempdir().unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000402";
    let placement = placement("resource-device");
    let first = ChatAddress::new(chat_id);
    let second = ChatAddress::new(chat_id);
    let first_scope = ChatStore::resolve(temp.path(), first.clone());
    let second_scope = ChatStore::resolve(temp.path(), second.clone());

    materialize_first_project_turn(
        first_scope.root(),
        &first,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            project_id: "project-a".into(),
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
        .is_some());
    assert!(
        rua_server::services::chat_index::get(second_scope.root(), chat_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn chat_index_commit_failure_rolls_back_the_complete_staged_session() {
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000407");
    let placement = placement("resource-device");
    let scope = ChatStore::resolve(temp.path(), address.clone());
    std::fs::create_dir_all(scope.chat_index_path()).unwrap();

    let error = materialize_first_project_turn(
        scope.root(),
        &address,
        &assignment(),
        &first_turn_input(&placement),
        &persistence_context(),
        &placement,
        ProjectChatMaterializationSnapshot {
            project_id: "project-a".into(),
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
        let address = ChatAddress::new("failed-staging");
        let scope = ChatStore::resolve(temp.path(), address.clone());
        let placement = placement("resource-device");
        let mut invalid = configuration(Default::default());
        invalid.epoch.clear();
        let snapshot = ProjectChatMaterializationSnapshot {
            project_id: "project-a".into(),
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
        let address = ChatAddress::new("existing-storage");
        let scope = ChatStore::resolve(temp.path(), address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let chat_dir = store.chat_dir(address.chat_id().as_str());
        std::fs::create_dir_all(&chat_dir).unwrap();
        let sentinel = chat_dir.join("user-content");
        std::fs::write(&sentinel, "preserve").unwrap();
        let placement = placement("resource-device");
        let snapshot = ProjectChatMaterializationSnapshot {
            project_id: "project-a".into(),
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
    let running = ChatAddress::new("00000000-0000-4000-8000-000000000408");
    let idle = ChatAddress::new("00000000-0000-4000-8000-000000000409");
    materialize(temp.path(), &running, &placement).await;
    materialize(temp.path(), &idle, &placement).await;
    let idle_scope = ChatStore::resolve(temp.path(), idle.clone());
    rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(idle_scope.root()))
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

    let running_scope = ChatStore::resolve(temp.path(), running.clone());
    let blocked = ChatAgentSwitch::begin(
        running_scope.root(),
        &running,
        "default",
        "code",
        &authority,
        &approvals,
    )
    .await;
    assert!(blocked.err().unwrap().contains("active_turn"));
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
    assert_eq!(running_header.agent_id, "default");

    let candidate = ChatHookConfiguration {
        selected_agent_id: "code".into(),
        model: Some(
            serde_json::json!({"provider_id": "fixture", "model": "fixture-model", "reasoning_effort": "high"}),
        ),
        ..configuration(Default::default())
    };
    let state = ChatAgentSwitch::begin(
        idle_scope.root(),
        &idle,
        "default",
        "code",
        &authority,
        &approvals,
    )
    .await
    .unwrap()
    .commit(&candidate, "controller")
    .await
    .unwrap();
    assert_eq!(state.selection.agent_id, "code");
    assert_eq!(state.selection.revision, 2);
    let reloaded = rua_server::services::project_chat::load_chat_agent_selection(
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
        header.extra[rua_session::AGENT_SWITCH_EVENTS_KEY][0]["previous_agent_id"],
        "default"
    );
    assert_eq!(
        header.extra[rua_session::AGENT_SWITCH_EVENTS_KEY][0]["selected_agent_id"],
        "code"
    );
    assert_eq!(header.agent_id, "code");
    assert_eq!(header.extra["model"]["model"], "fixture-model");
    assert_eq!(header.extra["model"]["reasoning_effort"], "high");
    let candidate = ChatHookConfiguration {
        selected_agent_id: "default".into(),
        model: Some(
            serde_json::json!({"provider_id": "fixture", "model": "fixture-model", "reasoning_effort": "high"}),
        ),
        ..configuration(Default::default())
    };
    let second = ChatAgentSwitch::begin(
        idle_scope.root(),
        &idle,
        "code",
        "default",
        &authority,
        &approvals,
    )
    .await
    .unwrap()
    .commit(&candidate, "controller")
    .await
    .unwrap();
    assert_eq!(second.selection.revision, 3);
    let reloaded = rua_server::services::project_chat::load_chat_agent_selection(
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
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000411");
    materialize(temp.path(), &address, &placement).await;
    let scope = ChatStore::resolve(temp.path(), address.clone());
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
        project_id: Some("project-a".into()),
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

    ChatAgentSwitch::begin(
        scope.root(),
        &address,
        "default",
        "code",
        &rua_server::services::conversation_authority::ConversationAuthority::new(),
        &ApprovalAuthority::new(),
    )
    .await
    .unwrap()
    .commit(&candidate, "controller")
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
async fn agent_switch_serializes_input_prepared_for_the_previous_agent() {
    use rua_server::services::conversation_authority::ConversationAuthority;
    use rua_server::services::foreground_operation::ForegroundOperationRegistry;
    use rua_server::streaming::{ActiveStreamRegistry, Priority, StreamKey};
    use std::sync::Arc;
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("00000000-0000-4000-8000-000000001516");
    let placement = placement("resource-device");
    materialize(temp.path(), &address, &placement).await;
    let scope = ChatStore::resolve(temp.path(), address.clone());
    let hooks = Arc::new(ProductHookService::new());
    let authority = Arc::new(ConversationAuthority::with_product_hooks(
        Arc::new(ForegroundOperationRegistry::new(false)),
        Arc::clone(&hooks),
    ));
    let old = configuration(Default::default());
    hooks.adopt_chat_configuration(&address, old.clone()).await;
    let mut pending = first_turn_input(&placement);
    pending.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(old).unwrap(),
    );
    let approvals = ApprovalAuthority::new();
    let switch = ChatAgentSwitch::begin(
        scope.root(),
        &address,
        "default",
        "code",
        &authority,
        &approvals,
    )
    .await
    .unwrap();
    let streams = ActiveStreamRegistry::new();
    let key = StreamKey::new("console", address.clone(), Priority::Normal);
    let input = authority.accept_submission(
        &streams,
        &key,
        ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        },
        pending,
        None,
        None,
    );
    tokio::pin!(input);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut input)
            .await
            .is_err()
    );
    let candidate = ChatHookConfiguration {
        selected_agent_id: "code".into(),
        ..configuration(Default::default())
    };
    switch.commit(&candidate, "controller").await.unwrap();
    hooks.adopt_chat_configuration(&address, candidate).await;
    drop(switch);
    assert!(input
        .await
        .err()
        .unwrap()
        .contains("Agent selection changed"));
    assert!(authority.active_turn(&address).await.is_none());
    assert!(authority.queue_snapshot(&address).await.is_empty());
}

#[tokio::test]
async fn idle_acp_and_collaboration_each_reject_an_agent_switch_without_changing_the_chat() {
    use rua_server::services::conversation_authority::ConversationAuthority;
    let dir = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("00000000-0000-4000-8000-000000001517");
    let placement = placement("resource-device");
    materialize(dir.path(), &address, &placement).await;
    let scope = ChatStore::resolve(dir.path(), address.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let baseline = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let authority = ConversationAuthority::new();
    let approvals = ApprovalAuthority::new();
    for (key, value, blocker) in [
        (
            "acp_binding",
            serde_json::json!({"runner":"codex", "device_id":"resource-device"}),
            "active_acp_binding",
        ),
        (
            rua_session::collaboration::COLLABORATION_META_KEY,
            serde_json::json!({"executor":{"kind":"agent","agent_id":"default"}}),
            "fixed_collaboration_executor",
        ),
    ] {
        let mut header = baseline.clone();
        header.extra.insert(key.to_owned(), value.clone());
        store.save_chat_header(&header).await.unwrap();
        let error = ChatAgentSwitch::begin(
            scope.root(),
            &address,
            "default",
            "code",
            &authority,
            &approvals,
        )
        .await
        .err()
        .unwrap();
        assert!(error.contains(blocker), "{error}");
        let saved = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.agent_id, baseline.agent_id);
        assert_eq!(
            saved.extra["hook_configuration"],
            baseline.extra["hook_configuration"]
        );
        assert_eq!(saved.extra[key], value);
    }
}

#[tokio::test]
async fn agent_switch_boundary_reports_every_chat_local_blocker() {
    let temp = tempfile::tempdir().unwrap();
    let placement = placement("resource-device");
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000410");
    materialize(temp.path(), &address, &placement).await;
    let scope = ChatStore::resolve(temp.path(), address.clone());
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

    let state = chat_agent_state(scope.root(), &address, true, &authority, &approvals)
        .await
        .unwrap();
    assert!(!state.switchable);
    assert_eq!(
        state.blockers,
        vec![
            ChatAgentSwitchBlocker::ActiveTurn,
            ChatAgentSwitchBlocker::QueuedInput,
            ChatAgentSwitchBlocker::UnresolvedApproval,
            ChatAgentSwitchBlocker::UnansweredPrompt,
            ChatAgentSwitchBlocker::ActiveAcpBinding,
        ]
    );

    let error = ChatAgentSwitch::begin(
        scope.root(),
        &address,
        "default",
        "code",
        &authority,
        &approvals,
    )
    .await
    .err()
    .expect("blocked switch");
    for blocker in state.blockers {
        assert!(
            error.contains(serde_json::to_value(blocker).unwrap().as_str().unwrap()),
            "{error}"
        );
    }

    rua_tool::extended::ask_user::AskUserService::global()
        .clear_by_session(address.chat_id().as_str())
        .await;
}

#[tokio::test]
async fn concurrent_project_chats_keep_turn_queue_and_approval_state_isolated() {
    let placement = placement("resource-device");
    let first = ChatAddress::new("00000000-0000-4000-8000-000000000414");
    let second = ChatAddress::new("00000000-0000-4000-8000-000000000415");
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

/// Exposes the actual constructed Agent request at the model boundary and lets
/// the test choose exactly when the Turn ends.
struct ProjectTurnRuntime(
    tokio::sync::mpsc::UnboundedSender<(
        rua_contract::agent::AgentRequest,
        tokio::sync::oneshot::Sender<()>,
    )>,
);

#[async_trait::async_trait]
impl rua_contract::AgentRuntime for ProjectTurnRuntime {
    async fn stream(
        &self,
        req: rua_contract::agent::AgentRequest,
    ) -> rua_core::Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = rua_core::Result<rua_core::RuntimeEvent>> + Send>,
        >,
    > {
        let (release, held) = tokio::sync::oneshot::channel();
        self.0.send((req, release)).unwrap();
        Ok(Box::pin(futures::stream::once(async move {
            let _ = held.await;
            Ok(rua_core::RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            })
        })))
    }
    async fn cancel(&self, _: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }
    async fn warmup(&self, _: rua_contract::agent::RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_turns_capture_resources_at_claim_and_keep_their_execution_directory() {
    use rua_project::{ProjectCreateResourcesRequest, ProjectResourceDirectoryInput};
    use rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome;
    let temp = tempfile::tempdir().unwrap();
    let state = rua_server::AppState::from_tempdir(
        temp.path().join("data"),
        rua_server::ServerProfile::Test,
    );
    state.wait_platform_initialization().await.unwrap();
    let runtime = state.registry.get_or_create("default").await.unwrap();
    let (sender, mut observations) = tokio::sync::mpsc::unbounded_channel();
    runtime
        .set_react_runtime(std::sync::Arc::new(ProjectTurnRuntime(sender)))
        .await;
    let device =
        rua_server::services::device_context::current_sync_device_id(&state.sync_device).unwrap();
    let root = temp.path().join("photos-not-mounted-here");
    let directory = |device_id: &str| ProjectResourceDirectoryInput {
        directory_id: None,
        device_id: device_id.into(),
        resource_root: root.clone(),
        display_name: None,
        extra: Default::default(),
    };
    let project = state
        .project_catalog
        .create_resources(ProjectCreateResourcesRequest {
            execution_default: None,
            directories: vec![directory("phone-a"), directory("laptop-b")],
            display_name: Some("Photos".into()),
        })
        .unwrap()
        .project;
    let id = uuid::Uuid::new_v4().to_string();
    let submit = |message: &str| rua_channel::core::console::ConsoleChatRequest {
        message: message.into(),
        chat_id: Some(id.clone()),
        model: None,
        is_reconnect: false,
        attachments: vec![],
        input: None,
        messages: None,
        extra: Default::default(),
        channel_kind: None,
    };
    let intake = &state.conversation_input_intake;
    let first = intake
        .submit_project_delivery(&project.project_id, "default", submit("first"))
        .await;
    assert!(
        matches!(
            first,
            ConversationInputDeliveryOutcome::Delivered { accepted: true, .. }
        ),
        "{first:?}"
    );
    let (first, release) =
        tokio::time::timeout(std::time::Duration::from_secs(10), observations.recv())
            .await
            .unwrap()
            .unwrap();
    let key = rua_session::TURN_PROJECT_RESOURCES_KEY;
    assert_eq!(first.metadata[key], serde_json::json!(project.resources));
    let resource_context = first.metadata[rua_agent::hooks::PROJECT_INSTRUCTIONS_METADATA_KEY]
        .as_str()
        .unwrap();
    for value in ["phone-a", "laptop-b", root.to_str().unwrap()] {
        assert!(resource_context.contains(value), "{resource_context}");
    }
    assert!(!first.metadata.contains_key("project_resource_roots"));
    let queued = intake
        .submit_project_delivery(&project.project_id, "default", submit("queued before edit"))
        .await;
    assert!(
        matches!(
            queued,
            ConversationInputDeliveryOutcome::Delivered {
                queued_to_active_authority: true,
                ..
            }
        ),
        "{queued:?}"
    );
    let store = WorkspaceSessionStore::new(state.chat_workspace().as_path());
    let first_turn = store.read_turn_range(&id, 1, 1).await.unwrap().remove(0);
    let execution_dir = first.working_dir.as_ref().unwrap();
    let filename = "task-fixture.txt";
    std::fs::write(
        execution_dir.join(filename),
        "retained Task working directory",
    )
    .unwrap();
    let owner = rua_task::TaskOwner::from_address(ChatAddress::new(id.as_str()));
    let configuration = state
        .product_hooks
        .chat_configuration(&owner.chat_address)
        .await
        .unwrap();
    let (release_task, waiting) = tokio::sync::oneshot::channel::<()>();
    let dependency = state
        .task_runtime
        .adopt_running(
            uuid::Uuid::new_v4().to_string(),
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "Hold the read until after the resource edit".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: serde_json::json!({}),
                },
                origin: Default::default(),
                depends_on: vec![],
                context_refs: vec![],
            },
            rua_task::TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_configuration_epoch(configuration.epoch.clone()),
            rua_task::TaskAdoptedExecution::new(
                async {
                    let _ = waiting.await;
                    rua_task::TaskRunOutput::completed("ready")
                },
                Default::default(),
            ),
        )
        .await
        .unwrap();
    let task_context = rua_core::tool::ToolContext {
        agent_id: "default".into(),
        session_id: id.clone(),
        working_dir: execution_dir.clone(),
        metadata: std::collections::HashMap::from([
            ("turn_id".into(), serde_json::json!(first_turn.turn_id)),
            ("tool_use_id".into(), serde_json::json!("original-read")),
            (
                "chat_configuration_epoch".into(),
                serde_json::json!(configuration.epoch),
            ),
        ]),
        task_runtime: Some(std::sync::Arc::new(
            rua_tool::extended::task_tools::TaskRuntimeHandle::new(state.task_runtime.clone()),
        )),
        ..Default::default()
    };
    let created = rua_contract::Tool::call(&rua_tool::extended::task_tools::TaskCreateTool, serde_json::json!({
        "depends_on":[dependency], "runner":{"type":"tool","tool_name":"read","arguments":{"file_path":filename}}
    }),task_context).await.unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    let updated = state
        .project_catalog
        .replace_resources(&project.project_id, project.resources.revision, vec![])
        .unwrap()
        .project;
    assert_eq!(
        first_turn.metadata[key],
        serde_json::json!(project.resources)
    );
    release.send(()).unwrap();
    let (second, release) =
        tokio::time::timeout(std::time::Duration::from_secs(10), observations.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(second.metadata[key], serde_json::json!(updated.resources));
    let resource_context = second.metadata[rua_agent::hooks::PROJECT_INSTRUCTIONS_METADATA_KEY]
        .as_str()
        .unwrap();
    assert!(!resource_context.contains("phone-a"));
    assert!(!resource_context.contains("laptop-b"));
    assert!(resource_context.contains("[]"));
    assert_eq!(first.working_dir, second.working_dir);
    let header = store.load_chat_header(&id).await.unwrap().unwrap();
    assert_eq!(
        header.extra[rua_session::EXECUTION_DIRECTORY_KEY],
        serde_json::json!({"device_id":device,"path":first.working_dir,"managed":true})
    );
    let turns = store.read_turn_range(&id, 1, 2).await.unwrap();
    assert_eq!(turns[0].metadata[key], serde_json::json!(project.resources));
    assert_eq!(turns[1].metadata[key], serde_json::json!(updated.resources));
    assert!(!root.exists());
    let mut retry = submit("regenerate the original Turn");
    retry.extra.insert(
        rua_server::services::conversation_authority::REGENERATE_TURN_ID_KEY.into(),
        serde_json::json!(first_turn.turn_id),
    );
    let retry = intake
        .submit_project_delivery(&project.project_id, "default", retry)
        .await;
    assert!(
        matches!(
            retry,
            ConversationInputDeliveryOutcome::Delivered {
                queued_to_active_authority: true,
                ..
            }
        ),
        "{retry:?}"
    );
    release.send(()).unwrap();
    let (regenerated, release) =
        tokio::time::timeout(std::time::Duration::from_secs(10), observations.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        regenerated.metadata[key],
        serde_json::json!(project.resources)
    );
    assert_eq!(regenerated.working_dir, first.working_dir);
    store
        .set_collaboration_wake_suppressed(&id, true)
        .await
        .unwrap();
    release_task.send(()).unwrap();
    let task = state
        .task_runtime
        .await_terminal(&owner, task_id, std::time::Duration::from_secs(12))
        .await
        .unwrap();
    assert_eq!(
        task.state,
        rua_task::TaskState::Completed,
        "{:?}",
        task.result
    );
    assert!(task
        .result
        .as_ref()
        .unwrap()
        .output
        .contains("retained Task working directory"));
    let origin = task.spec.origin.turn_id.as_ref().unwrap();
    assert_eq!(origin, &first_turn.turn_id);
    assert_eq!(
        store
            .read_turn(&id, origin)
            .await
            .unwrap()
            .unwrap()
            .metadata[key],
        serde_json::json!(project.resources)
    );
    release.send(()).unwrap();
}
