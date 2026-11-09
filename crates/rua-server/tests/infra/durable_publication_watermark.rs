use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;

use rua_core::ChatAddress;
use rua_server::services::conversation_manifest::ConversationManifest;
use rua_server::services::durable_publication_watermark::DurablePublicationWatermarkStore;
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009963";

fn address() -> ChatAddress {
    ChatAddress::agent("default", CHAT_ID)
}

fn manifest(sequence: u64) -> ConversationManifest {
    ConversationManifest {
        address: address(),
        latest_turn: u64::from(sequence > 0),
        latest_sequence: sequence,
        turn_count: u64::from(sequence > 0),
        message_count: sequence,
        latest_turn_id: (sequence > 0).then(|| "turn-1".to_string()),
        latest_message_id: (sequence > 0).then(|| format!("message-{sequence}")),
        latest_turn_status: (sequence > 0).then(|| "in_progress".to_string()),
        extra: BTreeMap::new(),
    }
}

#[tokio::test]
async fn missing_watermark_fails_closed_then_round_trips_exact_publication() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");

    assert_eq!(
        store
            .load(&address())
            .await
            .expect("load missing watermark"),
        None,
        "an old Chat without publication evidence must not derive from disk state"
    );

    let published = store
        .advance(&address(), &manifest(3))
        .await
        .expect("publish watermark");
    let loaded = store
        .load(&address())
        .await
        .expect("load watermark")
        .expect("watermark exists");

    assert_eq!(loaded, published);
    assert_eq!(loaded.durable_sequence, 3);
    assert_eq!(
        loaded.manifest_hash,
        manifest(3).hash().expect("manifest hash")
    );
    assert_eq!(loaded.manifest, manifest(3));
}

#[tokio::test]
async fn advancement_is_monotonic_and_same_manifest_retry_is_idempotent() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");
    let first = store
        .advance(&address(), &manifest(2))
        .await
        .expect("first publication");
    let retry = store
        .advance(&address(), &manifest(2))
        .await
        .expect("idempotent publication");
    assert_eq!(retry, first);

    let advanced = store
        .advance(&address(), &manifest(4))
        .await
        .expect("newer publication");
    assert_eq!(advanced.durable_sequence, 4);

    let error = store
        .advance(&address(), &manifest(3))
        .await
        .expect_err("watermark regression must fail");
    assert!(error.to_string().contains("regress"), "{error}");
    assert_eq!(
        store
            .load(&address())
            .await
            .expect("load after rejection")
            .expect("watermark exists")
            .durable_sequence,
        4
    );
}

#[tokio::test]
async fn concurrent_advancement_is_linearized_without_regression() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");
    let first_store = store.clone();
    let second_store = store.clone();

    let first = tokio::spawn(async move { first_store.advance(&address(), &manifest(2)).await });
    let second = tokio::spawn(async move { second_store.advance(&address(), &manifest(4)).await });
    let (first, second) = tokio::join!(first, second);
    let outcomes = [first.expect("first task"), second.expect("second task")];
    assert!(outcomes.iter().any(Result::is_ok));

    let loaded = store
        .load(&address())
        .await
        .expect("load watermark")
        .expect("watermark exists");
    assert_eq!(loaded.durable_sequence, 4);
}

#[tokio::test]
async fn advancement_rejects_wrong_chat_and_same_sequence_divergence() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");
    store
        .advance(&address(), &manifest(2))
        .await
        .expect("first publication");

    let wrong = ChatAddress::project("project-a", CHAT_ID);
    let error = store
        .advance(&wrong, &manifest(3))
        .await
        .expect_err("wrong Chat Address must fail");
    assert!(error.to_string().contains("Chat Address"), "{error}");

    let mut divergent = manifest(2);
    divergent.latest_message_id = Some("different-message".to_string());
    let error = store
        .advance(&address(), &divergent)
        .await
        .expect_err("same sequence with another manifest must fail");
    assert!(error.to_string().contains("diverg"), "{error}");
}

#[tokio::test]
async fn unknown_fields_survive_load_and_next_advance() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");
    let mut first = store
        .advance(&address(), &manifest(1))
        .await
        .expect("first publication");
    first.extra.insert(
        "future_watermark_field".to_string(),
        serde_json::json!({"enabled": true}),
    );
    let path = store.path_for(&address()).expect("watermark path");
    let bytes = serde_json::to_vec_pretty(&first).expect("serialize future watermark");
    let temp_path = path.with_extension("future.tmp");
    let mut file = File::create(&temp_path).expect("create future watermark temp file");
    file.write_all(&bytes).expect("write future watermark");
    file.sync_all().expect("sync future watermark");
    drop(file);
    std::fs::rename(&temp_path, &path).expect("publish future watermark");
    File::open(path.parent().expect("watermark parent"))
        .expect("open watermark parent")
        .sync_all()
        .expect("sync watermark parent");

    store
        .advance(&address(), &manifest(2))
        .await
        .expect("advance publication");
    let loaded = store
        .load(&address())
        .await
        .expect("load watermark")
        .expect("watermark exists");
    assert_eq!(
        loaded.extra["future_watermark_field"],
        serde_json::json!({"enabled": true})
    );
}

#[tokio::test]
async fn stores_watermarks_under_device_local_sync_state() {
    let root = TempDir::new().expect("temp root");
    let store =
        DurablePublicationWatermarkStore::new(root.path().join("sync")).expect("open store");

    let path = store.path_for(&address()).expect("watermark path");
    assert!(path.starts_with(root.path().join("sync/conversation-publication-watermarks")));
    assert!(!path.starts_with(root.path().join("conversation-publication-watermarks")));
}

#[tokio::test]
async fn migrates_legacy_watermark_directory_when_current_directory_is_absent() {
    let root = TempDir::new().expect("temp root");
    let fixture = TempDir::new().expect("legacy fixture");
    let fixture_sync_dir = fixture.path().join("sync");
    let legacy_store = DurablePublicationWatermarkStore::new(&fixture_sync_dir)
        .expect("open legacy fixture store");
    legacy_store
        .advance(&address(), &manifest(3))
        .await
        .expect("write legacy watermark");
    let legacy_dir = root.path().join("conversation-publication-watermarks");
    std::fs::rename(
        fixture_sync_dir.join("conversation-publication-watermarks"),
        &legacy_dir,
    )
    .expect("place legacy directory");
    let sync_dir = root.path().join("sync");
    let current_dir = sync_dir.join("conversation-publication-watermarks");

    let store = DurablePublicationWatermarkStore::new(&sync_dir).expect("migrate legacy store");

    assert!(!legacy_dir.exists());
    assert!(current_dir.exists());
    assert_eq!(
        store
            .load(&address())
            .await
            .expect("load migrated watermark")
            .expect("migrated watermark exists")
            .durable_sequence,
        3
    );
}

#[test]
fn refuses_to_merge_legacy_and_current_watermark_directories() {
    let root = TempDir::new().expect("temp root");
    let legacy_dir = root.path().join("conversation-publication-watermarks");
    let sync_dir = root.path().join("sync");
    let current_dir = sync_dir.join("conversation-publication-watermarks");
    std::fs::create_dir_all(&legacy_dir).expect("create legacy directory");
    std::fs::create_dir_all(&current_dir).expect("create current directory");

    let error = DurablePublicationWatermarkStore::new(&sync_dir)
        .expect_err("two watermark directories must be rejected");

    assert!(error.to_string().contains("refusing to merge"), "{error}");
    assert!(legacy_dir.exists());
    assert!(current_dir.exists());
}
