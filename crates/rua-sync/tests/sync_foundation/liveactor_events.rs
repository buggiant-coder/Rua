use iroh_docs::engine::{LiveEvent, Origin, SyncEvent};
use rua_sync::liveactor_apply::{
    classify_live_event, merge_live_event_action, push_unique_entry_ref, wait_for_inbound_event,
    InboundEntryRef, LiveEventAction,
};
use std::time::SystemTime;

#[test]
fn content_readiness_events_drain_without_full_scan() {
    assert_eq!(
        classify_live_event(&LiveEvent::PendingContentReady),
        Some(LiveEventAction::Drain)
    );

    let hash = iroh_blobs::Hash::new(b"ciphertext");
    assert_eq!(
        classify_live_event(&LiveEvent::ContentReady { hash }),
        Some(LiveEventAction::Drain)
    );
}

#[test]
fn sync_finished_requests_drain_without_repair_scan() {
    let peer = iroh::PublicKey::from_bytes(&[1; 32]).unwrap();
    let event = SyncEvent {
        peer,
        origin: Origin::Accept,
        started: SystemTime::UNIX_EPOCH,
        finished: SystemTime::UNIX_EPOCH,
        result: Err("synthetic sync finished event".to_string()),
    };

    assert_eq!(
        classify_live_event(&LiveEvent::SyncFinished(event)),
        Some(LiveEventAction::Drain)
    );
}

#[test]
fn sync_finished_drain_combines_with_incremental_events() {
    assert_eq!(
        merge_live_event_action(LiveEventAction::Entry, LiveEventAction::Drain),
        LiveEventAction::Drain
    );
    assert_eq!(
        merge_live_event_action(LiveEventAction::Drain, LiveEventAction::Entry),
        LiveEventAction::Drain
    );
}

#[test]
fn event_stream_repair_scan_wins_over_incremental_work() {
    assert_eq!(
        merge_live_event_action(LiveEventAction::Entry, LiveEventAction::RepairScan),
        LiveEventAction::RepairScan
    );
    assert_eq!(
        merge_live_event_action(LiveEventAction::RepairScan, LiveEventAction::Drain),
        LiveEventAction::RepairScan
    );
}

#[tokio::test]
async fn event_stream_error_requests_repair_scan() {
    let mut stream =
        futures_lite::stream::iter(vec![Err::<LiveEvent, _>(anyhow::anyhow!("event gap"))]);
    let mut pending = Vec::new();
    let mut content_ready = Vec::new();

    let action = wait_for_inbound_event(&mut stream, &mut pending, &mut content_ready).await;

    assert_eq!(action, Some(LiveEventAction::RepairScan));
    assert!(pending.is_empty());
    assert!(content_ready.is_empty());
}

#[tokio::test]
async fn content_ready_event_preserves_its_blob_hash() {
    let hash = iroh_blobs::Hash::new(b"ready ciphertext");
    let mut stream = futures_lite::stream::iter(vec![Ok::<LiveEvent, anyhow::Error>(
        LiveEvent::ContentReady { hash },
    )]);
    let mut pending = Vec::new();
    let mut content_ready = Vec::new();

    let action = wait_for_inbound_event(&mut stream, &mut pending, &mut content_ready).await;

    assert_eq!(action, Some(LiveEventAction::Drain));
    assert!(pending.is_empty());
    assert_eq!(content_ready, vec![hash]);
}

#[test]
fn event_stream_end_requests_repair_before_exit() {
    let src = include_str!("../../src/liveactor_apply.rs");
    assert!(src.contains("event stream ended; requesting Repair Scan"));
    assert!(src.contains("apply_fn(ApplyMode::RepairScan { force: true }).await"));
}

#[test]
fn duplicate_remote_entry_refs_coalesce_within_one_window() {
    let author = iroh_docs::AuthorId::from(&[7; 32]);
    let first = InboundEntryRef {
        author,
        key: b"record".to_vec(),
        content_hash: iroh_blobs::Hash::new(b"first"),
    };
    let changed = InboundEntryRef {
        author,
        key: b"record".to_vec(),
        content_hash: iroh_blobs::Hash::new(b"changed"),
    };
    let expected_first = first.clone();
    let mut pending = Vec::new();

    push_unique_entry_ref(&mut pending, first.clone());
    push_unique_entry_ref(&mut pending, first);
    push_unique_entry_ref(&mut pending, changed.clone());

    assert_eq!(pending, vec![expected_first, changed]);
}

#[test]
fn liveactor_debounce_is_short_enough_for_interactive_sync() {
    let src = include_str!("../../src/liveactor_apply.rs");
    assert!(
        src.contains("Duration::from_millis(250)") && !src.contains("Duration::from_secs(1)"),
        "LiveActor debounce should stay short so receiver-side apply does not add a fixed 1s delay to First Sync and Remote Run result sync"
    );
}
