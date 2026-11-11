use iroh_docs::engine::{LiveEvent, Origin, SyncEvent};
use rua_sync::inbound_inbox::{next_retry_after_ms, InboundInbox, InboxState, RetryExhaustion};
use rua_sync::liveactor_apply::{
    apply_inbound_event_stream, classify_live_event, merge_live_event_action,
    push_unique_entry_ref, wait_for_inbound_event, ApplyFn, ApplyMode, InboundEntryRef,
    LiveEventAction,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

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

#[tokio::test(start_paused = true)]
async fn budget_exhausted_inbox_continues_without_another_live_event() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = Arc::new(InboundInbox::open(dir.path()).unwrap());
    let author = iroh_docs::AuthorId::from(&[7; 32]);
    for key in 0u32..130 {
        inbox
            .record_remote_insert(author, &key.to_le_bytes(), iroh_blobs::Hash::new(b"frame"))
            .unwrap();
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let apply_fn: ApplyFn = Arc::new({
        let inbox = Arc::clone(&inbox);
        let calls = Arc::clone(&calls);
        move |mode| {
            let inbox = Arc::clone(&inbox);
            let calls = Arc::clone(&calls);
            Box::pin(async move {
                let pass = calls.fetch_add(1, Ordering::SeqCst);
                if pass == 0 {
                    assert!(matches!(mode, ApplyMode::RepairScan { force: false }));
                } else {
                    assert!(matches!(mode, ApplyMode::Drain(hashes) if hashes.is_empty()));
                }
                // One bounded drain leaves durable work behind, as a drain
                // reaching its time budget does in the real server.
                for entry in inbox.pending_ready_limited(u64::MAX, 64).unwrap() {
                    inbox.mark_applied(entry.author, &entry.key).unwrap();
                }
                inbox.next_drain_delay(0).unwrap()
            })
        }
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(apply_inbound_event_stream(
        futures_lite::stream::pending(),
        apply_fn,
        cancel.clone(),
    ));
    tokio::task::yield_now().await;
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 66);
    for _ in 0..4 {
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
    }
    let remaining = inbox.count_by_state(InboxState::Pending).unwrap();
    cancel.cancel();
    task.await.unwrap();
    assert_eq!(
        remaining, 0,
        "durable pending work must not require a new Docs event"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn automatic_drains_obey_backoff_and_stop_at_three_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = Arc::new(InboundInbox::open(dir.path()).unwrap());
    let author = iroh_docs::AuthorId::from(&[8; 32]);
    inbox
        .record_remote_insert(author, b"unavailable", iroh_blobs::Hash::new(b"frame"))
        .unwrap();
    let started = tokio::time::Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let apply_fn: ApplyFn = Arc::new({
        let inbox = Arc::clone(&inbox);
        let calls = Arc::clone(&calls);
        move |_| {
            let inbox = Arc::clone(&inbox);
            let calls = Arc::clone(&calls);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let now = started.elapsed().as_millis() as u64;
                for entry in inbox.pending_ready(now).unwrap() {
                    inbox
                        .mark_retryable_limited(
                            entry.author,
                            &entry.key,
                            "unavailable content",
                            next_retry_after_ms(entry.attempts, now),
                            3,
                            RetryExhaustion::Failed,
                        )
                        .unwrap();
                }
                inbox.next_drain_delay(now).unwrap()
            })
        }
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(apply_inbound_event_stream(
        futures_lite::stream::pending(),
        apply_fn,
        cancel.clone(),
    ));
    tokio::task::yield_now().await;
    for (advance_ms, expected_calls) in [(999, 1), (1, 2), (1_999, 2), (1, 3)] {
        tokio::time::advance(Duration::from_millis(advance_ms)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
    }
    assert_eq!(inbox.count_by_state(InboxState::Failed).unwrap(), 1);
    assert_eq!(inbox.diagnostics(1).unwrap().recent_errors[0].attempts, 3);
    assert_eq!(inbox.next_drain_delay(u64::MAX).unwrap(), None);
    tokio::time::advance(Duration::from_secs(86_400)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "terminal/idle queues must not poll or reactivate"
    );
    cancel.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn event_received_during_apply_is_not_lost_when_that_apply_reports_idle() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = Arc::new(InboundInbox::open(dir.path()).unwrap());
    let author = iroh_docs::AuthorId::from(&[9; 32]);
    let hash = iroh_blobs::Hash::new(b"arriving while apply is active");
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let apply_fn: ApplyFn = Arc::new({
        let inbox = Arc::clone(&inbox);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        let calls = Arc::clone(&calls);
        move |mode| {
            let inbox = Arc::clone(&inbox);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let calls = Arc::clone(&calls);
            Box::pin(async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.notify_one();
                    release.notified().await;
                    return None;
                }
                assert!(matches!(mode, ApplyMode::Drain(hashes) if hashes == vec![hash]));
                for entry in inbox.pending_ready(u64::MAX).unwrap() {
                    inbox.mark_applied(entry.author, &entry.key).unwrap();
                }
                inbox.next_drain_delay(0).unwrap()
            })
        }
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stream = futures_lite::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(apply_inbound_event_stream(stream, apply_fn, cancel.clone()));
    started.notified().await;
    inbox
        .record_remote_insert(author, b"arriving", hash)
        .unwrap();
    tx.send(Ok(LiveEvent::ContentReady { hash })).unwrap();
    release.notify_one();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(250)).await;
    tokio::task::yield_now().await;
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    cancel.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn continuous_events_have_a_bounded_first_apply_window() {
    let calls = Arc::new(AtomicUsize::new(0));
    let apply_fn: ApplyFn = Arc::new({
        let calls = Arc::clone(&calls);
        move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { None })
        }
    });
    let stream = futures_lite::stream::unfold((), |_| async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Some((Ok(LiveEvent::PendingContentReady), ()))
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(apply_inbound_event_stream(stream, apply_fn, cancel.clone()));
    tokio::task::yield_now().await;
    for _ in 0..7 {
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
    }
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "continuous events cannot reset the coalescing deadline forever"
    );
    cancel.cancel();
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_event_backlog_cannot_starve_the_coalescing_timer() {
    let polls = Arc::new(AtomicUsize::new(0));
    let observed_at_apply = Arc::new(AtomicUsize::new(usize::MAX));
    let calls = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();
    let apply_fn: ApplyFn = Arc::new({
        let polls = Arc::clone(&polls);
        let observed_at_apply = Arc::clone(&observed_at_apply);
        let calls = Arc::clone(&calls);
        let cancel = cancel.clone();
        move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                let _ = observed_at_apply.compare_exchange(
                    usize::MAX,
                    polls.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                cancel.cancel();
            }
            Box::pin(async { None })
        }
    });
    let stream = futures_lite::stream::poll_fn({
        let polls = Arc::clone(&polls);
        move |_| {
            let count = polls.fetch_add(1, Ordering::SeqCst);
            if count >= 80 {
                return std::task::Poll::Ready(None);
            }
            // Model a finite, immediately-ready buffered event backlog with
            // per-event processing cost. It never yields Pending, so biased
            // branch order must still allow the elapsed timer to win.
            std::thread::sleep(Duration::from_millis(10));
            std::task::Poll::Ready(Some(Ok(LiveEvent::PendingContentReady)))
        }
    });
    apply_inbound_event_stream(stream, apply_fn, cancel).await;
    assert!(
        observed_at_apply.load(Ordering::SeqCst) < 80,
        "the first apply must run before an always-ready backlog is exhausted"
    );
}

#[tokio::test(start_paused = true)]
async fn ended_event_stream_finishes_durable_work_before_exiting() {
    let calls = Arc::new(AtomicUsize::new(0));
    let apply_fn: ApplyFn = Arc::new({
        let calls = Arc::clone(&calls);
        move |mode| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => assert!(matches!(mode, ApplyMode::RepairScan { force: false })),
                1 => assert!(matches!(mode, ApplyMode::RepairScan { force: true })),
                2 => assert!(matches!(mode, ApplyMode::Drain(_))),
                _ => panic!("stream end must not repeatedly force a Repair Scan"),
            }
            Box::pin(async move { (call < 2).then_some(Duration::ZERO) })
        }
    });
    apply_inbound_event_stream(
        futures_lite::stream::empty(),
        apply_fn,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cancellation_drops_an_inflight_apply() {
    struct ApplyDropped(Arc<AtomicBool>);
    impl Drop for ApplyDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let started = Arc::new(tokio::sync::Notify::new());
    let apply_fn: ApplyFn = Arc::new({
        let dropped = Arc::clone(&dropped);
        let started = Arc::clone(&started);
        move |_| {
            let guard = ApplyDropped(Arc::clone(&dropped));
            let started = Arc::clone(&started);
            Box::pin(async move {
                let _guard = guard;
                started.notify_one();
                std::future::pending().await
            })
        }
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(apply_inbound_event_stream(
        futures_lite::stream::pending(),
        apply_fn,
        cancel.clone(),
    ));
    started.notified().await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .expect("cancellation must not wait for the apply/read timeout")
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}
