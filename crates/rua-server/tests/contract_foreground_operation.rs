use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rua_core::ChatAddress;
use rua_device_network::DeviceSessionLiveActivityHandler;
use rua_server::services::active_stream_broadcast::{
    ActiveStreamBroadcastDeviceSessionHandler, ActiveStreamBroadcastHub,
    ActiveStreamBroadcastSubscribeRequest, ActiveStreamBroadcastViewer, ActiveStreamOwner,
};
use rua_server::services::foreground_operation::{
    ForegroundOperationKind, ForegroundOperationRegistry, ForegroundOperationTransition,
    MobileResidencyState,
};
use rua_server::services::sync_transfer::{
    SourceTransferOperationStore, TransferExecutionOutcome, TransferJobStatus, TransferJobStore,
};

#[test]
fn mobile_transfer_leases_are_reference_counted_and_publish_progress() {
    let registry = ForegroundOperationRegistry::new(true);
    assert_eq!(registry.enter_background(false), Some(1));

    assert!(registry.acquire_transfer(
        "job:one".to_string(),
        "transfer-one".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        100,
    ));
    assert!(registry.acquire_transfer(
        "receive:one".to_string(),
        "transfer-one".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        100,
    ));
    registry.update_transfer("job:one", 40, 100);

    let snapshot = registry.snapshot();
    assert!(snapshot.background);
    assert_eq!(snapshot.active_count, 2);
    assert!(snapshot.locks_required);
    assert_eq!(snapshot.residency_state, MobileResidencyState::Active);
    assert_eq!(snapshot.operations[0].bytes_written, 40);
    assert!(registry.release("job:one"));
    assert_eq!(registry.snapshot().active_count, 1);
    assert!(registry.release("receive:one"));
    assert_eq!(registry.snapshot().active_count, 0);
}

#[test]
fn typed_leases_are_idempotent_and_snapshots_are_revision_ordered() {
    let registry = ForegroundOperationRegistry::new(true);
    assert!(registry.acquire(
        "viewer:terminal:z".to_string(),
        "terminal-z".to_string(),
        ForegroundOperationKind::TerminalViewer,
    ));
    let first = registry.snapshot();
    assert!(registry.acquire(
        "viewer:terminal:z".to_string(),
        "terminal-z".to_string(),
        ForegroundOperationKind::TerminalViewer,
    ));
    assert_eq!(registry.snapshot().revision, first.revision);
    assert!(!registry.acquire(
        "viewer:terminal:z".to_string(),
        "chat-z".to_string(),
        ForegroundOperationKind::ChatViewer,
    ));
    assert!(registry.acquire(
        "execution:chat:a".to_string(),
        "chat-a".to_string(),
        ForegroundOperationKind::ConversationExecution,
    ));
    assert!(registry.acquire(
        "viewer:chat:m".to_string(),
        "chat-m".to_string(),
        ForegroundOperationKind::ChatViewer,
    ));

    let snapshot = registry.snapshot();
    assert!(snapshot.revision > first.revision);
    assert_eq!(
        snapshot
            .operations
            .iter()
            .map(|operation| operation.lease_id.as_str())
            .collect::<Vec<_>>(),
        vec!["execution:chat:a", "viewer:chat:m", "viewer:terminal:z"]
    );
}

#[test]
fn touching_a_live_lease_prevents_stale_reaping_until_activity_stops() {
    let registry = ForegroundOperationRegistry::new(true);
    registry.acquire(
        "viewer:chat:a".to_string(),
        "viewer:chat:a".to_string(),
        ForegroundOperationKind::ChatViewer,
    );

    thread::sleep(Duration::from_millis(60));
    assert!(registry.touch("viewer:chat:a"));
    assert!(registry.expire_stale(Duration::from_millis(50)).is_empty());

    thread::sleep(Duration::from_millis(60));
    assert_eq!(
        registry.expire_stale(Duration::from_millis(50)),
        vec!["viewer:chat:a".to_string()]
    );
}

