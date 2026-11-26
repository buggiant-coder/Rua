use std::{path::Path, sync::Arc};

use rua_core::{ChatAddress, ChatStore};
use rua_sync::{
    evergreen_chat_adapter::{file_record_id, EvergreenChatSyncAdapter},
    hlc::HlcTimestamp,
    merge_store::MergeStore,
    opaque_replica_log::SyncChange,
    sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry},
    SyncError,
};
use serde_json::{json, Value};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}
fn directory(root: &Path) -> std::path::PathBuf {
    ChatStore::resolve(root, ChatAddress::new("history"))
        .root()
        .join("sessions/history")
}
fn header(root: &Path, device: &str, epoch: u64, title: &str) {
    std::fs::create_dir_all(directory(root)).unwrap();
    rua_config::atomic_write_json(&directory(root).join("session.json"), &json!({
        "chat_id":"history", "metadata":{"title":title}, "agent_id":"agent-a",
        "authority_placement":{"device_id":device,"authority_epoch":hlc(epoch).to_sortable_string()},
        "message_log":{"latest_sequence":2}, "future_runtime_field":{"must":"survive"},
    })).unwrap();
}
fn record(changes: &[SyncChange], relative: &str, time: u64) -> SyncChange {
    let mut change = changes
        .iter()
        .find(|c| c.record_id == file_record_id(&ChatAddress::new("history"), relative))
        .unwrap()
        .clone();
    change.hlc = hlc(time);
    change
}
fn load(root: &Path, relative: &str) -> Value {
    serde_json::from_slice(&std::fs::read(directory(root).join(relative)).unwrap()).unwrap()
}

