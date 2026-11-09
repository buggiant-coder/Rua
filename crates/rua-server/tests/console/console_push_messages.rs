use rua_server::endpoints::console::PushMessagesQuery;
use rua_server::endpoints::dto::PushMessage;
use rua_server::endpoints::messages::PushMessageStore;
use serde_json::json;

#[test]
fn push_messages_query_deserializes_session_id() {
    let q: PushMessagesQuery = serde_json::from_str(r#"{"session_id": "abc123"}"#).unwrap();
    assert_eq!(q.session_id.as_deref(), Some("abc123"));

    let q: PushMessagesQuery = serde_json::from_str("{}").unwrap();
    assert!(q.session_id.is_none());
}

#[test]
fn push_messages_filter_by_session_id() {
    let mut extra1 = serde_json::Map::new();
    extra1.insert("session_id".into(), serde_json::Value::String("s1".into()));
    let msgs = [
        PushMessage {
            id: "m1".into(),
            content: "hello".into(),
            timestamp: None,
            extra: extra1,
        },
        PushMessage {
            id: "m2".into(),
            content: "world".into(),
            timestamp: None,
            extra: serde_json::Map::new(),
        },
    ];

    let filtered: Vec<&PushMessage> = msgs
        .iter()
        .filter(|m| {
            m.extra
                .get("session_id")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "s1")
        })
        .collect();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "m1");
}

fn push_message(id: &str, text: &str) -> PushMessage {
    PushMessage {
        id: id.into(),
        content: text.into(),
        timestamp: None,
        extra: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn durable_push_messages_survive_store_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let store = PushMessageStore::new(Some(temp.path().to_path_buf()));
    store.push(push_message("push-1", "persist me")).await;

    let reopened = PushMessageStore::new(Some(temp.path().to_path_buf()));
    let messages = reopened.list().await;

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "push-1");
    assert_eq!(messages[0].content, "persist me");
}

#[tokio::test]
async fn duplicate_push_message_ids_are_idempotent_across_retries() {
    let temp = tempfile::tempdir().unwrap();
    let store = PushMessageStore::new(Some(temp.path().to_path_buf()));

    store.push(push_message("retry-1", "first delivery")).await;
    store.push(push_message("retry-1", "retry delivery")).await;

    let messages = store.list().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "retry-1");
    assert_eq!(messages[0].content, "first delivery");

    let reopened = PushMessageStore::new(Some(temp.path().to_path_buf()));
    assert_eq!(reopened.list().await.len(), 1);
}

#[tokio::test]
async fn ephemeral_push_messages_are_not_persisted_or_listed() {
    let temp = tempfile::tempdir().unwrap();
    let store = PushMessageStore::new(Some(temp.path().to_path_buf()));
    let mut msg = push_message("live-typing-1", "typing...");
    msg.extra
        .insert("delivery_class".into(), json!("ephemeral"));

    store.push(msg).await;

    assert!(store.list().await.is_empty());
    let reopened = PushMessageStore::new(Some(temp.path().to_path_buf()));
    assert!(reopened.list().await.is_empty());
}

#[tokio::test]
async fn push_message_store_prunes_oldest_messages() {
    let temp = tempfile::tempdir().unwrap();
    let store = PushMessageStore::new(Some(temp.path().to_path_buf()));

    for index in 0..510 {
        store
            .push(push_message(
                &format!("push-{index}"),
                &format!("message {index}"),
            ))
            .await;
    }

    let messages = store.list().await;
    assert_eq!(messages.len(), 500);
    assert_eq!(messages[0].id, "push-10");
    assert_eq!(messages[499].id, "push-509");

    let reopened = PushMessageStore::new(Some(temp.path().to_path_buf()));
    assert_eq!(reopened.list().await.len(), 500);
}
