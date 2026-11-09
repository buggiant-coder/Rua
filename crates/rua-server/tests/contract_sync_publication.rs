use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use iroh_docs::store::Query;
use rua_core::{ChatAddress, ChatScopeStore, ProjectId};
use rua_server::sync_publication::{
    publish_project_permanently_deleted, publish_sync_fact, publish_sync_fact_with_runtime,
    PublicationStatus, SyncPublicationFact, SyncPublicationRuntime,
};
use rua_server::{AppState, ServerProfile};
use rua_session::store::{NewConversationMessage, NewConversationTurn, TurnLifecycle};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use rua_task::{
    TaskArtifact, TaskResourceRef, TaskRunOutput, TaskRunRequest, TaskRunner, TaskRunnerError,
    TaskRunnerKind, TaskRunnerSpec, TaskSpec,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

struct ImmediateTaskRunner;

#[async_trait]
impl TaskRunner for ImmediateTaskRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        Ok(TaskRunOutput {
            outcome: rua_task::TaskRunOutcome::Completed,
            output: "Outcome\nPublished.\n\nKey Results\nRuntime Note ready.\n\nVerification\nSync log inspected.\n\nRemaining Work\nNone".to_string(),
            raw_output: Some(br#"{"detail":"device-local full result"}"#.to_vec()),
            artifacts: vec![TaskArtifact {
                id: "report".to_string(),
                name: "report.txt".to_string(),
                media_type: "text/plain".to_string(),
                resource: TaskResourceRef {
                    uri: "task://artifact/report".to_string(),
                    ref_type: "task_artifact".to_string(),
                    source_device_id: None,
                },
                metadata: json!({"kind": "verification"}),
            }],
            artifact_payloads: Vec::new(),
            error: None,
        })
    }
}

fn build_state(sync_enabled: bool, with_network: bool) -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    std::fs::create_dir_all(tmp.path().join("secret/providers/providers"))
        .expect("seed providers dir");

    if sync_enabled {
        let sync_dir = tmp.path().join("sync");
        std::fs::create_dir_all(&sync_dir).expect("sync dir");
        std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
        if with_network {
            rua_sync::identity::NetworkSecret::generate()
                .save(&sync_dir)
                .expect("network secret");
        }
    }

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    (state, tmp)
}

