use rua_core::chat::ChatAddress;
use rua_server::services::chat_deletion::ChatDeleteIntentStore;

fn address() -> ChatAddress {
    ChatAddress::agent("default", "chat-a")
}

#[test]
fn persisted_intent_is_portable_and_roundtrips() {
    let temp = tempfile::tempdir().unwrap();
    let store = ChatDeleteIntentStore::from_rua_root(temp.path());

    let intent = store.persist(&address()).unwrap();
    let loaded = store.load(&address()).unwrap().unwrap();

    assert_eq!(loaded, intent);
    let value = serde_json::to_value(loaded).unwrap();
    assert!(value.get("address").is_some());
    assert!(value.get("intent_id").is_some());
    assert!(value.get("deleted_at").is_some());
    assert!(value.get("scope_root").is_none());
    assert!(value.get("tool_recall_keys").is_none());
    assert!(value.get("media_paths").is_none());
}

#[test]
fn repeated_persist_reuses_the_terminal_intent() {
    let temp = tempfile::tempdir().unwrap();
    let store = ChatDeleteIntentStore::from_rua_root(temp.path());

    let first = store.persist(&address()).unwrap();
    let second = store.persist(&address()).unwrap();

    assert_eq!(second, first);
}

#[test]
fn persistence_failure_does_not_create_an_intent() {
    let temp = tempfile::tempdir().unwrap();
    let sync_path = temp.path().join("sync");
    std::fs::write(&sync_path, "not a directory").unwrap();
    let store = ChatDeleteIntentStore::from_rua_root(temp.path());

    assert!(store.persist(&address()).is_err());
    assert!(store.load(&address()).is_err());
}
