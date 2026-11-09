use futures_lite::StreamExt;
use rua_sync::endpoint::SyncNode;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::opaque_replica_log::{
    should_download_replica_log_content_after_read_error, OpaqueReplicaLog, SyncChange,
    SyncReplicaLogFrame,
};

async fn named_tag_count(node: &SyncNode) -> usize {
    let mut tags = node.blobs_store().tags().list().await.unwrap();
    let mut count = 0;
    while tags.next().await.is_some() {
        count += 1;
    }
    count
}

fn hlc() -> HlcTimestamp {
    HlcTimestamp {
        wall_ms: 42,
        counter: 0,
        device_id: [7; 8],
    }
}

async fn spawn_log() -> (
    tempfile::TempDir,
    SyncNode,
    OpaqueReplicaLog,
    DeviceIdentity,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let sync_key = network.derive_sync_key();
    let node = SyncNode::bind_local(tempdir.path(), network.namespace_secret(), &device)
        .await
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        sync_key,
        node.blobs_store().clone(),
    );
    (tempdir, node, log, device)
}

#[tokio::test]
async fn append_then_read_change_roundtrips_encrypted_envelope() {
    let (_dir, node, log, device) = spawn_log().await;
    let change = SyncChange::new(
        "op-1",
        "prompt",
        "prompts/private-name.md",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "secret"}),
    );

    let tags_before = named_tag_count(&node).await;
    let key = log.append_change(&change).await.unwrap();
    assert_eq!(named_tag_count(&node).await, tags_before);
    let recovered = log
        .read_change(device.to_iroh_author().id(), &key)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(recovered, change);
}

#[tokio::test]
async fn inbox_content_hash_read_does_not_query_doc_again() {
    let (_dir, node, log, device) = spawn_log().await;
    let change = SyncChange::new(
        "op-inbox-hash",
        "prompt",
        "prompts/inbox-hash.md",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "read from the durable Inbox hash"}),
    );
    let author = device.to_iroh_author().id();
    let key = log.append_change(&change).await.unwrap();
    let content_hash = node
        .doc()
        .get_exact(author, &key, false)
        .await
        .unwrap()
        .unwrap()
        .content_hash();

    node.doc().del(author, key).await.unwrap();
    assert!(node
        .doc()
        .get_exact(
            author,
            log.record_entry_key(&change.domain, &change.record_id),
            false
        )
        .await
        .unwrap()
        .is_none());

    let (frame, _) = log
        .read_frame_by_content_hash_with_diagnostics(&content_hash.to_string())
        .await
        .unwrap();
    assert_eq!(frame, SyncReplicaLogFrame::Single(change));
}

#[tokio::test]
async fn append_change_batch_writes_each_change_as_its_own_record_entry() {
    let (_dir, _node, log, device) = spawn_log().await;
    let first = SyncChange::new(
        "op-batch-1",
        "prompt",
        "prompts/one.md",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "one"}),
    );
    let second = SyncChange::new(
        "op-batch-2",
        "provider_secret",
        "openai",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"name": "openai"}),
    );

    log.append_change_batch("batch-1", Some("bulk"), &[first.clone(), second.clone()])
        .await
        .unwrap();

    let author = device.to_iroh_author().id();
    for change in [&first, &second] {
        let key = log.record_entry_key(&change.domain, &change.record_id);
        let recovered = log.read_frame(author, &key).await.unwrap().unwrap();
        match recovered {
            SyncReplicaLogFrame::Single(single) => assert_eq!(&single, change),
        }
    }

    let updated_first = SyncChange::new(
        "op-batch-3",
        first.domain.clone(),
        first.record_id.clone(),
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "one updated"}),
    );
    let first_key = log.append_change(&updated_first).await.unwrap();
    assert_eq!(
        first_key,
        log.record_entry_key(&first.domain, &first.record_id)
    );
    assert_eq!(
        log.read_change(author, &first_key).await.unwrap(),
        Some(updated_first),
        "re-exporting one record should supersede that record"
    );
    let second_key = log.record_entry_key(&second.domain, &second.record_id);
    assert_eq!(
        log.read_change(author, &second_key).await.unwrap(),
        Some(second),
        "re-exporting one record must not replace another record"
    );
}

#[tokio::test]
async fn production_node_persists_replica_log_blob_content_across_restart() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let sync_key = network.derive_sync_key();
    let node = SyncNode::bind(tempdir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        sync_key.clone(),
        node.blobs_store().clone(),
    );
    let change = SyncChange::new(
        "op-persisted",
        "prompt",
        "prompts/persisted.md",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "survives restart"}),
    );
    let key = log.append_change(&change).await.unwrap();
    node.shutdown().await;

    let node = SyncNode::bind(tempdir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        sync_key,
        node.blobs_store().clone(),
    );
    let recovered = log
        .read_change(device.to_iroh_author().id(), &key)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(recovered, change);
    node.shutdown().await;
}

