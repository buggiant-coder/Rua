use std::path::Path;

use rua_workspace::chats_store::ensure_chat_entry;
use rusqlite::Connection;

#[derive(Debug)]
struct ChatRow {
    id: String,
    name: String,
    user_id: String,
    channel: String,
    created_at: String,
}

fn read_chat_rows(ws: &Path) -> Vec<ChatRow> {
    let conn = Connection::open(ws.join("chat-index.sqlite")).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, user_id, channel, created_at
             FROM chat_index ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(ChatRow {
            id: row.get("id")?,
            name: row.get("name")?,
            user_id: row.get("user_id")?,
            channel: row.get("channel")?,
            created_at: row.get("created_at")?,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

#[tokio::test]
async fn creates_entry_on_empty_chat_index() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    ensure_chat_entry(ws, "chat-1", "dingtalk", "user-a", None)
        .await
        .unwrap();

    let rows = read_chat_rows(ws);
    assert_eq!(rows.len(), 1);
    let entry = &rows[0];
    assert_eq!(entry.id, "chat-1");
    assert_eq!(entry.channel, "dingtalk");
    assert_eq!(entry.user_id, "user-a");
    assert_eq!(entry.name, "dingtalk chat");
    assert!(!entry.created_at.is_empty());
}

#[tokio::test]
async fn skips_duplicate_chat_id() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    ensure_chat_entry(ws, "chat-1", "dingtalk", "user-a", None)
        .await
        .unwrap();
    ensure_chat_entry(ws, "chat-1", "dingtalk", "user-a", None)
        .await
        .unwrap();

    let rows = read_chat_rows(ws);
    assert_eq!(
        rows.len(),
        1,
        "duplicate chat_id should not create a second entry"
    );
}

#[tokio::test]
async fn creates_chat_index_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(!ws.join("chat-index.sqlite").exists());

    ensure_chat_entry(ws, "chat-new", "feishu", "user-b", None)
        .await
        .unwrap();

    assert!(ws.join("chat-index.sqlite").exists());
    let rows = read_chat_rows(ws);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel, "feishu");
}

#[tokio::test]
async fn different_chats_create_separate_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    ensure_chat_entry(ws, "chat-1", "dingtalk", "user-a", None)
        .await
        .unwrap();
    ensure_chat_entry(ws, "chat-2", "feishu", "user-b", None)
        .await
        .unwrap();

    let rows = read_chat_rows(ws);
    assert_eq!(rows.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_do_not_clobber_chat_index() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let writes = (0..24).map(|idx| {
        let ws = ws.clone();
        tokio::spawn(async move {
            ensure_chat_entry(
                &ws,
                &format!("chat-{idx:02}"),
                "console",
                &format!("user-{idx:02}"),
                None,
            )
            .await
            .unwrap();
        })
    });

    for write in writes {
        write.await.expect("join concurrent chat write");
    }

    let rows = read_chat_rows(&ws);
    let chat_ids: Vec<_> = rows.iter().map(|entry| entry.id.as_str()).collect();

    assert_eq!(chat_ids.len(), 24, "every concurrent chat create must land");
    assert_eq!(chat_ids.first(), Some(&"chat-00"));
    assert_eq!(chat_ids.last(), Some(&"chat-23"));
}

#[tokio::test]
async fn sender_name_sets_display_name_and_user_id() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    ensure_chat_entry(ws, "chat-1", "dingtalk", "raw-id-123", Some("雨农"))
        .await
        .unwrap();

    let rows = read_chat_rows(ws);
    assert_eq!(rows.len(), 1);
    let entry = &rows[0];
    assert_eq!(entry.name, "dingtalk: 雨农");
    assert_eq!(entry.user_id, "雨农");
}

#[tokio::test]
async fn empty_sender_name_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    ensure_chat_entry(ws, "chat-1", "dingtalk", "raw-id-123", Some(""))
        .await
        .unwrap();

    let rows = read_chat_rows(ws);
    let entry = &rows[0];
    assert_eq!(entry.name, "dingtalk chat");
    assert_eq!(entry.user_id, "raw-id-123");
}
