use super::support::{recv, reply_process};
use rua_channel::core::channel::Channel;
use rua_channel::imessage::{IMessageChannel, IMessageConfig};
use rusqlite::Connection;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn insert(conn: &Connection, id: i64, text: &str, from_me: bool) {
    conn.execute(
        "INSERT INTO message(ROWID,text,is_from_me,handle_id) VALUES (?1,?2,?3,1)",
        rusqlite::params![id, text, from_me],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_message_join(message_id,chat_id) VALUES (?1,1)",
        [id],
    )
    .unwrap();
}

#[tokio::test]
async fn poller_skips_history_replies_via_imsg_and_can_restart() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("chat.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE message(text TEXT, is_from_me INTEGER, handle_id INTEGER);
        CREATE TABLE chat(title TEXT);
        CREATE TABLE chat_message_join(message_id INTEGER, chat_id INTEGER);
        CREATE TABLE handle(id TEXT);
        INSERT INTO handle(ROWID,id) VALUES(1,'alice@example.test');
        INSERT INTO chat(ROWID,title) VALUES(1,'Alice');
    ",
    )
    .unwrap();
    insert(&conn, 1, "history", false);
    let executable = directory.path().join("imsg");
    let log = directory.path().join("sent");
    // A local fake command: it writes its arguments, never accesses Messages.
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/sent\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let channel = IMessageChannel::new(IMessageConfig {
        db_path: db.to_string_lossy().into(),
        poll_sec: 0.02,
        bot_prefix: "[Rua]".into(),
        ..Default::default()
    })
    .with_imsg_path(executable)
    .with_acl_config(json!({"bot_prefix":"[Rua]"}));
    let (process, mut requests) = reply_process("answer");
    for id in [2, 5] {
        channel.start(process.clone()).await.unwrap();
        insert(&conn, id, "fresh input without an incoming prefix", false);
        let request = recv(&mut requests).await;
        assert_eq!(request.user_id, "alice@example.test");
        assert_eq!(request.message_id, Some(id.to_string()));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read_to_string(&log).is_ok_and(|text| text.contains("[Rua]  answer")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        assert!(sent.contains("--to\nalice@example.test"));
        channel.stop().await.unwrap();
        if id == 2 {
            insert(&conn, 3, "while stopped", false);
            insert(&conn, 4, "own message", true);
            std::fs::remove_file(&log).unwrap();
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), requests.recv())
            .await
            .is_err()
    );
}