async fn seed_task_chat(state: &AppState, data_dir: &std::path::Path, chat_id: &str) {
    let workspace_dir = data_dir.join("workspaces/default");
    let now = Utc::now();
    let device_id = state
        .task_runtime
        .device_context()
        .device_id
        .clone()
        .expect("local task Device");
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "default".to_string(),
        title: "Task publication".to_string(),
        created_at: now.to_rfc3339(),
        last_activity_at: now.to_rfc3339(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        json!({
            "device_id": device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": format!("sha256:{}", "0".repeat(64))
        }),
    );
    rua_chat::apply_agent_scope_defaults(&mut header, "default");
    store
        .save_chat_header(&header)
        .await
        .expect("save chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "task-publication-turn".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: now.to_rfc3339(),
                    assistant_started_at: Some(now.to_rfc3339()),
                    assistant_completed_at: Some(now.to_rfc3339()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "task-publication-user".to_string(),
                    role: "user".to_string(),
                    created_at: now.to_rfc3339(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": "Run the background task"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .expect("seed task publication Turn");
    rua_server::services::chat_index::upsert(
        &workspace_dir,
        rua_server::services::chat_index::ChatIndexEntry {
            id: chat_id.to_string(),
            name: "Task publication".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({
                "authority_placement": {
                    "device_id": device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "0".repeat(64))
                }
            }),
            status: "active".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("save chat index");
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let bound = state
        .bound_sync_network()
        .await
        .expect("bound sync network");
    let log = bound.opaque_replica_log();
    let stream = bound
        .doc()
        .get_many(Query::all().build())
        .await
        .expect("query sync doc");
    tokio::pin!(stream);
    let mut changes = Vec::new();
    while let Some(entry) = stream.next().await {
        let entry = entry.expect("sync doc entry");
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

async fn collect_v4_frame_stats(
    state: &AppState,
) -> (usize, Vec<rua_sync::opaque_replica_log::SyncChange>) {
    let bound = state
        .bound_sync_network()
        .await
        .expect("bound sync network");
    let log = bound.opaque_replica_log();
    let stream = bound
        .doc()
        .get_many(Query::all().build())
        .await
        .expect("query sync doc");
    tokio::pin!(stream);
    let mut frame_count = 0usize;
    let mut changes = Vec::new();
    while let Some(entry) = stream.next().await {
        let entry = entry.expect("sync doc entry");
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            frame_count += 1;
            changes.extend(frame.into_changes());
        }
    }
    (frame_count, changes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_task_runtime_note_is_published_as_evergreen_chat_state() {
    let (state, tmp) = build_state(true, true);
    let chat_id = "task-publication-chat";
    seed_task_chat(&state, tmp.path(), chat_id).await;
    state
        .task_runtime
        .register_runner(Arc::new(ImmediateTaskRunner));
    let owner = rua_task::TaskOwner::new("default", chat_id);
    let task_id = state
        .task_runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "publish a terminal Runtime Note".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Fixture_Publish".to_string(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .expect("create task");
    state
        .task_runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .expect("task terminal");

    let published_note = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(note) = collect_v4_changes(&state)
                .await
                .into_iter()
                .filter(|change| {
                    change.domain == "evergreen_chat"
                        && change
                            .record_id
                            .starts_with(&format!("agent/default/{chat_id}/messages/"))
                })
                .filter_map(|change| change.payload.as_str().map(str::to_string))
                .flat_map(|content| {
                    content
                        .lines()
                        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                        .collect::<Vec<_>>()
                })
                .filter_map(|entry| entry["metadata"]["runtime_note"].as_object().cloned())
                .map(serde_json::Value::Object)
                .find(|note| {
                    note["kind"] == "task_attempt_terminal"
                        && note["payload"]["task_id"].as_str() == Some(&task_id)
                })
            {
                break note;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("terminal Runtime Note was not published");

    assert_eq!(published_note["payload"]["terminal_status"], "completed");
    assert!(published_note["payload"]["output"]
        .as_str()
        .unwrap()
        .contains("Published."));
    assert_eq!(
        published_note["payload"]["device"]["device_id"].as_str(),
        state.task_runtime.device_context().device_id.as_deref()
    );
    assert_eq!(
        published_note["payload"]["artifacts"][0]["resource"]["source_device_id"].as_str(),
        state.task_runtime.device_context().device_id.as_deref()
    );
    assert_eq!(
        published_note["payload"]["full_result_ref"]["source_device_id"].as_str(),
        state.task_runtime.device_context().device_id.as_deref()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permanent_project_delete_publishes_chat_and_project_tombstones_in_one_batch() {
    let (state, _tmp) = build_state(true, true);
    let project_id = ProjectId::new("project-delete-publication");
    let address = ChatAddress::project(project_id.clone(), "chat-delete-publication");
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone())
        .root()
        .to_path_buf();
    let chat_dir = scope_root.join("sessions").join(address.chat_id().as_str());
    std::fs::create_dir_all(chat_dir.join("messages")).unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("session.json"),
        &json!({"chat_id": address.chat_id()}),
    )
    .unwrap();
    rua_config::atomic_write(
        &chat_dir.join("messages/000000000000.jsonl"),
        br#"{"message_id":"m1","role":"user"}"#,
    )
    .unwrap();

    let result = publish_project_permanently_deleted(
        &state,
        project_id.clone(),
        scope_root,
        vec![address.clone()],
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 4);
    let (_frames, changes) = collect_v4_frame_stats(&state).await;
    assert_eq!(changes.len(), 4);
    assert!(changes.iter().any(|change| {
        change.domain == rua_sync::chat_adapter::CHAT_DOMAIN
            && change.kind == rua_sync::chat_adapter::CHAT_KIND_INDEX_DELETE
            && change.record_id == rua_sync::chat_adapter::index_record_id(&address)
    }));
    assert_eq!(
        changes
            .iter()
            .filter(|change| {
                change.domain == rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_DOMAIN
                    && change.kind
                        == rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_KIND_FILE_DELETE
            })
            .count(),
        2
    );
    assert!(changes.iter().any(|change| {
        change.domain == rua_sync::project_adapter::PROJECT_CATALOG_DOMAIN
            && change.kind == rua_sync::project_adapter::PROJECT_DELETE_KIND
            && change.record_id == project_id.as_str()
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_committed_fact_publishes_provider_v4_change_when_bound() {
    let (state, _tmp) = build_state(true, true);

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: "pub-provider".to_string(),
            config: json!({
                "id": "pub-provider",
                "name": "Published Provider",
                "base_url": "https://api.example.com/v1",
                "api_key": "sk-published",
                "models": [{"id": "pub-model", "name": "Published Model"}]
            }),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);
    let changes = collect_v4_changes(&state).await;
    let provider_change = changes
        .iter()
        .find(|change| change.domain == "provider_secret" && change.record_id == "pub-provider")
        .expect("provider change");
    assert_eq!(provider_change.payload["api_key"], "sk-published");
    assert_eq!(provider_change.payload["name"], "Published Provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_deleted_fact_publishes_provider_v4_delete_when_bound() {
    let (state, _tmp) = build_state(true, true);

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::ProviderConfigDeleted {
            provider_id: "deleted-provider".to_string(),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);
    let changes = collect_v4_changes(&state).await;
    let provider_change = changes
        .iter()
        .find(|change| change.domain == "provider_secret" && change.record_id == "deleted-provider")
        .expect("provider delete change");
    assert_eq!(provider_change.kind, "delete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_and_template_facts_publish_domain_changes_when_bound() {
    let (state, _tmp) = build_state(true, true);

    for fact in [
        SyncPublicationFact::PromptLibraryFilesCommitted {
            changes: vec![rua_prompts::file_store::PromptFileChange {
                path: "research/example.md".to_string(),
                content: Some("# Example".to_string()),
                version: rua_prompts::file_store::PromptFileVersion::local(
                    chrono::Utc::now().timestamp_millis() as u64,
                ),
                deleted_before: None,
            }],
        },
        SyncPublicationFact::PromptLibraryFilesCommitted {
            changes: vec![rua_prompts::file_store::PromptFileChange {
                path: "research/deleted.md".to_string(),
                content: None,
                version: rua_prompts::file_store::PromptFileVersion::local(
                    chrono::Utc::now().timestamp_millis() as u64,
                ),
                deleted_before: None,
            }],
        },
        SyncPublicationFact::AgentTemplateCommitted {
            template_id: "writer".to_string(),
            template: json!({"name": "Writer"}),
        },
        SyncPublicationFact::AgentTemplateDeleted {
            template_id: "old-writer".to_string(),
        },
        SyncPublicationFact::SubagentTemplatesCommitted {
            templates: json!({"templates": [{"id": "reviewer"}]}),
        },
    ] {
        let result = publish_sync_fact(&state, fact).await;
        assert_eq!(result.status, PublicationStatus::Published);
        assert_eq!(result.appended, 1);
    }

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "prompts"
            && change.record_id == "research/example.md"
            && change.kind == "upsert"
            && change.payload["content"] == "# Example"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "prompts"
            && change.record_id == "research/deleted.md"
            && change.kind == "delete"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_template"
            && change.record_id == "writer"
            && change.kind == "upsert"
            && change.payload["name"] == "Writer"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_template"
            && change.record_id == "old-writer"
            && change.kind == "delete"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "root_config"
            && change.record_id == "subagent_templates"
            && change.kind == "upsert"
            && change.payload["templates"][0]["id"] == "reviewer"
    }));
    let measured = changes
        .iter()
        .filter(|change| {
            matches!(
                (change.domain.as_str(), change.record_id.as_str()),
                ("prompts", "research/example.md")
                    | ("prompts", "research/deleted.md")
                    | ("agent_template", "writer")
                    | ("agent_template", "old-writer")
                    | ("root_config", "subagent_templates")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(measured.len(), 5);
    assert!(measured.iter().all(|change| change.source_timing.is_some()));
    let diagnostics = state.sync_stage_recorder.snapshot();
    assert_eq!(diagnostics.recent_publications.len(), 5);
    assert!(diagnostics.recent_publications.iter().all(|sample| {
        measured
            .iter()
            .any(|change| change.op_id == sample.correlation_id)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_prompt_publication_is_not_suppressed_by_seed_manifest() {
    let (state, tmp) = build_state(true, true);
    let prompt_path = tmp.path().join("prompt_library/seed.md");
    std::fs::create_dir_all(prompt_path.parent().unwrap()).unwrap();
    std::fs::write(&prompt_path, "seed prompt").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/seed.md"],
    )
    .unwrap();

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::PromptLibraryFilesCommitted {
            changes: vec![rua_prompts::file_store::PromptFileChange {
                path: "seed.md".to_string(),
                content: Some("seed prompt".to_string()),
                version: rua_prompts::file_store::PromptFileVersion::local(
                    chrono::Utc::now().timestamp_millis() as u64,
                ),
                deleted_before: None,
            }],
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);
    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "prompts"
            && change.record_id == "seed.md"
            && change.kind == "upsert"
            && change.payload["content"] == "seed prompt"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_skill_publication_is_not_suppressed_by_seed_manifest() {
    let (state, tmp) = build_state(true, true);
    let skill_path = tmp.path().join("skill_pool/guidance/SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(&skill_path, "seed skill").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "skill_pool:guidance",
        "builtin_skill_pool",
        &["skill_pool/guidance/SKILL.md"],
    )
    .unwrap();

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::SkillPoolCommitted {
            skill_name: "guidance".to_string(),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);
    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "skill_blob_manifest"
            && change.record_id == "guidance"
            && change.kind == "upsert"
            && change.payload["skill_name"] == "guidance"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_config_facts_publish_root_config_changes_when_bound() {
    let (state, _tmp) = build_state(true, true);

    for fact in [
        SyncPublicationFact::RootSettingsCommitted {
            settings: json!({"language": "zh-CN"}),
        },
        SyncPublicationFact::RootAgentRegistryCommitted {
            agents: json!({"default": {"enabled": true}}),
        },
        SyncPublicationFact::RootUserTimezoneCommitted {
            timezone: json!("Asia/Shanghai"),
        },
        SyncPublicationFact::RootProcessSecurityCommitted {
            security: json!({
                "allow_no_auth_hosts": ["127.0.0.1"],
                "file_guard": {"enabled": true}
            }),
        },
    ] {
        let result = publish_sync_fact(&state, fact).await;
        assert_eq!(result.status, PublicationStatus::Published);
        assert_eq!(result.appended, 1);
    }

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "root_config"
            && change.record_id == "settings"
            && change.kind == "upsert"
            && change.payload["language"] == "zh-CN"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "root_config"
            && change.record_id == "agents"
            && change.kind == "upsert"
            && change.payload["default"]["enabled"] == true
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "root_config"
            && change.record_id == "user_timezone"
            && change.kind == "upsert"
            && change.payload == "Asia/Shanghai"
    }));
    let security_change = changes
        .iter()
        .find(|change| change.domain == "root_config" && change.record_id == "security")
        .expect("security root-config change");
    assert!(security_change.payload.get("allow_no_auth_hosts").is_none());
    assert_eq!(security_change.payload["file_guard"]["enabled"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_config_and_security_helpers_publish_portable_agent_changes_when_bound() {
    let (state, _tmp) = build_state(true, true);

    let config_result = rua_server::sync_publication::publish_agent_config_committed(
        &state,
        "agent-a",
        json!({
            "id": "agent-a",
            "name": "Agent A",
            "channels": {"dingtalk": {"enabled": true}},
            "security": {"tool_guard": {"enabled": true}},
            "running": {"max_iters": 17},
            "default_model": {"provider": "local", "model": "device-model"},
            "acp": {
                "agents": {
                    "coder": {
                        "command": "local-only",
                        "args": ["--local"],
                        "env": {"TOKEN": "local"},
                        "model": "portable"
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(config_result.status, PublicationStatus::Published);
    assert_eq!(config_result.appended, 1);

    let security_result = rua_server::sync_publication::publish_agent_security_committed(
        &state,
        "agent-a",
        json!({
            "tool_guard": {"enabled": true},
            "allow_no_auth_hosts": ["127.0.0.1"]
        }),
    )
    .await;
    assert_eq!(security_result.status, PublicationStatus::Published);
    assert_eq!(security_result.appended, 1);

    let delete_config_result =
        rua_server::sync_publication::publish_agent_config_deleted(&state, "agent-old").await;
    assert_eq!(delete_config_result.status, PublicationStatus::Published);
    assert_eq!(delete_config_result.appended, 1);

    let delete_security_result =
        rua_server::sync_publication::publish_agent_security_deleted(&state, "agent-old").await;
    assert_eq!(delete_security_result.status, PublicationStatus::Published);
    assert_eq!(delete_security_result.appended, 1);

    let changes = collect_v4_changes(&state).await;
    let agent_config = changes
        .iter()
        .find(|change| change.domain == "agent_config" && change.record_id == "agent-a")
        .expect("agent_config upsert");
    assert_eq!(agent_config.payload["name"], "Agent A");
    assert!(agent_config.payload.get("channels").is_none());
    assert!(agent_config.payload.get("security").is_none());
    assert!(agent_config.payload.get("running").is_none());
    assert!(agent_config.payload.get("default_model").is_none());
    assert!(agent_config.payload["acp"]["agents"]["coder"]
        .get("command")
        .is_none());
    assert_eq!(
        agent_config.payload["acp"]["agents"]["coder"]["model"],
        "portable"
    );

    let agent_security = changes
        .iter()
        .find(|change| change.domain == "agent_security" && change.record_id == "agent-a")
        .expect("agent_security upsert");
    assert_eq!(agent_security.payload["tool_guard"]["enabled"], true);
    assert!(agent_security.payload.get("allow_no_auth_hosts").is_none());

    assert!(changes.iter().any(|change| {
        change.domain == "agent_config"
            && change.record_id == "agent-old"
            && change.kind == "delete"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_security"
            && change.record_id == "agent-old"
            && change.kind == "delete"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_mcp_helpers_publish_catalog_and_client_tombstone_when_bound() {
    let (state, _tmp) = build_state(true, true);

    let catalog_result = rua_server::sync_publication::publish_shared_mcp_catalog_committed(
        &state,
        json!({"clients": {"shared-a": {"command": "node"}}}),
    )
    .await;
    assert_eq!(catalog_result.status, PublicationStatus::Published);
    assert_eq!(catalog_result.appended, 1);

    let delete_result =
        rua_server::sync_publication::publish_shared_mcp_client_deleted(&state, "shared-a").await;
    assert_eq!(delete_result.status, PublicationStatus::Published);
    assert_eq!(delete_result.appended, 1);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "shared_mcp"
            && change.record_id == "catalog"
            && change.kind == "upsert"
            && change.payload["clients"]["shared-a"]["command"] == "node"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "shared_mcp"
            && change.record_id == "client/shared-a"
            && change.kind == "delete"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_prompt_file_facts_publish_agent_prompt_file_changes_when_bound() {
    let (state, _tmp) = build_state(true, true);

    use rua_prompts::agent_files::{AgentPromptFileChange, PromptFileVersion};
    let changes = [
        ("NOTES.md", Some("root note")),
        ("OLD.md", None),
        ("plans/today.md", Some("subdir note")),
        ("plans/old.md", None),
    ]
    .into_iter()
    .map(|(path, content)| AgentPromptFileChange {
        path: path.into(),
        content: content.map(str::to_owned),
        version: PromptFileVersion::local(100),
        deleted_before: None,
    })
    .collect();
    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::AgentPromptFilesCommitted {
            agent_id: "agent-a".into(),
            changes,
        },
    )
    .await;
    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 4);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == "agent-a/root/NOTES.md"
            && change.kind == "upsert"
            && change.payload["content"] == "root note"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == "agent-a/root/OLD.md"
            && change.kind == "delete"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == "agent-a/subdir/plans/today.md"
            && change.kind == "upsert"
            && change.payload["content"] == "subdir note"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "agent_prompt_file"
            && change.record_id == "agent-a/subdir/plans/old.md"
            && change.kind == "delete"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_memory_facts_publish_structured_memory_changes_when_bound() {
    let (state, _tmp) = build_state(true, true);
    let now = chrono::Utc::now();
    let record = rua_memory::MemoryRecord {
        id: rua_memory::MemoryId::new("mem_pub_1"),
        agent_id: "agent-a".to_string(),
        scope: rua_memory::MemoryScope::User,
        kind: rua_memory::MemoryKind::Preference,
        title: "response_style".to_string(),
        content: "User prefers direct engineering conclusions.".to_string(),
        status: rua_memory::MemoryRecordStatus::Active,
        confidence: 0.9,
        importance: Some(0.8),
        created_at: now,
        updated_at: Some(now),
        last_used_at: None,
        expires_at: None,
        superseded_by: None,
        written_on_device_id: Some("device-a".to_string()),
        written_on_device_name_snapshot: Some("Laptop A".to_string()),
        written_on_host_profile: Some("desktop".to_string()),
        resource_device_id: Some("device-a".to_string()),
        resource_device_name_snapshot: Some("Laptop A".to_string()),
        metadata: serde_json::Value::Null,
        sources: vec![],
        links: vec![],
        terms: vec![],
        resources: vec![],
        relations: vec![],
    };

    let upsert = publish_sync_fact(
        &state,
        SyncPublicationFact::StructuredMemoryRecordCommitted {
            record: Box::new(record.clone()),
        },
    )
    .await;
    assert_eq!(upsert.status, PublicationStatus::Published);
    assert_eq!(upsert.appended, 1);
    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "structured_memory"
            && change.record_id == "memory/agent-a/mem_pub_1"
            && change.kind == "snapshot"
            && change.payload["content"] == "User prefers direct engineering conclusions."
    }));

    let visibility = rua_memory::MemoryVisibility {
        memory_id: record.id.clone(),
        owner_agent_id: "agent-a".to_string(),
        mode: rua_memory::MemoryVisibilityMode::Selected,
        grantee_agent_ids: vec!["agent-b".to_string()],
        grants: vec![rua_memory::MemoryVisibilityGrant {
            memory_id: record.id.clone(),
            owner_agent_id: "agent-a".to_string(),
            grantee_agent_id: "agent-b".to_string(),
            permission: "read".to_string(),
            created_by: "user".to_string(),
            created_at: now,
        }],
    };
    let visibility_pub = publish_sync_fact(
        &state,
        SyncPublicationFact::StructuredMemoryVisibilityCommitted {
            visibility: Box::new(visibility),
        },
    )
    .await;
    assert_eq!(visibility_pub.status, PublicationStatus::Published);
    assert_eq!(visibility_pub.appended, 1);
    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "structured_memory"
            && change.record_id == "memory/agent-a/mem_pub_1"
            && change.kind == "snapshot"
            && change.payload["content"] == "User prefers direct engineering conclusions."
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "structured_memory"
            && change.record_id == "visibility/agent-a/mem_pub_1"
            && change.kind == "visibility_snapshot"
            && change.payload["mode"] == "selected"
            && change.payload["grantee_agent_ids"][0] == "agent-b"
    }));

    let delete = publish_sync_fact(
        &state,
        SyncPublicationFact::StructuredMemoryRecordDeleted {
            agent_id: "agent-a".to_string(),
            memory_id: record.id.clone(),
        },
    )
    .await;
    assert_eq!(delete.status, PublicationStatus::Published);
    assert_eq!(delete.appended, 1);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "structured_memory"
            && change.record_id == "visibility/agent-a/mem_pub_1"
            && change.kind == "visibility_snapshot"
            && change.payload["mode"] == "selected"
            && change.payload["grantee_agent_ids"][0] == "agent-b"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "structured_memory"
            && change.record_id == "memory/agent-a/mem_pub_1"
            && change.kind == "delete"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_manager_write_publishes_structured_memory_fact_when_bound() {
    use rua_agent::memory_manager::{
        MemoryKind, MemoryScope, MemoryVisibilityUpdateRequest, MemoryWriteRequest,
    };

    let (state, _tmp) = build_state(true, true);
    let manager = state
        .memory_manager_for("default")
        .await
        .expect("default memory manager");

    let outcome = manager
        .memory_write(MemoryWriteRequest {
            agent_id: "default".to_string(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            title: "response_style".to_string(),
            content: "User prefers direct engineering conclusions.".to_string(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write");
    let record = outcome.record.expect("created record");

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().any(|change| {
            change.domain == "structured_memory"
                && change.record_id == format!("memory/default/{}", record.id)
                && change.kind == "snapshot"
                && change.payload["content"] == "User prefers direct engineering conclusions."
        }),
        "MemoryManager write must publish structured Memory snapshot, got {changes:#?}"
    );

    manager
        .memory_visibility_update(
            "default",
            record.id.clone(),
            MemoryVisibilityUpdateRequest {
                mode: rua_memory::MemoryVisibilityMode::Selected,
                grantee_agent_ids: vec!["agent-b".to_string()],
            },
        )
        .await
        .expect("memory visibility update")
        .expect("visibility");
    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().any(|change| {
            change.domain == "structured_memory"
                && change.record_id == format!("visibility/default/{}", record.id)
                && change.kind == "visibility_snapshot"
                && change.payload["grantee_agent_ids"][0] == "agent-b"
        }),
        "Memory visibility update must publish structured Memory visibility snapshot, got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_list_facts_publish_device_list_changes_when_bound() {
    let (state, tmp) = build_state(true, true);
    let mut entry = rua_sync::device_list::DeviceEntry {
        device_id: [1, 2, 3, 4, 5, 6, 7, 8],
        public_key: [9; 32],
        label: "phone".to_string(),
        friendly_name: "Phone".to_string(),
        added_at_ms: 1,
        last_seen_ms: 2,
        direct_addrs: vec![],
    };
    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::DeviceListEntrySnapshot {
            entry: entry.clone(),
        },
    )
    .await;
    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "device_list"
            && change.record_id == "0102030405060708"
            && change.kind == "snapshot"
            && change.payload["friendly_name"] == "Phone"
    }));

    entry.friendly_name = "Pocket".to_string();
    let device_list = rua_sync::device_list::DeviceList {
        devices: vec![entry],
    };
    std::fs::write(
        tmp.path().join("sync").join("device_list.json"),
        serde_json::to_vec_pretty(&device_list).expect("serialize device list"),
    )
    .expect("write device list");
    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::DeviceListDeviceRenamed {
            device_id_hex: "0102030405060708".to_string(),
            friendly_name: "Pocket".to_string(),
        },
    )
    .await;
    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "device_list"
            && change.record_id == "0102030405060708"
            && change.kind == "rename"
            && change.payload["device_id"] == "0102030405060708"
            && change.payload["friendly_name"] == "Pocket"
    }));

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::DeviceListDeviceRemoved {
            device_id_hex: "0202020202020202".to_string(),
        },
    )
    .await;
    assert_eq!(result.status, PublicationStatus::Published);
    assert_eq!(result.appended, 1);

    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "device_list"
            && change.record_id == "0202020202020202"
            && change.kind == "remove"
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "device_list"
            && change.record_id == "0102030405060708"
            && change.kind == "rename"
            && change.payload["device_id"] == "0102030405060708"
            && change.payload["friendly_name"] == "Pocket"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_skips_when_sync_disabled_without_error() {
    let (state, _tmp) = build_state(false, false);

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: "disabled-provider".to_string(),
            config: json!({"id": "disabled-provider", "name": "Disabled Provider"}),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::SkippedDisabled);
    assert_eq!(result.appended, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_skips_when_sync_enabled_without_bound_network() {
    let (state, _tmp) = build_state(true, false);

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: "unbound-provider".to_string(),
            config: json!({"id": "unbound-provider", "name": "Unbound Provider"}),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::SkippedUnbound);
    assert_eq!(result.appended, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stable_conversation_fact_publishes_evergreen_snapshots_when_bound() {
    let (state, tmp) = build_state(true, true);
    let workspace_dir = tmp.path().join("workspaces/default");
    let chat_id = "00000000-0000-0000-0000-000000001001";
    rua_session::WorkspaceSessionStore::new(&workspace_dir)
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.into(),
                agent_id: "default".into(),
                title: "Published chat".into(),
                created_at: "2026-06-25T00:00:00Z".into(),
                last_activity_at: "2026-06-25T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(&workspace_dir);
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-published".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-25T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-25T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-25T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    NewConversationMessage {
                        message_id: "msg-user".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-25T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: Default::default(),
                    },
                    NewConversationMessage {
                        message_id: "msg-assistant".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-25T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hi"}]),
                        provider: None,
                        tool: None,
                        metadata: Default::default(),
                    },
                ],
            },
        )
        .await
        .expect("append turn");

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::StableConversationState {
            scope_root: workspace_dir,
            address: rua_core::ChatAddress::agent("default", chat_id),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert!(result.appended >= 2, "expected session and turn snapshots");
    let changes = collect_v4_changes(&state).await;
    assert!(changes.iter().any(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/turn_index")
            && change.payload.to_string().contains("completed")
    }));
    assert!(changes.iter().any(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/messages/000000000000.jsonl")
            && change.payload.to_string().contains("hi")
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_seam_preserves_large_daily_change_sets() {
    let (state, tmp) = build_state(true, true);
    let workspace_dir = tmp.path().join("workspaces/default");
    let chat_id = "00000000-0000-0000-0000-000000001002";
    let messages_dir = workspace_dir
        .join("sessions")
        .join(chat_id)
        .join("messages");
    std::fs::create_dir_all(&messages_dir).expect("messages dir");
    for idx in 0..100 {
        let message = json!({
            "message_id": format!("message-{idx:03}"),
            "sequence": idx + 1,
            "turn_id": "turn-large-daily",
            "role": "assistant",
            "created_at": "2026-06-25T00:00:00Z",
            "status": "completed",
            "content": [{"type": "text", "text": format!("daily batch {idx}")}],
            "metadata": {}
        });
        std::fs::write(
            messages_dir.join(format!("{idx:012}.jsonl")),
            format!("{}\n", serde_json::to_string(&message).unwrap()),
        )
        .expect("write message chunk");
    }

    let result = publish_sync_fact(
        &state,
        SyncPublicationFact::StableConversationState {
            scope_root: workspace_dir,
            address: rua_core::ChatAddress::agent("default", chat_id),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    assert!(result.appended >= 100);
    let (frame_count, changes) = collect_v4_frame_stats(&state).await;
    assert_eq!(
        frame_count,
        changes.len(),
        "record-keyed replica-log entries supersede per record; large ordinary publication should expose one current frame per logical record"
    );
    assert!(
        changes.iter().any(|change| {
            change.domain == "evergreen_chat"
                && change.record_id
                    == format!("agent/default/{chat_id}/messages/000000000099.jsonl")
                && change.payload.to_string().contains("daily batch 99")
        }),
        "expanded batch should preserve ordinary Evergreen Chat changes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn successful_publication_notifies_write_coalesced_sync_trigger() {
    let (state, _tmp) = build_state(true, true);
    let cycles = Arc::new(AtomicUsize::new(0));
    let cycles_for_fn = Arc::clone(&cycles);
    let scheduler = rua_sync::trigger::SyncScheduler::start(
        Arc::new(move |_reason| {
            let cycles = Arc::clone(&cycles_for_fn);
            Box::pin(async move {
                cycles.fetch_add(1, Ordering::SeqCst);
                Ok(rua_sync::transport::SyncCycleResult::default())
            })
        }),
        rua_sync::trigger::TriggerConfig {
            write_quiet_period: Duration::from_millis(20),
            write_max_delay: Duration::from_millis(40),
            periodic_interval: Duration::from_secs(3600),
            app_resume_min_interval: Duration::from_millis(1),
        },
    );
    let sync_trigger = Arc::new(tokio::sync::RwLock::new(Some(scheduler.handle())));
    let runtime = SyncPublicationRuntime::new(
        state.rua_root.join("sync"),
        state.sync_enabled.clone(),
        state.sync_node.clone(),
        state.sync_network_secret.clone(),
        state.sync_device.clone(),
        Some(state.sync_hlc_clock.clone()),
        sync_trigger,
    );

    let result = publish_sync_fact_with_runtime(
        &runtime,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: "trigger-provider".to_string(),
            config: json!({"id": "trigger-provider", "name": "Trigger Provider"}),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::Published);
    tokio::time::timeout(Duration::from_secs(2), async {
        while cycles.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("write-coalesced trigger should run a cycle");
    scheduler.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipped_publication_does_not_notify_write_coalesced_sync_trigger() {
    let (state, _tmp) = build_state(false, false);
    let cycles = Arc::new(AtomicUsize::new(0));
    let cycles_for_fn = Arc::clone(&cycles);
    let scheduler = rua_sync::trigger::SyncScheduler::start(
        Arc::new(move |_reason| {
            let cycles = Arc::clone(&cycles_for_fn);
            Box::pin(async move {
                cycles.fetch_add(1, Ordering::SeqCst);
                Ok(rua_sync::transport::SyncCycleResult::default())
            })
        }),
        rua_sync::trigger::TriggerConfig {
            write_quiet_period: Duration::from_millis(20),
            write_max_delay: Duration::from_millis(40),
            periodic_interval: Duration::from_secs(3600),
            app_resume_min_interval: Duration::from_millis(1),
        },
    );
    let sync_trigger = Arc::new(tokio::sync::RwLock::new(Some(scheduler.handle())));
    let runtime = SyncPublicationRuntime::new(
        state.rua_root.join("sync"),
        state.sync_enabled.clone(),
        state.sync_node.clone(),
        state.sync_network_secret.clone(),
        state.sync_device.clone(),
        Some(state.sync_hlc_clock.clone()),
        sync_trigger,
    );

    let result = publish_sync_fact_with_runtime(
        &runtime,
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id: "skip-trigger-provider".to_string(),
            config: json!({"id": "skip-trigger-provider", "name": "Skip Trigger Provider"}),
        },
    )
    .await;

    assert_eq!(result.status, PublicationStatus::SkippedDisabled);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(cycles.load(Ordering::SeqCst), 0);
    scheduler.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_file_operations_publish_canonical_facts_and_recover_offline_deletes() {
    use rua_sync::sync_registry::DomainSyncAdapter;
    let (state, _tmp) = build_state(true, true);
    let files = rua_server::services::agent_prompt_files::agent_prompt_files(&state, "agent-a");
    files.write_file("before", "body".into()).await.unwrap();
    files.rename_file("before", "after").await.unwrap();
    let changes = collect_v4_changes(&state).await;
    assert!(changes
        .iter()
        .any(|c| c.record_id == "agent-a/root/before.md" && c.kind == "delete"));
    assert!(changes
        .iter()
        .any(|c| c.record_id == "agent-a/root/after.md" && c.kind == "upsert"));
    let count = changes.len();
    state.sync_enabled.store(false, Ordering::Relaxed);
    files.delete_file("after").await.unwrap();
    assert_eq!(collect_v4_changes(&state).await.len(), count);
    let adapter = rua_sync::agent_prompt_file_adapter::AgentPromptFileSyncAdapter::new(
        state.rua_root.clone(),
        "agent-device",
    );
    let recovered = adapter.export_records().await.unwrap();
    assert!(recovered
        .iter()
        .any(|c| c.record_id == "agent-a/root/after.md" && c.kind == "delete"));
    assert!(recovered
        .iter()
        .any(|c| c.record_id == "agent-a/root/before.md" && c.kind == "delete"));
    files.write_file("after", "rebuilt".into()).await.unwrap();
    let recovered = adapter.export_records().await.unwrap();
    assert!(recovered
        .iter()
        .any(|c| c.record_id == "agent-a/root/after.md"
            && c.kind == "snapshot"
            && c.payload["content"] == "rebuilt"));
    assert!(!recovered
        .iter()
        .any(|c| c.record_id == "agent-a/root/after.md" && c.kind == "delete"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_library_domain_commits_publish_canonical_rename_and_delete() {
    let (state, _tmp) = build_state(true, true);
    let lib = rua_server::services::prompt_library::prompt_library(&state);
    lib.update_file("domain-old", " exact bytes ")
        .await
        .unwrap();
    lib.rename_file("domain-old", "domain-new").await.unwrap();
    let changes = collect_v4_changes(&state).await;
    assert!(changes
        .iter()
        .any(|c| c.domain == "prompts" && c.record_id == "domain-old.md" && c.kind == "delete"));
    assert!(changes.iter().any(|c| c.domain == "prompts"
        && c.record_id == "domain-new.md"
        && c.payload["content"] == " exact bytes "));
    lib.delete_file("domain-new").await.unwrap();
    let changes = collect_v4_changes(&state).await;
    assert!(changes
        .iter()
        .any(|c| c.domain == "prompts" && c.record_id == "domain-new.md" && c.kind == "delete"));
}
