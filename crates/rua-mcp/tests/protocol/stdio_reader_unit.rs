use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use mcp_manager::error::McpError;
use tokio::io::{duplex, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};
use tokio::sync::oneshot;

use rua_mcp::transports::stdio_reader::{PendingMap, ReaderTask};

fn pending_map() -> PendingMap {
    Arc::new(Mutex::new(HashMap::new()))
}

#[tokio::test]
async fn reader_dispatches_response_by_id() {
    let pending: PendingMap = pending_map();
    let (mut writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(7, otx);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "test".into());

    let resp = serde_json::json!({"jsonrpc":"2.0","id":7,"result":{"ok":true,"value":42}});
    let mut line = serde_json::to_string(&resp).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire in time")
        .expect("oneshot sender dropped before send");
    let value = received.expect("dispatch should produce Ok");
    assert_eq!(value["ok"], serde_json::json!(true));
    assert_eq!(value["value"], serde_json::json!(42));

    assert!(
        pending.lock().unwrap().is_empty(),
        "pending map was not cleaned"
    );

    drop(writer);
    let _ = handle.await;
}

#[tokio::test]
async fn reader_drops_notification_with_log() {
    let pending: PendingMap = pending_map();
    let (mut writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(1, otx);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "test".into());

    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {"progress": 0.5}
    });
    let mut nline = serde_json::to_string(&notif).unwrap();
    nline.push('\n');
    writer.write_all(nline.as_bytes()).await.unwrap();

    let resp = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"after":"notif"}});
    let mut rline = serde_json::to_string(&resp).unwrap();
    rline.push('\n');
    writer.write_all(rline.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let value = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire")
        .expect("sender dropped")
        .expect("dispatch must succeed");
    assert_eq!(value["after"], serde_json::json!("notif"));

    assert!(
        pending.lock().unwrap().is_empty(),
        "notification leaked entry"
    );

    drop(writer);
    let _ = handle.await;
}

#[tokio::test]
async fn reader_drops_invalid_json_and_still_dispatches_later_response() {
    let pending: PendingMap = pending_map();
    let (mut writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(2, otx);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "srv-invalid".into());

    writer.write_all(b"{not json\n").await.unwrap();
    writer.write_all(b"\n").await.unwrap();

    let resp = serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"after":"invalid"}});
    let mut line = serde_json::to_string(&resp).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let value = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire after invalid JSON")
        .expect("sender dropped")
        .expect("dispatch must succeed after invalid JSON");
    assert_eq!(value["after"], serde_json::json!("invalid"));
    assert!(pending.lock().unwrap().is_empty());

    drop(writer);
    let _ = handle.await;
}

#[tokio::test]
async fn reader_ignores_unknown_response_id_without_draining_pending() {
    let pending: PendingMap = pending_map();
    let (mut writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(3, otx);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "srv-unknown-id".into());

    let unknown = serde_json::json!({"jsonrpc":"2.0","id":999,"result":{"ignored":true}});
    let mut unknown_line = serde_json::to_string(&unknown).unwrap();
    unknown_line.push('\n');
    writer.write_all(unknown_line.as_bytes()).await.unwrap();

    assert!(
        pending.lock().unwrap().contains_key(&3),
        "unknown response id must not remove unrelated pending requests"
    );

    let expected = serde_json::json!({"jsonrpc":"2.0","id":3,"result":{"ok":true}});
    let mut expected_line = serde_json::to_string(&expected).unwrap();
    expected_line.push('\n');
    writer.write_all(expected_line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let value = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire after unknown id")
        .expect("sender dropped")
        .expect("expected id should still succeed");
    assert_eq!(value["ok"], serde_json::json!(true));
    assert!(pending.lock().unwrap().is_empty());

    drop(writer);
    let _ = handle.await;
}

#[tokio::test]
async fn reader_dispatches_json_rpc_error_to_waiting_request() {
    let pending: PendingMap = pending_map();
    let (mut writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(4, otx);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "srv-error".into());

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "error": {"code": -32602, "message": "invalid tool args"}
    });
    let mut line = serde_json::to_string(&resp).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire for JSON-RPC error")
        .expect("sender dropped");
    let err = outcome.expect_err("JSON-RPC error must map to McpError");
    match err {
        McpError::ProtocolError { server, message } => {
            assert_eq!(server, "srv-error");
            assert_eq!(message, "invalid tool args");
        }
        other => panic!("expected ProtocolError, got {other:?}"),
    }
    assert!(pending.lock().unwrap().is_empty());

    drop(writer);
    let _ = handle.await;
}

#[tokio::test]
async fn reader_fails_pending_on_eof() {
    let pending: PendingMap = pending_map();
    let (writer, reader) = duplex(4096);
    let stdout = BufReader::new(reader);

    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();
    pending.lock().unwrap().insert(11, tx1);
    pending.lock().unwrap().insert(12, tx2);

    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "srv-eof".into());

    drop(writer);

    for rx in [rx1, rx2] {
        let outcome = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect("oneshot did not fire on EOF")
            .expect("sender dropped without sending");
        let err = outcome.expect_err("EOF must produce an error");
        assert!(
            matches!(err, McpError::TransportClosed { .. }),
            "expected TransportClosed, got {err:?}",
        );
    }

    let _ = handle.await;
    assert!(
        pending.lock().unwrap().is_empty(),
        "pending map must be drained on EOF"
    );
}

#[tokio::test]
async fn reader_fails_pending_on_io_error() {
    struct ErrReader;

    impl AsyncRead for ErrReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("synthetic stdout read failure")))
        }
    }

    let pending: PendingMap = pending_map();
    let (otx, orx) = oneshot::channel();
    pending.lock().unwrap().insert(99, otx);

    let stdout = BufReader::new(ErrReader);
    let handle = ReaderTask::spawn(stdout, Arc::clone(&pending), "srv-ioerr".into());

    let outcome = tokio::time::timeout(Duration::from_secs(2), orx)
        .await
        .expect("oneshot did not fire on IO error")
        .expect("sender dropped without sending");
    let err = outcome.expect_err("IO error must produce an error");
    assert!(
        matches!(err, McpError::TransportClosed { .. }),
        "expected TransportClosed, got {err:?}",
    );

    let _ = handle.await;
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn aborted_reader_fails_pending_calls_without_waiting_for_stdout_eof() {
    let (_writer, reader) = tokio::io::duplex(64);
    let pending: rua_mcp::transports::stdio_reader::PendingMap =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    pending.lock().unwrap().insert(1, sender);
    let handle = rua_mcp::transports::stdio_reader::ReaderTask::spawn(
        tokio::io::BufReader::new(reader),
        pending.clone(),
        "aborted".into(),
    );
    handle.abort();
    let _ = handle.await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), receiver)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(pending.lock().unwrap().is_empty());
}
