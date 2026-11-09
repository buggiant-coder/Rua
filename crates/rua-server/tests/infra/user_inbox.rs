use rua_server::services::user_inbox::{InboxDelivery, InboxSender, UserInbox};

#[tokio::test]
async fn delivered_message_survives_reopen_with_uniform_presentation_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inbox = UserInbox::open(temp.path()).expect("open inbox");

    let delivered = inbox
        .deliver(InboxDelivery {
            sender: InboxSender {
                kind: "release-monitor".to_string(),
                id: Some("stable".to_string()),
                display_name: "Release Monitor".to_string(),
            },
            title: "Version 2 is available".to_string(),
            body_markdown: "See the **release notes** for details.".to_string(),
        })
        .await
        .expect("deliver message");

    assert!(delivered.read_at.is_none());
    drop(inbox);

    let reopened = UserInbox::open(temp.path()).expect("reopen inbox");
    let messages = reopened.list_all().await;
    assert_eq!(messages, vec![delivered]);
}

#[tokio::test]
async fn inbox_supports_pagination_read_state_and_delete_lifecycle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inbox = UserInbox::open(temp.path()).expect("open inbox");
    let first = inbox.deliver(delivery("First")).await.expect("first");
    let second = inbox.deliver(delivery("Second")).await.expect("second");
    let third = inbox.deliver(delivery("Third")).await.expect("third");

    let page = inbox.list(None, 2, None).await.expect("first page");
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![third.id.as_str(), second.id.as_str()]
    );
    assert_eq!(page.unread_count, 3);
    let next = page.next_cursor.expect("next cursor");

    let page = inbox.list(Some(&next), 2, None).await.expect("next page");
    assert_eq!(page.items, vec![first.clone()]);
    assert!(page.next_cursor.is_none());

    let read = inbox.set_read(&second.id, true).await.expect("mark read");
    assert!(read.read_at.is_some());
    assert_eq!(inbox.unread_count().await, 2);
    assert_eq!(
        inbox
            .list(None, 10, Some(false))
            .await
            .expect("unread")
            .items
            .len(),
        2
    );

    let unread = inbox
        .set_read(&second.id, false)
        .await
        .expect("mark unread");
    assert!(unread.read_at.is_none());
    assert_eq!(inbox.mark_all_read().await.expect("mark all read"), 3);
    assert_eq!(inbox.unread_count().await, 0);
    assert_eq!(
        inbox.mark_all_read().await.expect("repeat mark all read"),
        0
    );
    assert!(inbox.delete(&second.id).await.expect("delete"));
    assert!(inbox.get(&second.id).await.is_none());
    assert!(!inbox.delete(&second.id).await.expect("repeat delete"));
}

#[tokio::test]
async fn persistence_preserves_unknown_document_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("user_inbox.json"),
        serde_json::json!({
            "schema_version": 7,
            "messages": [],
            "future_metadata": { "value": 42 }
        })
        .to_string(),
    )
    .expect("seed inbox document");

    let inbox = UserInbox::open(temp.path()).expect("open inbox");
    inbox.deliver(delivery("Preserve")).await.expect("deliver");

    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("user_inbox.json")).expect("read inbox document"),
    )
    .expect("parse inbox document");
    assert_eq!(persisted["schema_version"], 7);
    assert_eq!(persisted["future_metadata"]["value"], 42);
}

fn delivery(title: &str) -> InboxDelivery {
    InboxDelivery {
        sender: InboxSender {
            kind: "test".to_string(),
            id: None,
            display_name: "Test Sender".to_string(),
        },
        title: title.to_string(),
        body_markdown: format!("**{title}** body"),
    }
}
