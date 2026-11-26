//! Agent deletion is an admission decision, not a shutdown.
//!
//! These cases pin the two failure modes the product must never have: deleting
//! an Agent while it still owns unfinished work, and "freeing" it by cancelling
//! that work on the user's behalf. Each refusal is asserted against the real
//! owner state, so a generic busy answer cannot pass.

use axum::http::{Method, StatusCode};
use rua_core::ChatAddress;
use rua_security::{ApprovalDecision, ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use serde_json::{json, Value};

use super::support::{
    build_app, create_agent, read_json, request, seed_materialized_chat, wait_for_chat_idle,
    TestApp,
};

/// A Runner that never finishes on its own, so the Task is genuinely executing
/// while the deletion is attempted and only ends when it is cancelled.
struct BlockingRunner {
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl rua_task::TaskRunner for BlockingRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }

    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        Ok(rua_task::TaskRunOutput::cancelled())
    }
}

async fn delete(app: &TestApp, agent_id: &str) -> (StatusCode, Value) {
    let response = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/{agent_id}"),
        None,
        Some("default"),
    )
    .await;
    let status = response.status();
    let body = read_json(response).await;
    if status.is_success() {
        delete_ok_assert(&body, agent_id);
    }
    (status, body)
}

fn delete_ok_assert(body: &Value, agent_id: &str) {
    assert_eq!(body["success"], true);
    assert_eq!(body["agent_id"], agent_id);
}