#[tokio::test]
async fn history_waits_for_new_placement_and_old_epoch_cannot_win_with_a_later_clock() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    header(a.path(), "computer", 1, "Initial");
    rua_config::atomic_write_json(
        &directory(a.path()).join("turn_index.json"),
        &json!({"epoch":"one","turns":["original"]}),
    )
    .unwrap();
    let adapter = EvergreenChatSyncAdapter::new(a.path().into(), "computer");
    let first = adapter.export_records().await.unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            b.path().into(),
            "observer",
        )))
        .unwrap();
    let merge = MergeStore::open(&b.path().join("merge")).unwrap();
    let first_history = record(&first, "turn_index", 900);
    let signer = rua_sync::identity::DeviceIdentity::generate();
    let other_signer = rua_sync::identity::DeviceIdentity::generate();
    for mut signed in [first_history.clone(), record(&first, "session", 900)] {
        signed.device_id = hex::encode(signer.device_id_short());
        rua_sync::evergreen_chat_adapter::validate_chat_record_author(
            &signed,
            signer.to_iroh_author().id(),
        )
        .unwrap();
        assert!(
            rua_sync::evergreen_chat_adapter::validate_chat_record_author(
                &signed,
                other_signer.to_iroh_author().id()
            )
            .is_err()
        );
    }
    assert!(matches!(
        registry.apply_change(&first_history, &merge).await,
        Err(SyncError::PendingChatPlacement { .. })
    ));
    assert!(!merge.has_applied_op(&first_history.op_id).unwrap());
    registry
        .apply_change(&record(&first, "session", 900), &merge)
        .await
        .unwrap();
    registry.apply_change(&first_history, &merge).await.unwrap();

    let mut forged = record(&first, "turn_index", 1000);
    forged.op_id = "forged".into();
    forged.device_id = "phone".into();
    assert!(matches!(
        registry.apply_change(&forged, &merge).await,
        Err(SyncError::InvalidPayload(_))
    ));

    header(a.path(), "phone", 2, "Old list title");
    rua_config::atomic_write_json(
        &directory(a.path()).join("turn_index.json"),
        &json!({"epoch":"two","turns":["original","task-result"]}),
    )
    .unwrap();
    let next = EvergreenChatSyncAdapter::new(a.path().into(), "phone")
        .export_records()
        .await
        .unwrap();
    let second_history = record(&next, "turn_index", 100);
    assert!(matches!(
        registry.apply_change(&second_history, &merge).await,
        Err(SyncError::PendingChatPlacement { .. })
    ));
    registry
        .apply_change(&record(&next, "session", 100), &merge)
        .await
        .unwrap();
    assert_eq!(
        load(b.path(), "session.json")["metadata"]["title"],
        "Initial"
    );
    registry
        .apply_change(&second_history, &merge)
        .await
        .unwrap();
    assert_eq!(
        load(b.path(), "turn_index.json")["turns"],
        json!(["original", "task-result"])
    );
    let mut late = first_history;
    late.op_id = "old-epoch-late".into();
    late.hlc = hlc(2000);
    assert_eq!(
        registry.apply_change(&late, &merge).await.unwrap().status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        load(b.path(), "turn_index.json")["turns"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let mut old_listing = record(&first, "session", 500);
    old_listing.op_id = "old-listing-late".into();
    old_listing.payload["metadata"]["title"] = json!("Must not revive");
    assert_eq!(
        registry
            .apply_change(&old_listing, &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        load(b.path(), "session.json")["metadata"]["title"],
        "Initial"
    );
}

#[tokio::test]
async fn replica_listing_edits_keep_authority_runtime_fields_and_cannot_change_project() {
    let root = tempfile::tempdir().unwrap();
    header(root.path(), "computer", 1, "Initial");
    let adapter = EvergreenChatSyncAdapter::new(root.path().into(), "computer");
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let mut incoming = load(root.path(), "session.json");
    incoming["metadata"]["title"] = json!("Renamed on phone");
    incoming["message_log"]["latest_sequence"] = json!(1);
    incoming["agent_id"] = json!("stale-agent");
    incoming["future_runtime_field"] = json!("stale");
    let mut change = EvergreenChatSyncAdapter::file_change(
        "rename",
        &ChatAddress::new("history"),
        "session",
        incoming,
        "phone",
        hlc(1000),
    );
    adapter.apply_change(&change, &merge).await.unwrap();
    let stored = load(root.path(), "session.json");
    assert_eq!(stored["metadata"]["title"], "Renamed on phone");
    assert_eq!(stored["message_log"]["latest_sequence"], 2);
    assert_eq!(stored["agent_id"], "agent-a");
    assert_eq!(stored["future_runtime_field"], json!({"must":"survive"}));
    change.payload["project_id"] = json!("different");
    assert!(adapter.apply_change(&change, &merge).await.is_err());
}

#[tokio::test]
async fn replica_listing_clock_does_not_block_the_authoritys_runtime_header() {
    let root = tempfile::tempdir().unwrap();
    header(root.path(), "computer", 1, "Initial");
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            root.path().into(),
            "observer",
        )))
        .unwrap();
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let address = ChatAddress::new("history");
    let mut renamed = load(root.path(), "session.json");
    renamed["metadata"]["title"] = json!("New phone title");
    renamed["message_log"]["latest_sequence"] = json!(1);
    registry
        .apply_change(
            &EvergreenChatSyncAdapter::file_change(
                "rename",
                &address,
                "session",
                renamed,
                "phone",
                hlc(900),
            ),
            &merge,
        )
        .await
        .unwrap();
    let mut runtime = load(root.path(), "session.json");
    runtime["metadata"]["title"] = json!("Stale authority title");
    runtime["message_log"]["latest_sequence"] = json!(3);
    registry
        .apply_change(
            &EvergreenChatSyncAdapter::file_change(
                "runtime",
                &address,
                "session",
                runtime.clone(),
                "computer",
                hlc(100),
            ),
            &merge,
        )
        .await
        .unwrap();
    assert_eq!(
        load(root.path(), "session.json")["message_log"]["latest_sequence"],
        3
    );
    assert_eq!(
        load(root.path(), "session.json")["metadata"]["title"],
        "New phone title"
    );
    runtime["message_log"]["latest_sequence"] = json!(1);
    registry
        .apply_change(
            &EvergreenChatSyncAdapter::file_change(
                "stale-runtime",
                &address,
                "session",
                runtime,
                "computer",
                hlc(50),
            ),
            &merge,
        )
        .await
        .unwrap();
    assert_eq!(
        load(root.path(), "session.json")["message_log"]["latest_sequence"],
        3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_rechecks_placement_after_waiting_for_the_chat_writer() {
    for relative in ["turn_index"] {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        header(source.path(), "computer", 1, "Original");
        header(target.path(), "computer", 1, "Original");
        let filename = format!("{relative}.json");
        rua_config::atomic_write_json(
            &directory(source.path()).join(&filename),
            &json!({"epoch":"old"}),
        )
        .unwrap();
        rua_config::atomic_write_json(
            &directory(target.path()).join(&filename),
            &json!({"epoch":"local"}),
        )
        .unwrap();
        let changes = EvergreenChatSyncAdapter::new(source.path().into(), "computer")
            .export_records()
            .await
            .unwrap();
        let change = record(&changes, relative, 1000);
        let store = rua_session::WorkspaceSessionStore::new(
            ChatStore::resolve(target.path(), ChatAddress::new("history")).root(),
        );
        let mut next_header = load(target.path(), "session.json");
        next_header["authority_placement"] =
            json!({"device_id":"phone","authority_epoch":hlc(2).to_sortable_string()});
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = tokio::spawn(async move {
            store
                .merge_replicated_chat_header("history", next_header, move |_, _| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
                .unwrap();
        });
        entered_rx.await.unwrap();
        let adapter = EvergreenChatSyncAdapter::new(target.path().into(), "observer");
        let merge = MergeStore::open(&target.path().join("merge")).unwrap();
        let mut apply = Box::pin(adapter.apply_change(&change, &merge));
        assert!(
            futures_lite::future::poll_once(apply.as_mut())
                .await
                .is_none(),
            "history write must wait for the Chat writer"
        );
        release_tx.send(()).unwrap();
        holder.await.unwrap();
        assert!(matches!(
            apply.await.unwrap(),
            rua_sync::sync_registry::ApplyOutcome::Skipped
        ));
        assert_eq!(load(target.path(), &filename)["epoch"], "local");
    }
}
