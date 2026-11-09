use rua_prompts::agent_files::{AgentPromptFileStore, PromptFileVersion};
use serde_json::json;

fn state_file(root: &std::path::Path) -> std::path::PathBuf {
    std::fs::read_dir(root.join(".prompt-file-state"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[tokio::test]
async fn delete_survives_restart_and_recreation_supersedes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AgentPromptFileStore::new(tmp.path());
    let guard = store.lock().await;
    let first = guard.write("note.md", "first", false).unwrap();
    let deleted = guard.delete("note.md").unwrap();
    assert!(deleted.version.modified_ms > first.version.modified_ms);
    drop(guard);
    let restarted = AgentPromptFileStore::new(tmp.path()).lock().await;
    assert_eq!(restarted.deletions().unwrap()[0].version, deleted.version);
    let recreated = restarted.write("note.md", "second", false).unwrap();
    assert!(recreated.version.modified_ms > deleted.version.modified_ms);
    assert!(restarted.deletions().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("prompts/note.md")).unwrap(),
        "second"
    );
}

#[tokio::test]
async fn prepared_delete_and_failed_replacement_do_not_hide_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    let written = guard.write("note.md", "original", false).unwrap();
    let path = state_file(tmp.path());
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["live_version"] = serde_json::to_value(written.version).unwrap();
    state["live_hash"] = state["prepared_live_hash"].clone();
    state["prepared_live_version"] =
        json!(PromptFileVersion::local(written.version.modified_ms + 100));
    state["deletion_version"] = json!(PromptFileVersion::local(written.version.modified_ms + 50));
    state["prepared_deleted_before"] =
        json!(PromptFileVersion::local(written.version.modified_ms + 25));
    rua_config::atomic_write_json(&path, &state).unwrap();
    drop(guard);
    let recovered = AgentPromptFileStore::new(tmp.path()).lock().await;
    assert!(recovered.deletions().unwrap().is_empty());
    assert!(recovered.deletion_fence("note.md").unwrap().is_none());
    assert_eq!(
        recovered.local_revision("note.md").unwrap(),
        Some((written.version, false))
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("prompts/note.md")).unwrap(),
        "original"
    );
}

#[tokio::test]
async fn rename_recovers_both_records_even_over_a_deleted_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    guard.write("old.md", "content", false).unwrap();
    guard.write("new.md", "previous", false).unwrap();
    let old_delete = guard.delete("new.md").unwrap();
    let changes = guard.rename("old.md", "new.md").unwrap();
    assert!(changes[1].version.modified_ms > old_delete.version.modified_ms);
    drop(guard);
    let recovered = AgentPromptFileStore::new(tmp.path()).lock().await;
    let deletions = recovered.deletions().unwrap();
    assert_eq!(deletions.len(), 1);
    assert_eq!(deletions[0].path, "old.md");
    assert_eq!(
        recovered.local_revision("new.md").unwrap(),
        Some((changes[1].version, false))
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("prompts/new.md")).unwrap(),
        "content"
    );
}

#[tokio::test]
async fn future_remote_version_is_preserved_and_local_recreation_advances_it() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    let future = PromptFileVersion {
        modified_ms: 4_000_000_000_000,
        counter: 91,
        device_id: Some([7; 8]),
    };
    guard.apply_delete("note.md", future).unwrap();
    assert_eq!(guard.deletions().unwrap()[0].version, future);
    let new = guard.write("note.md", "rebuilt", false).unwrap();
    assert!(new.version.modified_ms > future.modified_ms);
    assert!(guard.deletions().unwrap().is_empty());
}

#[tokio::test]
async fn failed_state_write_leaves_content_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("prompts")).unwrap();
    std::fs::write(tmp.path().join("prompts/note.md"), "original").unwrap();
    std::fs::write(tmp.path().join(".prompt-file-state"), "blocked").unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    assert!(guard.write("note.md", "new", false).is_err());
    assert!(guard.delete("note.md").is_err());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("prompts/note.md")).unwrap(),
        "original"
    );
}

#[tokio::test]
async fn same_millisecond_prepared_inbound_version_does_not_claim_old_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    let old = PromptFileVersion {
        modified_ms: 1000,
        counter: 1,
        device_id: Some([1; 8]),
    };
    guard.apply_write("note.md", "old", old, None).unwrap();
    let path = state_file(tmp.path());
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["live_version"] = json!(old);
    state["live_hash"] = state["prepared_live_hash"].clone();
    state["prepared_live_version"] = json!(PromptFileVersion { counter: 2, ..old });
    use sha2::{Digest, Sha256};
    state["prepared_live_hash"] = json!(format!("{:x}", Sha256::digest(b"new")));
    rua_config::atomic_write_json(&path, &state).unwrap();
    drop(guard);
    let recovered = AgentPromptFileStore::new(tmp.path()).lock().await;
    assert_eq!(
        recovered.local_revision("note.md").unwrap(),
        Some((old, false))
    );
}

#[tokio::test]
async fn failed_rename_keeps_source_version_recoverable() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(tmp.path()).lock().await;
    let old = guard.write("old.md", "old", false).unwrap();
    assert!(guard.rename("old.md", "missing/new.md").is_err());
    drop(guard);
    let recovered = AgentPromptFileStore::new(tmp.path()).lock().await;
    let (version, deleted) = recovered.local_revision("old.md").unwrap().unwrap();
    assert!(!deleted);
    assert!(version.modified_ms >= old.version.modified_ms);
    assert!(recovered.deletions().unwrap().is_empty());
}