fn assert_refused(status: StatusCode, body: &Value, needle: &str) {
    assert_eq!(status, StatusCode::CONFLICT, "expected refusal: {body}");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(needle),
        "refusal must identify {needle}, got: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_approval_blocks_deletion_without_resolving_it() {
    let app = build_app();
    let agent_id = "deletion_pending_approval";
    create_agent(&app.app, agent_id, "Pending approval agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "pending-approval-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);

    let approval = app
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            agent_id: agent_id.into(),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("bash requires confirmation"),
            tool_call: ToolCallSnapshot {
                display: "bash".into(),
                args: json!({"command": "ls"}),
            },
        })
        .await;

    let (status, body) = delete(&app, agent_id).await;
    assert_refused(status, &body, "pending approval");

    // The refusal must not have "helped": the Agent, the Chat and the approval
    // are exactly as the user left them.
    assert!(app.state.registry.get(agent_id).await.is_some());
    assert_eq!(
        app.state
            .security
            .approval_authority()
            .list_pending_for_agent(agent_id)
            .await
            .len(),
        1
    );
    assert_eq!(
        app.state
            .security
            .approval_authority()
            .request(&approval.id)
            .await
            .map(|request| request.status),
        Some(rua_security::AuthorityStatus::Pending)
    );
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);

    // Resolving the approval through the existing control admits the deletion,
    // and the Chat history stays readable afterwards.
    app.state
        .security
        .approval_authority()
        .decide(
            &approval.id,
            ApprovalDecision::RunOnce {
                decided_by: "fixture-user".into(),
            },
        )
        .await
        .expect("resolve approval");
    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        after.status(),
        StatusCode::OK,
        "a retained Chat must stay readable after its Agent is deleted"
    );
    let after = read_json(after).await;
    assert_eq!(
        after["meta"]["chat_agent_state"]["selected_agent_available"], false,
        "the retained Chat must report that its selected Agent is gone"
    );
    assert_eq!(
        after["meta"]["chat_agent_state"]["selection"]["agent_id"],
        agent_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_turn_blocks_deletion_and_is_not_stopped() {
    let app = build_app();
    let agent_id = "deletion_running_turn";
    create_agent(&app.app, agent_id, "Running turn agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "running-turn-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);
    let turn = app
        .state
        .conversation_authority
        .start_active_turn(
            rua_server::services::conversation_authority::ConversationActiveTurnStart {
                address: address.clone(),
                driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                authority_device_id: "device-local".into(),
                authority_epoch:
                    rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                        1,
                        0,
                        [0; 8],
                    ),
                driver_detail: None,
            },
        )
        .await
        .expect("start active turn");

    let (status, body) = delete(&app, agent_id).await;
    assert_refused(status, &body, &turn.turn_id);
    assert!(
        app.state
            .conversation_authority
            .active_turn(&address)
            .await
            .is_some(),
        "a refused deletion must not stop the Turn"
    );
    assert!(app.state.registry.get(agent_id).await.is_some());

    app.state
        .conversation_authority
        .finish_active_turn(&address)
        .await;
    wait_for_chat_idle(&app.state, &chat_id).await;
    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_input_blocks_deletion_and_is_kept() {
    let app = build_app();
    let agent_id = "deletion_queued_input";
    create_agent(&app.app, agent_id, "Queued input agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "queued-input-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);
    app.state
        .conversation_authority
        .start_active_turn(
            rua_server::services::conversation_authority::ConversationActiveTurnStart {
                address: address.clone(),
                driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                authority_device_id: "device-local".into(),
                authority_epoch:
                    rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                        1,
                        0,
                        [0; 8],
                    ),
                driver_detail: None,
            },
        )
        .await
        .expect("start active turn");
    app.state
        .conversation_authority
        .enqueue_input(
            &address,
            rua_server::services::conversation_authority::ConversationInputPayload {
                message: "still queued".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".into()),
        )
        .await
        .expect("enqueue input behind the running turn");

    let (status, body) = delete(&app, agent_id).await;
    assert_refused(status, &body, "queued input");
    assert_eq!(
        app.state
            .conversation_authority
            .queue_snapshot(&address)
            .await
            .len(),
        1,
        "a refused deletion must not discard queued input"
    );

    // Resolving the work through the existing controls — finish the Turn and
    // clear the queue — is what admits the deletion.
    app.state
        .conversation_authority
        .finish_active_turn(&address)
        .await;
    app.state.conversation_authority.clear_queue(&address).await;
    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_started_by_another_agent_still_blocks_after_the_chat_switched() {
    let app = build_app();
    let first = "deletion_task_origin_agent";
    let second = "deletion_task_current_agent";
    create_agent(&app.app, first, "Task origin agent").await;
    create_agent(&app.app, second, "Task current agent").await;
    let chat = seed_materialized_chat(&app, first, "switched-task-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();

    let runner = std::sync::Arc::new(BlockingRunner {
        started: tokio::sync::Notify::new(),
    });
    app.state.task_runtime.register_runner(runner.clone());

    let task_id = "00000000-0000-4000-8000-0000000d1a01";
    app.state
        .task_runtime
        .create_with_id(
            task_id.into(),
            rua_task::TaskSpec {
                owner: rua_task::TaskOwner::new(&chat_id),
                instruction: "work that outlives the selection".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture_deletion_block".into(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new(first).unwrap(),
        )
        .await
        .expect("create Task authorized by the first Agent");
    tokio::time::timeout(std::time::Duration::from_secs(5), runner.started.notified())
        .await
        .expect("the blocking Task starts");

    // The Chat now selects another Agent. The Task's authorizing Agent is still
    // the first one, so filtering by the Chat's current selection would wrongly
    // admit the deletion.
    let switched = request(
        &app.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({"expected_agent_id": first, "agent_id": second})),
        Some(first),
    )
    .await;
    assert!(
        switched.status().is_success(),
        "switch Agent: {}",
        read_json(switched).await
    );

    let (status, body) = delete(&app, first).await;
    assert_refused(status, &body, task_id);
    let (status, body) = delete(&app, second).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the Agent that did not start the Task must still be deletable: {body}"
    );

    // Removing the Chat that owns the Task through the ordinary delete control
    // stops that work and settles the block; a later, explicit retry of the
    // Agent deletion is then admitted.
    let removed = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(
        removed.status(),
        StatusCode::OK,
        "delete the Chat owning the Task: {}",
        read_json(removed).await
    );
    let (status, body) = delete(&app, first).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_refusal_does_not_erase_retained_chat_media_or_directory() {
    let app = build_app();
    let agent_id = "deletion_retention";
    create_agent(&app.app, agent_id, "Retention agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "retained-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);
    let scope = rua_core::ChatStore::resolve(&app.state.rua_root, address.clone());
    let media = scope.evergreen_chat_root().join("media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("retained.png"), b"retained-bytes").unwrap();
    let managed = rua_core::RuaRoot::new(&app.state.rua_root)
        .managed_chat_directory(&chat_id, "device-local")
        .unwrap();
    std::fs::create_dir_all(&managed).unwrap();
    std::fs::write(managed.join("work.txt"), b"chat work").unwrap();

    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        std::fs::read(media.join("retained.png")).unwrap(),
        b"retained-bytes",
        "Agent deletion must not reclaim media a retained Chat still references"
    );
    assert_eq!(
        std::fs::read(managed.join("work.txt")).unwrap(),
        b"chat work",
        "Agent deletion must not remove a Chat's own managed directory"
    );

    // The Chat remains discoverable and readable, with its history intact.
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    assert_eq!(history["agent_id"], agent_id);
    assert!(
        history["messages"].is_array(),
        "retained history must stay readable: {history}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_deletion_hands_admission_back_to_the_running_agent() {
    let app = build_app();
    let agent_id = "deletion_admission_restore";
    create_agent(&app.app, agent_id, "Admission restore agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "admission-restore-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);

    let approval = app
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            agent_id: agent_id.into(),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("bash requires confirmation"),
            tool_call: ToolCallSnapshot {
                display: "bash".into(),
                args: json!({"command": "ls"}),
            },
        })
        .await;

    let (status, body) = delete(&app, agent_id).await;
    assert_refused(status, &body, "pending approval");

    // A refused deletion took an admission fence. It has to give it back, or the
    // Agent would be left refusing work until the process restarted.
    assert!(
        !app.state.conversation_authority.is_draining(agent_id).await,
        "a refused deletion must resume the Agent's admission"
    );
    let restarted = app
        .state
        .conversation_authority
        .start_active_turn(
            rua_server::services::conversation_authority::ConversationActiveTurnStart {
                address: address.clone(),
                driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                authority_device_id: "device-local".into(),
                authority_epoch:
                    rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                        1,
                        0,
                        [0; 8],
                    ),
                driver_detail: None,
            },
        )
        .await;
    assert!(
        restarted.is_ok(),
        "a refused deletion must leave the Agent able to accept work: {:?}",
        restarted.err()
    );
    app.state
        .conversation_authority
        .finish_active_turn(&address)
        .await;
    wait_for_chat_idle(&app.state, &chat_id).await;
    app.state
        .security
        .approval_authority()
        .decide(
            &approval.id,
            ApprovalDecision::RunOnce {
                decided_by: "fixture-user".into(),
            },
        )
        .await
        .expect("resolve approval");
    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_an_idle_agent_leaves_its_chats_able_to_take_new_input() {
    let app = build_app();
    let doomed = "deletion_idle_agent";
    let replacement = "deletion_idle_replacement";
    create_agent(&app.app, doomed, "Idle agent").await;
    create_agent(&app.app, replacement, "Replacement agent").await;
    let chat = seed_materialized_chat(&app, doomed, "idle-agent-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let address = ChatAddress::new(&chat_id);

    // An Agent with nothing outstanding is deletable.
    let (status, body) = delete(&app, doomed).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The Chat is retained and reports that its selected Agent is gone.
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(replacement),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(
        read_json(history).await["meta"]["chat_agent_state"]["selected_agent_available"],
        false
    );

    // Choosing another valid Agent is what makes the retained Chat executable
    // again, in the same Chat and the same Project.
    let switch = request(
        &app.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": doomed,
            "agent_id": replacement,
        })),
        Some(replacement),
    )
    .await;
    assert_eq!(switch.status(), StatusCode::OK);

    // Deleting the previous Agent must not leave this Chat fenced: the admission
    // taken for the deletion belongs to the deleted Agent, not to the Chat.
    assert!(
        !app.state
            .conversation_authority
            .is_draining(replacement)
            .await,
        "the replacement Agent must not inherit the deleted Agent's admission fence"
    );
    let resumed = app
        .state
        .conversation_authority
        .accept_submission(
            app.state.streams.as_ref(),
            &rua_server::streaming::StreamKey::new(
                "console",
                address.clone(),
                rua_server::streaming::Priority::Normal,
            ),
            rua_server::services::conversation_authority::ConversationActiveTurnStart {
                address: address.clone(),
                driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                authority_device_id: "device-local".into(),
                authority_epoch:
                    rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                        1,
                        0,
                        [0; 8],
                    ),
                driver_detail: None,
            },
            rua_server::services::conversation_authority::ConversationInputPayload {
                message: "continue in the retained chat".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            None,
            None,
        )
        .await;
    assert!(
        resumed.is_ok(),
        "a retained Chat must accept input after its previous Agent was deleted: {:?}",
        resumed.err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_collaboration_executor_cannot_be_swapped_out_of_the_child_chat() {
    let app = build_app();
    let executor = "deletion_collab_executor";
    let other = "deletion_collab_other";
    create_agent(&app.app, executor, "Collaboration executor").await;
    create_agent(&app.app, other, "Another agent").await;
    let chat = seed_materialized_chat(&app, executor, "collaboration-child-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();

    // Pin the collaboration executor the way a child Chat carries it.
    let runtime = app.state.registry.get_or_create(executor).await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.chat_workspace_dir());
    let mut header = store.load_chat_header(&chat_id).await.unwrap().unwrap();
    header.extra.insert(
        rua_session::collaboration::COLLABORATION_META_KEY.to_owned(),
        json!({"executor": {"kind": "agent", "agent_id": executor}}),
    );
    store
        .save_chat_header_with_configuration(&header)
        .await
        .unwrap();

    // A pinned executor with nothing outstanding is still deletable; the Chat
    // keeps its history and its executor identity.
    let (status, body) = delete(&app, executor).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(other),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    assert!(
        history["messages"].is_array(),
        "a deleted executor must not take the child Chat's history with it: {history}"
    );

    // Nothing may reassign the fixed executor, not even through the ordinary
    // Agent-selection control: a different executor requires a new collaboration.
    let switch = request(
        &app.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": executor,
            "agent_id": other,
        })),
        Some(other),
    )
    .await;
    assert_eq!(
        switch.status(),
        StatusCode::CONFLICT,
        "a fixed collaboration executor must not be replaced after deletion"
    );
    let detail = read_json(switch).await["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        detail.contains("fixed_collaboration_executor"),
        "the refusal must name the fixed executor, got: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_keeps_the_chat_security_snapshot_and_unified_listing() {
    let app = build_app();
    let agent_id = "deletion_snapshot";
    create_agent(&app.app, agent_id, "Snapshot agent").await;
    let chat = seed_materialized_chat(&app, agent_id, "snapshot-chat").await;
    let chat_id = chat["id"].as_str().unwrap().to_string();

    // A Chat keeps the security snapshot it materialised with; that snapshot is
    // what a retained run must still honour, not a widened default.
    let runtime = app.state.registry.get_or_create(agent_id).await.unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.chat_workspace_dir());
    let mut header = store.load_chat_header(&chat_id).await.unwrap().unwrap();
    let snapshot = json!({"permission_mode": "read-only", "allowed_tools": ["read"]});
    header.extra.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.to_owned(),
        snapshot.clone(),
    );
    store
        .save_chat_header_with_configuration(&header)
        .await
        .unwrap();

    let (status, body) = delete(&app, agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let retained = store.load_chat_header(&chat_id).await.unwrap().unwrap();
    assert_eq!(
        retained.extra[rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY],
        snapshot,
        "Agent deletion must not widen or erase a retained Chat's security snapshot"
    );

    // The unified listing is projected from retained Chat headers, so the Chat
    // must stay discoverable after its Agent definition is gone.
    let listing = request(&app.app, Method::GET, "/api/chats", None, None).await;
    assert_eq!(listing.status(), StatusCode::OK);
    let listing = read_json(listing).await;
    assert!(
        listing
            .as_array()
            .is_some_and(|chats| chats.iter().any(|chat| chat["id"] == chat_id)),
        "the unified list must not drop a Chat whose Agent was deleted: {listing}"
    );
}
