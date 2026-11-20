use rua_core::{ChatAddress, ChatStore, RuaRoot, WorkspaceRoot};

#[test]
fn chats_share_one_index_and_media_workspace_with_separate_session_directories() {
    let data_dir = tempfile::tempdir().unwrap();
    let first = ChatStore::resolve(data_dir.path(), ChatAddress::new("first"));
    let second = ChatStore::resolve(data_dir.path(), ChatAddress::new("second"));
    assert_eq!(first.root(), data_dir.path().join("chats"));
    assert_eq!(first.root(), second.root());
    assert_eq!(first.chat_index_path(), second.chat_index_path());
    assert_eq!(
        first.evergreen_chat_root(),
        data_dir.path().join("chats/sessions/first")
    );
    assert_ne!(first.evergreen_chat_root(), second.evergreen_chat_root());
    assert!(!first.root().exists());
}

#[test]
fn materialization_adapter_uses_a_resolved_chat_workspace() {
    let data_dir = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(data_dir.path()).chat_workspace();
    let staging = WorkspaceRoot::new(root.as_path().join(".materializing/first-attempt"));
    let store = ChatStore::at_workspace(staging.clone(), ChatAddress::new("first"));
    assert_eq!(store.root(), staging.as_path());
    assert_eq!(
        store.evergreen_chat_root(),
        staging.sessions_dir().join("first")
    );
}

#[test]
fn new_storage_does_not_adopt_either_legacy_session_with_the_same_uuid() {
    let root = tempfile::tempdir().unwrap();
    let id = "82ff180d-bdf5-47c6-a116-6cc6cf8b78d3";
    for legacy in ["workspaces/agent-a", "projects/project-b"] {
        let dir = root.path().join(legacy).join("sessions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), legacy).unwrap();
    }

    let current = ChatStore::resolve(root.path(), ChatAddress::new(id));
    assert_eq!(
        current.evergreen_chat_root(),
        root.path().join("chats/sessions").join(id)
    );
    assert!(!current.evergreen_chat_root().exists());
    for legacy in ["workspaces/agent-a", "projects/project-b"] {
        let header = root
            .path()
            .join(legacy)
            .join("sessions")
            .join(id)
            .join("session.json");
        assert_eq!(std::fs::read_to_string(header).unwrap(), legacy);
    }
}