#[test]
fn short_replica_log_ciphertext_triggers_peer_content_download() {
    let error = rua_sync::SyncError::Crypto("ciphertext too short".to_string());

    assert!(
        should_download_replica_log_content_after_read_error(&error),
        "iroh-blobs may return an empty/short local blob before content is fetched; \
         the replica log reader must try peer download instead of leaving inbound work retryable"
    );
}

#[tokio::test]
async fn entry_key_is_fixed_length_and_opaque() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-private",
        "prompts",
        "PRIVATE/private-name.md",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"content": "secret"}),
    );

    let key = log.append_change(&change).await.unwrap();
    assert_eq!(key.len(), 32);

    let key_text = String::from_utf8_lossy(&key);
    assert!(!key_text.contains("prompts"));
    assert!(!key_text.contains("PRIVATE"));
    assert!(!key_text.contains("private-name"));
    assert!(!key_text.contains(".md"));
    assert!(!key_text.contains("upsert"));
    assert!(!key_text.contains("device-a"));
}

#[tokio::test]
async fn same_record_derives_same_key() {
    let (_dir, _node, log, _device) = spawn_log().await;

    assert_eq!(
        log.record_entry_key("prompts", "SOUL.md"),
        log.record_entry_key("prompts", "SOUL.md")
    );
    assert_ne!(
        log.record_entry_key("prompts", "SOUL.md"),
        log.record_entry_key("prompts", "OTHER.md")
    );

    assert_ne!(
        log.record_entry_key("a", "b/c"),
        log.record_entry_key("a/b", "c")
    );
}

#[tokio::test]
async fn agent_config_key_does_not_contain_agent_id() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-agent-config",
        "agent_config",
        "test1",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"id": "test1"}),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("agent_config"));
    assert!(!key_text.contains("test1"));
    assert!(!key_text.contains("upsert"));
}

#[tokio::test]
async fn chat_key_does_not_contain_chat_or_session_id() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-chat",
        "chat",
        "default/sessions/session-secret",
        "session_upsert",
        "device-a",
        hlc(),
        serde_json::json!({"agent": {"memory": {"content": []}}}),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("chat"));
    assert!(!key_text.contains("default"));
    assert!(!key_text.contains("session-secret"));
    assert!(!key_text.contains("session_upsert"));
}

#[tokio::test]
async fn provider_secret_key_does_not_contain_provider_id() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-provider-secret",
        "provider_secret",
        "openai-private",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({"id": "openai-private", "api_key": "sk-secret"}),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("provider_secret"));
    assert!(!key_text.contains("openai-private"));
    assert!(!key_text.contains("upsert"));
    assert!(!key_text.contains("device-a"));
}

#[tokio::test]
async fn device_list_key_does_not_contain_device_id() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-device-list",
        "device_list",
        "0101010101010101",
        "remove",
        "device-a",
        hlc(),
        serde_json::json!({"device_id": "0101010101010101"}),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("device_list"));
    assert!(!key_text.contains("0101010101010101"));
    assert!(!key_text.contains("remove"));
    assert!(!key_text.contains("device-a"));
}

#[tokio::test]
async fn skill_blob_manifest_key_does_not_contain_skill_or_path() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-skill",
        "skill_blob_manifest",
        "private-skill",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({
            "skill_name": "private-skill",
            "files": [{"rel_path": "scripts/secret.sh", "hash_ref": "abc"}]
        }),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("skill_blob_manifest"));
    assert!(!key_text.contains("private-skill"));
    assert!(!key_text.contains("scripts"));
    assert!(!key_text.contains("secret.sh"));
}

#[tokio::test]
async fn workspace_skill_key_does_not_contain_agent_skill_or_path() {
    let (_dir, _node, log, _device) = spawn_log().await;
    let change = SyncChange::new(
        "op-workspace-skill",
        "skill_blob_manifest",
        "workspace/private-agent/private-skill",
        "upsert",
        "device-a",
        hlc(),
        serde_json::json!({
            "scope": "workspace",
            "agent_id": "private-agent",
            "skill_name": "private-skill",
            "files": [{"rel_path": "scripts/secret.sh", "hash_ref": "abc"}]
        }),
    );

    let key = log.append_change(&change).await.unwrap();
    let key_text = String::from_utf8_lossy(&key);

    assert_eq!(key.len(), 32);
    assert!(!key_text.contains("workspace"));
    assert!(!key_text.contains("private-agent"));
    assert!(!key_text.contains("private-skill"));
    assert!(!key_text.contains("secret.sh"));
}
