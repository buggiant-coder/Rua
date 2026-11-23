use super::*;
use rua_server::services::conversation_authority::{
    ConversationAuthority, ConversationInputPayload,
};
use rua_session::chat_handoff::{ChatHandoffPhase, ChatHandoffRecord};
use serde_json::json;

async fn prepared(state: &AppState) -> (WorkspaceSessionStore, ChatHandoffRecord) {
    seed_local_authority(state, CHAT_ID, "handoff fence").await;
    let scope = ChatStore::resolve(&state.rua_root, chat_address());
    let store = WorkspaceSessionStore::new(scope.root());
    store
        .append_conversation_turn(
            CHAT_ID,
            NewConversationTurn {
                turn_id: "origin".into(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-11-22T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: Some("2026-11-22T00:00:01Z".into()),
                    status: "completed".into(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "origin-message".into(),
                    role: "user".into(),
                    created_at: "2026-11-22T00:00:00Z".into(),
                    status: "completed".into(),
                    content: json!([{"type":"text", "text":"original"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
    let header = store.load_chat_header(CHAT_ID).await.unwrap().unwrap();
    let mut candidate = header.extra["hook_configuration"].clone();
    let record = ChatHandoffRecord {
        handoff_id: "fenced-handoff".into(),
        chat_id: CHAT_ID.into(),
        old_placement: header.extra["authority_placement"].clone(),
        new_placement: json!({"device_id":"other-device", "authority_epoch":epoch(2),
            "base_manifest_hash":"sha256:1111111111111111111111111111111111111111111111111111111111111111"}),
        expected_configuration_epoch: candidate["epoch"].as_str().unwrap().into(),
        candidate_configuration_epoch: "candidate-handoff".into(),
        requirements: json!({}),
        phase: ChatHandoffPhase::Prepared,
        extra: Default::default(),
    };
    candidate["epoch"] = json!("candidate-handoff");
    candidate["capability_configuration"] = json!({"execution_directory":{"device_id":"other-device", "path":"/target", "managed":false}});
    store
        .prepare_chat_handoff(&record, Some(&candidate))
        .await
        .unwrap();
    (store, record)
}

fn start(state: &AppState) -> ConversationActiveTurnStart {
    ConversationActiveTurnStart {
        address: chat_address(),
        driver: ConversationDriverKind::Rua,
        authority_device_id: local_device_id(state),
        authority_epoch: epoch(1),
        driver_detail: None,
    }
}

#[tokio::test]
async fn prepared_rejects_new_input_configuration_and_direct_turn_start_but_abort_reopens() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (store, record) = prepared(&state).await;
    let before = store.load_chat_header(CHAT_ID).await.unwrap().unwrap();
    assert!(state
        .conversation_authority
        .start_active_turn(start(&state))
        .await
        .is_err());
    let result = state
        .conversation_authority
        .accept_submission(
            &state.streams,
            &stream_key(),
            start(&state),
            ConversationInputPayload {
                message: "must not start".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: vec![],
                enqueued_at: Instant::now(),
            },
            None,
            None,
        )
        .await;
    assert!(matches!(result, Err(message) if message.contains("handoff")));
    let scope = ChatStore::resolve(&state.rua_root, chat_address());
    assert!(intake(&state)
        .chat_configuration_control(&chat_address(), "default", scope.root(), None)
        .await
        .unwrap_err()
        .contains("handoff"));
    assert!(state
        .conversation_authority
        .queue_snapshot(&chat_address())
        .await
        .is_empty());
    assert_eq!(
        store.load_chat_header(CHAT_ID).await.unwrap().unwrap(),
        before
    );
    store
        .abort_chat_handoff(CHAT_ID, &record.handoff_id)
        .await
        .unwrap();
    assert!(state
        .conversation_authority
        .start_active_turn(start(&state))
        .await
        .is_ok());
    state
        .conversation_authority
        .finish_active_turn(&chat_address())
        .await;
}

#[tokio::test]
async fn recreated_authority_reads_released_fence_from_disk_with_old_header() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (store, record) = prepared(&state).await;
    store
        .release_chat_handoff(CHAT_ID, &record.handoff_id)
        .unwrap();
    let recreated = Arc::new(ConversationAuthority::new());
    let _intake = intake(&state).with_conversation_authority(recreated.clone());
    let failure = recreated
        .start_active_turn(start(&state))
        .await
        .unwrap_err();
    assert!(failure.contains("Released"), "{failure}");
    assert!(recreated.active_turn(&chat_address()).await.is_none());
    assert_eq!(
        store
            .load_chat_header(CHAT_ID)
            .await
            .unwrap()
            .unwrap()
            .extra["authority_placement"],
        record.old_placement
    );
}

#[tokio::test]
async fn prepared_rejects_history_mutations_before_any_stored_history_changes() {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use tower::ServiceExt;
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (store, _) = prepared(&state).await;
    let before = store.load_chat_header(CHAT_ID).await.unwrap().unwrap();
    let turns = store.read_turn_range(CHAT_ID, 1, usize::MAX).await.unwrap();
    for (method, suffix, body) in [
        (
            Method::PUT,
            "/projection-exclusions",
            json!({"message_ids":[]}),
        ),
        (
            Method::PUT,
            "",
            json!({"selected_variant_id":"origin-message", "turn_id":"origin"}),
        ),
        (
            Method::POST,
            "/rollback",
            json!({"before_turn_id":"origin"}),
        ),
    ] {
        let response = rua_server::router::build_app(state.clone(), None)
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/chats/{CHAT_ID}{suffix}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            error["detail"].as_str().unwrap().contains("handoff"),
            "{suffix}: {error}"
        );
    }
    assert_eq!(
        store.load_chat_header(CHAT_ID).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store.read_turn_range(CHAT_ID, 1, usize::MAX).await.unwrap(),
        turns
    );
}

fn watch(id: &str) -> rua_task::TaskWatchOutputNotification {
    rua_task::TaskWatchOutputNotification {
        notification_id: id.into(),
        owner: rua_task::TaskOwner::new(CHAT_ID),
        task_id: "existing-task".into(),
        attempt_no: 1,
        origin_turn_id: Some("origin".into()),
        projection: rua_task::TaskLiveOutputProjection::ActivityOnly,
        records: vec![rua_task::TaskLiveOutputRecord {
            source: "stdout".into(),
            text: "progress".into(),
        }],
        extra: Default::default(),
    }
}

#[tokio::test]
async fn activity_only_note_waits_for_handoff_and_only_durable_duplicates_acknowledge() {
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (store, record) = prepared(&state).await;
    assert!(state
        .conversation_input_intake
        .deliver_task_watch_output(watch("new-note"))
        .await
        .unwrap_err()
        .contains("handoff"));
    assert!(store
        .read_runtime_notes_for_turn(CHAT_ID, "origin")
        .await
        .unwrap()
        .is_empty());
    // A genuinely durable pre-handoff receipt remains acknowledgeable.
    store
        .append_runtime_note(
            CHAT_ID,
            "origin",
            rua_session::RuntimeNote {
                runtime_note_id: "old-note".into(),
                created_at: "2026-11-22T00:00:00Z".into(),
                payload: rua_session::RuntimeNotePayload::TaskWatchOutput(
                    rua_session::TaskWatchOutputRuntimeNote {
                        task_id: "existing-task".into(),
                        attempt_no: 1,
                        projection: rua_session::TaskWatchOutputProjection::ActivityOnly,
                        records: vec![],
                    },
                ),
            },
        )
        .await
        .unwrap();
    state
        .conversation_input_intake
        .deliver_task_watch_output(watch("old-note"))
        .await
        .unwrap();
    store
        .abort_chat_handoff(CHAT_ID, &record.handoff_id)
        .await
        .unwrap();
    state
        .conversation_input_intake
        .deliver_task_watch_output(watch("new-note"))
        .await
        .unwrap();
    assert_eq!(
        store
            .read_runtime_notes_for_turn(CHAT_ID, "origin")
            .await
            .unwrap()
            .len(),
        2
    );
}

struct Completes;
#[async_trait::async_trait]
impl rua_task::TaskRunner for Completes {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }
    async fn run(
        &self,
        _request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        Ok(rua_task::TaskRunOutput::completed("done"))
    }
}

#[tokio::test]
async fn suppressed_terminal_result_keeps_outbox_until_handoff_allows_durable_append() {
    use rua_task::TaskNotificationSink;
    let temp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (store, record) = prepared(&state).await;
    store
        .set_collaboration_wake_suppressed(CHAT_ID, true)
        .await
        .unwrap();
    let task_dir = TempDir::new().unwrap();
    let tasks = Arc::new(
        rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(task_dir.path())).unwrap(),
    );
    tasks.register_runner(Arc::new(Completes));
    let owner = rua_task::TaskOwner::new(CHAT_ID);
    let id = tasks
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "completed before handoff".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: json!({}),
                },
                origin: rua_task::TaskOrigin {
                    turn_id: Some("origin".into()),
                    tool_call_id: Some("call".into()),
                },
                context_refs: vec![],
                depends_on: vec![],
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    let terminal = tasks
        .await_terminal(&owner, &id, Duration::from_secs(5))
        .await
        .unwrap();
    let sink =
        rua_server::services::task_notification::ServerTaskNotificationSink::new(&state.rua_root);
    sink.install_tasks(&tasks);
    sink.install_watch_delivery(state.conversation_input_intake.clone());
    sink.terminal(rua_task::TaskCompletionNotification {
        record: terminal.clone(),
    })
    .await
    .unwrap();
    assert!(store
        .read_runtime_notes_for_turn(CHAT_ID, "origin")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .len(),
        1
    );
    store
        .abort_chat_handoff(CHAT_ID, &record.handoff_id)
        .await
        .unwrap();
    sink.terminal(rua_task::TaskCompletionNotification { record: terminal })
        .await
        .unwrap();
    assert_eq!(
        store
            .read_runtime_notes_for_turn(CHAT_ID, "origin")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(tasks
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap()
        .is_empty());
    tasks.shutdown().await.unwrap();
}
