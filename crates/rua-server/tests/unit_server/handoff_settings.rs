#[path = "../../src/services/conversation_input_intake/handoff_settings.rs"]
mod settings;

use rua_core::ChatAddress;
use serde_json::json;

#[test]
fn readiness_reads_applied_portable_settings_and_explicit_empty_rules() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("handoff");
    let expected = settings::capture(source.path(), &address, "agent").unwrap();
    let target_paths = settings::paths(target.path(), &address, "agent").unwrap();
    rua_config::atomic_write_json(&target_paths[0], &json!([])).unwrap();
    assert!(settings::matches(target.path(), &address, "agent", &expected).unwrap());
    for root in [source.path(), target.path()] {
        let paths = settings::paths(root, &address, "agent").unwrap();
        rua_config::atomic_write_json(&paths[1], &json!({"tool_guard":{"approval_mode":"ask_first"},"allow_no_auth_hosts":[root.to_string_lossy()]})).unwrap();
        rua_config::atomic_write_json(&paths[2], &json!({"user_timezone":root.to_string_lossy(),"security":{"skill_scanner":{"enabled":true},"allow_no_auth_hosts":[root.to_string_lossy()],"tool_guard":{"approval_mode":"different"}}})).unwrap();
    }
    let expected = settings::capture(source.path(), &address, "agent").unwrap();
    assert!(settings::matches(target.path(), &address, "agent", &expected).unwrap());
    rua_config::atomic_write_json(
        &target_paths[1],
        &json!({"tool_guard":{"approval_mode":"hands_off"}}),
    )
    .unwrap();
    assert!(!settings::matches(target.path(), &address, "agent", &expected).unwrap());
    assert_eq!(
        settings::capture(source.path(), &address, "agent").unwrap(),
        expected,
        "readiness must never copy the peer or recapture Chat configuration"
    );
}

#[test]
fn changed_or_unreadable_settings_prevent_release_without_writing_the_journal() {
    let root = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("handoff");
    let paths = settings::paths(root.path(), &address, "agent").unwrap();
    rua_config::atomic_write_json(
        &paths[1],
        &json!({"tool_guard":{"approval_mode":"ask_first"}}),
    )
    .unwrap();
    let expected = settings::capture(root.path(), &address, "agent").unwrap();
    rua_config::remove_file_committed(&paths[1]).unwrap();
    let journal = root.path().join("released.json");
    let result = settings::with_unchanged(root.path(), &address, "agent", &expected, || {
        rua_config::atomic_write(&journal, b"released")
    });
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    assert!(!journal.exists());
    rua_config::atomic_write(&paths[0], b"invalid").unwrap();
    assert!(settings::capture(root.path(), &address, "agent").is_err());
    assert!(!journal.exists());
    rua_config::atomic_write_json(&paths[0], &json!([])).unwrap();
    let expected = settings::capture(root.path(), &address, "agent").unwrap();
    settings::with_unchanged(root.path(), &address, "agent", &expected, || {
        rua_config::atomic_write(&journal, b"released")
    })
    .unwrap();
    assert_eq!(std::fs::read(journal).unwrap(), b"released");
}

#[tokio::test]
async fn scoped_settings_publication_preserves_snapshot_clocks_and_omits_history() {
    use rua_server::sync_domain_catalog::{
        build_publication_changes, PublicationChangeBuildContext,
    };
    use rua_server::sync_publication::SyncPublicationFact;
    use rua_sync::event::SyncMeta;
    use rua_sync::hlc::HlcTimestamp;
    let root = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("handoff");
    let paths = settings::paths(root.path(), &address, "agent").unwrap();
    let epoch = HlcTimestamp {
        wall_ms: 100,
        counter: 1,
        device_id: [1; 8],
    };
    rua_config::atomic_write_json(&paths[0].with_file_name("session.json"), &json!({"chat_id":"handoff","authority_placement":{"device_id":"source","authority_epoch":epoch.to_sortable_string()}})).unwrap();
    rua_config::atomic_write_json(&paths[0], &json!([])).unwrap();
    rua_config::atomic_write_json(
        &paths[1],
        &json!({"tool_guard":{"approval_mode":"ask_first"}}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &paths[2],
        &json!({"security":{"skill_scanner":{"enabled":true}}}),
    )
    .unwrap();
    for path in &paths {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(2))
            .unwrap();
    }
    let changes = build_publication_changes(
        SyncPublicationFact::HandoffSettingsSnapshot {
            scope_root: rua_core::ChatStore::resolve(root.path(), address.clone())
                .root()
                .to_path_buf(),
            address,
            agent_id: "agent".into(),
        },
        SyncMeta {
            device_id: "source".into(),
            hlc: HlcTimestamp {
                wall_ms: 999999,
                counter: 0,
                device_id: [1; 8],
            },
            modified_at: String::new(),
        },
        PublicationChangeBuildContext {
            data_dir: root.path(),
            blob_context: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().all(|change| change.hlc.wall_ms == 2000));
    assert!(changes.iter().any(
        |change| change.record_id == "chats/handoff/permission_rules"
            && change.payload["rules"] == json!([])
    ));
    assert!(changes
        .iter()
        .all(|change| !change.record_id.ends_with("/session")
            && !change.record_id.contains("turn_index")));
}