#[tokio::test]
async fn remote_live_stream_holds_residency_until_the_stream_is_dropped() {
    let registry = Arc::new(ForegroundOperationRegistry::new(true));
    registry.enter_background(false);
    let handler =
        ActiveStreamBroadcastDeviceSessionHandler::new(Arc::new(ActiveStreamBroadcastHub::new()))
            .with_foreground_operations(Arc::clone(&registry));
    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
            "default", "chat-a",
        ))),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: None,
        collapse_replay: false,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: Some(ActiveStreamBroadcastViewer {
            source_device_id: "device-b".to_string(),
            viewer_id: "viewer-a".to_string(),
            kind: ForegroundOperationKind::ChatViewer,
        }),
    };

    let stream = handler
        .subscribe_live_activity(&serde_json::to_vec(&request).expect("viewer request"))
        .await
        .expect("viewer stream");
    assert_eq!(registry.snapshot().active_count, 1);
    assert_eq!(
        registry.snapshot().operations[0].kind,
        ForegroundOperationKind::ChatViewer
    );

    let mut second_request = request;
    second_request
        .viewer
        .as_mut()
        .expect("second viewer")
        .viewer_id = "viewer-b".to_string();
    second_request.viewer.as_mut().expect("second viewer").kind =
        ForegroundOperationKind::TerminalViewer;
    second_request.owner = Some(ActiveStreamOwner::terminal("terminal-a"));
    let second_stream = handler
        .subscribe_live_activity(
            &serde_json::to_vec(&second_request).expect("second viewer request"),
        )
        .await
        .expect("second viewer stream");
    assert_eq!(registry.snapshot().active_count, 2);
    let kinds = registry
        .snapshot()
        .operations
        .iter()
        .map(|operation| operation.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&ForegroundOperationKind::ChatViewer));
    assert!(kinds.contains(&ForegroundOperationKind::TerminalViewer));

    drop(stream);
    assert_eq!(registry.snapshot().active_count, 1);
    drop(second_stream);
    assert_eq!(registry.snapshot().active_count, 0);

    second_request.viewer = None;
    let passive_stream = handler
        .subscribe_live_activity(
            &serde_json::to_vec(&second_request).expect("passive viewer request"),
        )
        .await
        .expect("passive viewer stream");
    assert_eq!(registry.snapshot().active_count, 0);
    drop(passive_stream);

    assert_eq!(
        registry.snapshot().residency_state,
        MobileResidencyState::BackgroundGrace
    );
}

#[test]
fn background_grace_generation_rejects_stale_suspension() {
    let registry = ForegroundOperationRegistry::new(true);
    let first_generation = registry
        .enter_background(false)
        .expect("first background grace");
    let first = registry.snapshot();
    assert!(first.locks_required);
    assert_eq!(first.residency_state, MobileResidencyState::BackgroundGrace);

    assert!(registry.acquire(
        "execution:chat".to_string(),
        "chat".to_string(),
        ForegroundOperationKind::ConversationExecution,
    ));
    assert!(!registry.mark_suspended(first_generation));
    assert!(registry.release("execution:chat"));
    let second = registry.snapshot();
    assert!(second.grace_generation > first_generation);
    assert!(second.locks_required);
    assert!(!registry.mark_suspended(first_generation));
    assert!(registry.mark_suspended(second.grace_generation));
    let suspended = registry.snapshot();
    assert!(!suspended.locks_required);
    assert_eq!(suspended.residency_state, MobileResidencyState::Suspended);
}

#[test]
fn resident_background_runtime_requires_android_locks() {
    let registry = ForegroundOperationRegistry::new(true);

    assert_eq!(registry.enter_background(true), None);

    let snapshot = registry.snapshot();
    assert!(snapshot.background);
    assert!(snapshot.locks_required);
    assert_eq!(snapshot.active_count, 0);
    assert_eq!(snapshot.residency_state, MobileResidencyState::ResidentIdle);
}

