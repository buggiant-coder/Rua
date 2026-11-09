use iroh_blobs::{
    api::Store as BlobsApiStore,
    store::fs::{compact_test_gate, CompactOutcome, CompactPolicy, FsStore},
};

#[test]
fn compact_policy_uses_absolute_or_ratio_threshold() {
    let policy = CompactPolicy {
        min_file_size: 256,
        min_reclaim_bytes: 128,
        min_reclaim_ratio: 0.25,
    };

    assert!(!policy.should_compact(255, 255));
    assert!(policy.should_compact(1_000, 128));
    assert!(policy.should_compact(400, 100));
    assert!(!policy.should_compact(1_000, 99));
}

#[tokio::test]
async fn metadata_compaction_skips_small_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(dir.path()).await.unwrap();
    let file_size = std::fs::metadata(dir.path().join("blobs.db"))
        .unwrap()
        .len();

    let outcome = store
        .compact_if_needed(CompactPolicy {
            min_file_size: file_size + 1,
            min_reclaim_bytes: 0,
            min_reclaim_ratio: 0.0,
        })
        .await
        .unwrap();

    assert_eq!(outcome, CompactOutcome::BelowMinimumFileSize { file_size });
}

#[tokio::test]
async fn metadata_compaction_keeps_store_usable() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(dir.path()).await.unwrap();
    let api: BlobsApiStore = store.clone().into();
    let before = api
        .blobs()
        .add_slice("before compaction")
        .temp_tag()
        .await
        .unwrap();
    let before_hash = before.hash();

    let outcome = store
        .compact_if_needed(CompactPolicy {
            min_file_size: 0,
            min_reclaim_bytes: u64::MAX,
            min_reclaim_ratio: 0.0,
        })
        .await
        .unwrap();
    assert!(matches!(outcome, CompactOutcome::Compacted { .. }));

    let after = api
        .blobs()
        .add_slice("after compaction")
        .temp_tag()
        .await
        .unwrap();
    assert_ne!(before_hash, after.hash());
}

#[tokio::test]
async fn blob_writes_wait_for_metadata_compaction_and_resume_afterward() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(dir.path()).await.unwrap();
    let api: BlobsApiStore = store.clone().into();
    let (gate, control) = compact_test_gate();

    let compact_store = store.clone();
    let compact = tokio::spawn(async move {
        compact_store
            .compact_if_needed_with_gate(
                CompactPolicy {
                    min_file_size: 0,
                    min_reclaim_bytes: u64::MAX,
                    min_reclaim_ratio: 0.0,
                },
                gate,
            )
            .await
    });
    control.entered.await.unwrap();

    let (write_started_tx, write_started_rx) = tokio::sync::oneshot::channel();
    let mut write = tokio::spawn(async move {
        write_started_tx.send(()).unwrap();
        api.blobs()
            .add_slice("queued during compaction")
            .temp_tag()
            .await
    });
    write_started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut write)
            .await
            .is_err(),
        "blob write completed while metadata compaction held the actor"
    );

    control.release.send(());
    assert!(matches!(
        compact.await.unwrap().unwrap(),
        CompactOutcome::Compacted { .. }
    ));
    let _tag = write.await.unwrap().unwrap();
}