#[test]
fn disabled_and_local_only_transfers_do_not_acquire_residency() {
    let disabled = ForegroundOperationRegistry::new(false);
    assert!(!disabled.acquire_transfer(
        "job:disabled".to_string(),
        "transfer-disabled".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        1,
    ));

    let mobile = ForegroundOperationRegistry::new(true);
    assert!(!mobile.acquire_transfer(
        "job:local".to_string(),
        "transfer-local".to_string(),
        "device-a".to_string(),
        "device-a".to_string(),
        1,
    ));
    assert!(!disabled.has_active());
    assert!(!mobile.has_active());
}

#[test]
fn stale_transfer_lease_is_reaped_instead_of_retaining_residency_forever() {
    let registry = ForegroundOperationRegistry::new(true);
    assert!(registry.acquire_transfer(
        "source:stale".to_string(),
        "transfer-stale".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        10,
    ));

    assert_eq!(
        registry.expire_stale(Duration::ZERO),
        vec!["source:stale".to_string()]
    );
    assert!(!registry.has_active());
}

#[test]
fn transfer_job_terminal_states_release_their_foreground_lease_once() {
    let registry = std::sync::Arc::new(ForegroundOperationRegistry::new(true));
    let jobs = TransferJobStore::with_foreground_operations(std::sync::Arc::clone(&registry));

    let (succeeded, _) = jobs.start(
        "device-a".to_string(),
        "device-b".to_string(),
        "/target".into(),
        100,
    );
    jobs.update_progress(&succeeded.job_id, 40);
    assert_eq!(registry.snapshot().operations[0].bytes_written, 40);
    jobs.succeed(
        &succeeded.job_id,
        TransferExecutionOutcome {
            files_written: 1,
            bytes_written: 100,
        },
    );
    assert_eq!(
        jobs.get(&succeeded.job_id).expect("succeeded job").status,
        TransferJobStatus::Succeeded
    );
    assert!(!registry.has_active());

    let (failed, _) = jobs.start(
        "device-a".to_string(),
        "device-b".to_string(),
        "/target".into(),
        10,
    );
    jobs.fail(&failed.job_id, "failed".to_string());
    assert!(!registry.has_active());

    let (cancelled, _) = jobs.start(
        "device-a".to_string(),
        "device-b".to_string(),
        "/target".into(),
        10,
    );
    jobs.cancel(&cancelled.job_id).expect("cancel job");
    jobs.fail(&cancelled.job_id, "late worker failure".to_string());
    assert_eq!(
        jobs.get(&cancelled.job_id).expect("cancelled job").status,
        TransferJobStatus::Cancelled
    );
    assert!(!registry.has_active());
}

#[test]
fn background_acquire_and_final_release_publish_non_collapsible_transitions() {
    let registry = ForegroundOperationRegistry::new(true);
    let mut transitions = registry.subscribe_transitions();
    let initial_generation = registry
        .enter_background(false)
        .expect("initial background grace");
    assert!(registry.acquire_transfer(
        "job:transition".to_string(),
        "transition".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        10,
    ));
    assert_eq!(
        transitions.try_recv().expect("acquire transition"),
        ForegroundOperationTransition::AcquiredWhileBackground
    );

    assert!(registry.release("job:transition"));
    let ForegroundOperationTransition::FinalReleasedWhileBackground { grace_generation } =
        transitions.try_recv().expect("release transition")
    else {
        panic!("expected final release transition");
    };
    assert!(grace_generation > initial_generation);
}

#[test]
fn remote_source_operation_holds_and_releases_its_own_transfer_lease() {
    let registry = std::sync::Arc::new(ForegroundOperationRegistry::new(true));
    let operations =
        SourceTransferOperationStore::with_foreground_operations(std::sync::Arc::clone(&registry));
    let (started, _) = operations
        .start(
            "remote-source".to_string(),
            100,
            "device-a".to_string(),
            "device-b".to_string(),
        )
        .expect("start source operation");
    assert_eq!(started.status, TransferJobStatus::Running);
    assert_eq!(registry.snapshot().active_count, 1);

    operations.update_progress("remote-source", 50);
    assert_eq!(registry.snapshot().operations[0].bytes_written, 50);
    operations.succeed(
        "remote-source",
        TransferExecutionOutcome {
            files_written: 1,
            bytes_written: 100,
        },
    );
    assert!(!registry.has_active());
}
