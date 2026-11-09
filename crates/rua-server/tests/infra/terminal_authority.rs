use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use rua_server::services::active_stream_broadcast::{
    ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub, ActiveStreamOwner,
};
use rua_server::services::terminal::{
    route_terminal_execution_device, terminal_activity_broadcast_after_seq,
    terminal_activity_frame_json, terminal_activity_from_remote_stream_frame,
    NativeTerminalProcessSpawner, OpenTerminalSessionRequest, TerminalActivityFrame,
    TerminalAuthorityConfig, TerminalError, TerminalExecutionDevice, TerminalPalette,
    TerminalPlacementRoute, TerminalProcessControl, TerminalProcessHandle, TerminalProcessSpawner,
    TerminalPtyGeometry, TerminalSessionAuthority, TerminalStreamCursor, TerminalViewerIdentity,
};
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

fn terminal_replay_cursors(session_id: &str, after_seq: u64) -> BTreeMap<String, u64> {
    BTreeMap::from([(format!("terminal-activity:{session_id}"), after_seq)])
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.old.as_ref() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

struct FakeProcessControl {
    killed: Arc<Mutex<bool>>,
    inputs: Arc<Mutex<Vec<Vec<u8>>>>,
    geometry_changes: Arc<Mutex<Vec<TerminalPtyGeometry>>>,
}

impl TerminalProcessControl for FakeProcessControl {
    fn kill(&self) {
        *self.killed.lock().expect("kill flag lock") = true;
    }

    fn write_input(
        &self,
        data: &[u8],
    ) -> Result<(), rua_server::services::terminal::TerminalError> {
        self.inputs.lock().expect("inputs lock").push(data.to_vec());
        Ok(())
    }

    fn resize_pty(
        &self,
        geometry: TerminalPtyGeometry,
    ) -> Result<(), rua_server::services::terminal::TerminalError> {
        self.geometry_changes
            .lock()
            .expect("geometry lock")
            .push(geometry);
        Ok(())
    }
}

#[derive(Clone)]
struct FakeProcessSpawner {
    killed: Arc<Mutex<bool>>,
    inputs: Arc<Mutex<Vec<Vec<u8>>>>,
    geometry_changes: Arc<Mutex<Vec<TerminalPtyGeometry>>>,
    spawned_cwd: Arc<Mutex<Option<PathBuf>>>,
    output_txs: Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>>,
}

impl FakeProcessSpawner {
    fn new() -> Self {
        Self {
            killed: Arc::new(Mutex::new(false)),
            inputs: Arc::new(Mutex::new(Vec::new())),
            geometry_changes: Arc::new(Mutex::new(Vec::new())),
            spawned_cwd: Arc::new(Mutex::new(None)),
            output_txs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn was_killed(&self) -> bool {
        *self.killed.lock().expect("kill flag lock")
    }

    fn spawned_cwd(&self) -> Option<PathBuf> {
        self.spawned_cwd.lock().expect("cwd lock").clone()
    }

    fn inputs(&self) -> Vec<Vec<u8>> {
        self.inputs.lock().expect("inputs lock").clone()
    }

    fn take_inputs(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.inputs.lock().expect("inputs lock"))
    }

    fn emit(&self, data: &[u8]) {
        self.output_txs
            .lock()
            .expect("output tx lock")
            .last()
            .expect("terminal output sender")
            .send(data.to_vec())
            .expect("send terminal output");
    }

    fn geometry_changes(&self) -> Vec<TerminalPtyGeometry> {
        self.geometry_changes.lock().expect("geometry lock").clone()
    }
}

impl TerminalProcessSpawner for FakeProcessSpawner {
    fn spawn(
        &self,
        request: rua_server::services::terminal::TerminalSpawnRequest,
    ) -> Result<TerminalProcessHandle, rua_server::services::terminal::TerminalError> {
        *self.spawned_cwd.lock().expect("cwd lock") = Some(request.cwd);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(b"terminal-ready\n".to_vec()).expect("send replay");
        self.output_txs.lock().expect("output tx lock").push(tx);
        Ok(TerminalProcessHandle {
            output: rx,
            control: Arc::new(FakeProcessControl {
                killed: Arc::clone(&self.killed),
                inputs: Arc::clone(&self.inputs),
                geometry_changes: Arc::clone(&self.geometry_changes),
            }),
        })
    }
}

#[derive(Clone)]
struct LiveOutputSpawner {
    output_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>>,
}

impl LiveOutputSpawner {
    fn new() -> Self {
        Self {
            output_tx: Arc::new(Mutex::new(None)),
        }
    }

    fn emit(&self, data: &[u8]) {
        self.output_tx
            .lock()
            .expect("output tx lock")
            .as_ref()
            .expect("terminal output sender")
            .send(data.to_vec())
            .expect("send live terminal output");
    }

    fn close(&self) {
        self.output_tx.lock().expect("output tx lock").take();
    }
}

impl TerminalProcessSpawner for LiveOutputSpawner {
    fn spawn(
        &self,
        _request: rua_server::services::terminal::TerminalSpawnRequest,
    ) -> Result<TerminalProcessHandle, rua_server::services::terminal::TerminalError> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.output_tx.lock().expect("output tx lock") = Some(tx);
        Ok(TerminalProcessHandle {
            output: rx,
            control: Arc::new(FakeProcessControl {
                killed: Arc::new(Mutex::new(false)),
                inputs: Arc::new(Mutex::new(Vec::new())),
                geometry_changes: Arc::new(Mutex::new(Vec::new())),
            }),
        })
    }
}

async fn next_terminal_activity_payload(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ActiveStreamBroadcastFrame>,
) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("terminal activity stream frame")
            .expect("terminal activity stream open");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            return serde_json::from_str(&payload).expect("terminal activity payload json");
        }
    }
}

fn active_broadcast_event_data(frame: &ActiveStreamBroadcastFrame) -> Option<String> {
    let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            let encoded = value
                .get("data_base64")
                .and_then(|data| data.as_str())
                .map(str::to_string)?;
            BASE64_STANDARD
                .decode(encoded)
                .ok()
                .and_then(|data| String::from_utf8(data).ok())
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn local_authority_opens_lists_attaches_detaches_and_kills_session() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let viewer = TerminalViewerIdentity::local("browser-a");

    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: viewer.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    assert_eq!(spawner.spawned_cwd().as_deref(), Some(home.path()));
    assert_eq!(opened.viewer_count, 1);
    assert_eq!(opened.input_lease_holder.as_ref(), Some(&viewer));

    let sessions = authority.list().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, opened.session_id);

    let attached = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach second viewer");
    assert!(
        attached.snapshot.data.contains("terminal-ready"),
        "new viewer should receive one coherent terminal snapshot"
    );
    assert_eq!(attached.snapshot.cursor, attached.live_cursor);
    assert!(attached.tail.is_empty());

    authority
        .detach(&opened.session_id, &viewer)
        .await
        .expect("detach creator");
    let sessions = authority.list().await;
    assert_eq!(sessions[0].viewer_count, 1);
    assert!(!spawner.was_killed(), "detach must not kill the PTY");

    let second = TerminalViewerIdentity::local("browser-b");
    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease before kill");
    authority
        .kill(&opened.session_id, &second)
        .await
        .expect("kill session");
    assert!(spawner.was_killed(), "kill must terminate the PTY");
    assert!(
        authority.list().await.is_empty(),
        "killed sessions are not live sessions"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn authority_xterm_is_the_only_terminal_reply_producer() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    authority
        .adapt_pty_geometry(
            &opened.session_id,
            &creator,
            TerminalPtyGeometry { cols: 20, rows: 3 },
        )
        .await
        .expect("set compact authority geometry");
    authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");

    let queries = concat!(
        "\u{1b}[c",
        "\u{1b}[>c",
        "\u{1b}[5n",
        "\u{1b}[6n",
        "\u{1b}[?6n",
        "\u{1b}]10;?\u{7}",
        "\u{1b}]11;?\u{7}",
        "\u{1b}]12;?\u{7}",
    );
    spawner.emit(queries.as_bytes());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if spawner.inputs().len() == 8 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("authority terminal replies");

    let replies = spawner.take_inputs();
    assert_eq!(replies.len(), 8, "many viewers must not duplicate replies");
    assert_eq!(replies[0], b"\x1b[?1;2c");
    assert!(replies[1].starts_with(b"\x1b[>"));
    assert_eq!(replies[2], b"\x1b[0n");
    assert!(replies[3].starts_with(b"\x1b[") && replies[3].ends_with(b"R"));
    assert!(replies[4].starts_with(b"\x1b[?") && replies[4].ends_with(b"R"));
    assert_eq!(replies[5], b"\x1b]10;rgb:e9e9/ecec/efef\x1b\\");
    assert_eq!(replies[6], b"\x1b]11;rgb:2424/2424/2424\x1b\\");
    assert_eq!(replies[7], b"\x1b]12;rgb:e9e9/ecec/efef\x1b\\");

    spawner.emit(b"\x1b[?1004h");
    tokio::time::sleep(Duration::from_millis(50)).await;
    authority
        .update_palette(
            &opened.session_id,
            &creator,
            TerminalPalette {
                foreground: "#101418".to_string(),
                background: "#f8f9fa".to_string(),
                cursor: "#228be6".to_string(),
            },
        )
        .await
        .expect("update authority terminal palette");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if spawner.inputs() == vec![b"\x1b[I".to_vec()] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("focus gained after palette update");
    spawner.take_inputs();
    spawner.emit(b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if spawner.inputs().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("updated terminal color replies");
    assert_eq!(
        spawner.take_inputs(),
        vec![
            b"\x1b]10;rgb:1010/1414/1818\x1b\\".to_vec(),
            b"\x1b]11;rgb:f8f8/f9f9/fafa\x1b\\".to_vec(),
            b"\x1b]12;rgb:2222/8b8b/e6e6\x1b\\".to_vec(),
        ]
    );

    spawner.emit(b"\x1b[2J\x1b[Hone\r\ntwo\r\nthree\x1b[1;3r\x1b[S");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let scrolled = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("serialize top-margin scrollback");
    assert!(
        scrolled.snapshot.data.contains("one"),
        "authority compatibility must retain CSI S leaving rows in scrollback"
    );

    authority
        .detach(&opened.session_id, &creator)
        .await
        .expect("detach creator");
    authority
        .detach(&opened.session_id, &second)
        .await
        .expect("detach second viewer");
    spawner.emit(b"\x1b[6n\x1b]11;?\x07");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if spawner.inputs().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background authority terminal replies");
    let background_replies = spawner.take_inputs();
    assert_eq!(background_replies.len(), 2);
    assert!(background_replies[0].starts_with(b"\x1b[") && background_replies[0].ends_with(b"R"));
    assert_eq!(background_replies[1], b"\x1b]11;rgb:f8f8/f9f9/fafa\x1b\\");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn local_attach_fails_when_authority_snapshot_state_is_unavailable() {
    let _path = EnvGuard::set("PATH", "/rua-test-no-node-runtime");
    let home = tempfile::tempdir().expect("home tempdir");
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(FakeProcessSpawner::new()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    let error = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect_err("attach must require authority snapshot state");

    assert!(matches!(
        error,
        rua_server::services::terminal::TerminalError::SnapshotUnavailable(session_id)
            if session_id == opened.session_id
    ));
    assert_eq!(
        authority.list().await[0].viewer_count,
        1,
        "a failed mandatory attach must not retain the new viewer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn local_authority_lists_sessions_in_creation_order() {
    let home = tempfile::tempdir().expect("home tempdir");
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(FakeProcessSpawner::new()),
    );
    let mut opened_ids = Vec::new();

    for index in 0..12 {
        let opened = authority
            .open(OpenTerminalSessionRequest {
                viewer: TerminalViewerIdentity::local(format!("browser-{index}")),
                cwd: None,
                terminal_profile_id: None,
                scrollback_lines: 100,
            })
            .await
            .expect("open local terminal");
        opened_ids.push(opened.session_id);
    }

    let listed_ids = authority
        .list()
        .await
        .into_iter()
        .map(|session| session.session_id)
        .collect::<Vec<_>>();
    assert_eq!(listed_ids, opened_ids);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_output_replay_preserves_utf8_across_chunk_boundaries() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    let initial = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach before output");
    let mut activity = authority
        .subscribe_activity(&opened.session_id, initial.live_cursor)
        .await
        .expect("subscribe positioned output");
    let line = "────\n".as_bytes();
    spawner.emit(&line[..2]);
    spawner.emit(&line[2..]);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let frame = activity.recv().await.expect("coalesced split output frame");
    assert!(matches!(
        frame,
        TerminalActivityFrame::Output {
            activity_sequence: 0,
            start_offset: 0,
            end_offset,
            ref data,
        } if end_offset == line.len() as u64 && data == line
    ));
    let attached = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-c"),
        )
        .await
        .expect("attach after output");
    assert!(
        attached.snapshot.data.contains("────"),
        "headless xterm snapshot must include background PTY output"
    );
    assert_eq!(attached.snapshot.cursor.next_activity_sequence, 1);
    assert_eq!(
        attached.snapshot.cursor.next_output_offset,
        line.len() as u64
    );
    assert_eq!(attached.snapshot.cursor, attached.live_cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_output_coalescing_respects_the_byte_budget_and_positions() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");
    let attachment = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach before output");
    let mut activity = authority
        .subscribe_activity(&opened.session_id, attachment.live_cursor)
        .await
        .expect("subscribe positioned output");
    let output = vec![b'x'; 64 * 1024 + 17];

    spawner.emit(&output);

    let first = activity.recv().await.expect("first coalesced frame");
    let second = activity.recv().await.expect("remaining coalesced frame");
    assert!(matches!(
        first,
        TerminalActivityFrame::Output {
            activity_sequence: 0,
            start_offset: 0,
            end_offset: 65_536,
            ref data,
        } if data.len() == 65_536
    ));
    assert!(matches!(
        second,
        TerminalActivityFrame::Output {
            activity_sequence: 1,
            start_offset: 65_536,
            end_offset: 65_553,
            ref data,
        } if data.len() == 17
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_output_positions_preserve_non_utf8_pty_bytes() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");
    let attachment = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach before raw bytes");
    let mut activity = authority
        .subscribe_activity(&opened.session_id, attachment.live_cursor)
        .await
        .expect("subscribe raw output");

    let raw = vec![0xff, 0x00, 0x1b, b'['];
    spawner.emit(&raw);
    let frame = activity.recv().await.expect("raw output frame");

    assert_eq!(
        frame,
        TerminalActivityFrame::Output {
            activity_sequence: attachment.live_cursor.next_activity_sequence,
            start_offset: attachment.live_cursor.next_output_offset,
            end_offset: attachment.live_cursor.next_output_offset + raw.len() as u64,
            data: raw,
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn attach_snapshot_cursor_and_concurrent_tail_share_one_boundary() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    let writer = tokio::spawn({
        let spawner = spawner.clone();
        async move {
            for index in 0..200_u16 {
                spawner.emit(&index.to_be_bytes());
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let concurrent = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach during output");
    writer.await.expect("output writer");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let final_attachment = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-c"),
        )
        .await
        .expect("attach after output");

    let snapshot = &concurrent.snapshot;
    assert!(snapshot.cursor.next_output_offset > 0);
    assert!(snapshot.cursor.next_output_offset <= concurrent.live_cursor.next_output_offset);
    assert!(
        concurrent.live_cursor.next_output_offset
            <= final_attachment.live_cursor.next_output_offset,
        "a later attachment cannot move the authority cursor backwards"
    );

    let mut expected_sequence = snapshot.cursor.next_activity_sequence;
    let mut expected_offset = snapshot.cursor.next_output_offset;
    for frame in &concurrent.tail {
        let TerminalActivityFrame::Output {
            activity_sequence,
            start_offset,
            end_offset,
            data,
        } = frame
        else {
            continue;
        };
        assert_eq!(*activity_sequence, expected_sequence);
        assert_eq!(*start_offset, expected_offset);
        assert_eq!(*end_offset - *start_offset, data.len() as u64);
        expected_sequence += 1;
        expected_offset = *end_offset;
    }
    assert_eq!(
        expected_sequence,
        concurrent.live_cursor.next_activity_sequence
    );
    assert_eq!(expected_offset, concurrent.live_cursor.next_output_offset);
    assert_eq!(final_attachment.live_cursor.next_output_offset, 200 * 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn reattach_from_retained_cursor_returns_current_snapshot() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator,
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    spawner.emit(b"snapshot-boundary");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let initial = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("capture initial cursor");

    spawner.emit(b"-tail-a");
    tokio::time::sleep(Duration::from_millis(20)).await;
    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("append control-state activity");
    spawner.emit(b"-tail-b");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resumed = authority
        .attach_with_scrollback(&opened.session_id, second, 100, Some(initial.live_cursor))
        .await
        .expect("reattach from retained cursor");

    assert!(resumed
        .snapshot
        .data
        .contains("snapshot-boundary-tail-a-tail-b"));
    assert_eq!(resumed.snapshot.cursor, resumed.live_cursor);
    assert!(resumed.tail.is_empty());
    assert_eq!(
        resumed.session.input_lease_holder,
        Some(TerminalViewerIdentity::local("browser-b"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn reattach_from_ahead_or_non_boundary_cursor_returns_fresh_snapshot() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    spawner.emit(b"cursor-boundary");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let current = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("capture current cursor");
    let invalid = TerminalStreamCursor {
        next_activity_sequence: current.live_cursor.next_activity_sequence + 1,
        next_output_offset: current.live_cursor.next_output_offset + 1,
    };

    let recovered = authority
        .attach_with_scrollback(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-c"),
            100,
            Some(invalid),
        )
        .await
        .expect("recover invalid cursor");

    assert_ne!(recovered.live_cursor, invalid);
    assert_eq!(recovered.snapshot.cursor, recovered.live_cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn reattach_from_cursor_older_than_replay_retention_returns_fresh_snapshot() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    let stale = TerminalStreamCursor::default();
    let mut activity = authority
        .subscribe_activity(&opened.session_id, stale)
        .await
        .expect("subscribe before retention overflow");
    let output_len = 17 * 1024 * 1024;
    spawner.emit(&vec![b'x'; output_len]);
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut observed_end = 0;
        loop {
            if let Some(TerminalActivityFrame::Output { end_offset, .. }) = activity.recv().await {
                observed_end = end_offset;
            }
            if observed_end == output_len as u64 {
                break;
            }
        }
    })
    .await
    .expect("authority should publish the complete positioned output");

    let recovered = authority
        .attach_with_scrollback(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
            100,
            Some(stale),
        )
        .await
        .expect("recover stale cursor after retention overflow");

    assert!(recovered.live_cursor.next_output_offset > 1024 * 1024);
    assert_eq!(recovered.snapshot.cursor, recovered.live_cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_snapshot_retains_long_line_without_text_trimming() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    let cursor = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("capture stream cursor")
        .live_cursor;
    let output = b"0123456789abcdefWXYZ";
    spawner.emit(output);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resumed = authority
        .attach_with_scrollback(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
            100,
            Some(cursor),
        )
        .await
        .expect("resume terminal output");
    assert!(resumed.snapshot.data.contains("0123456789abcdefWXYZ"));
    assert_eq!(resumed.snapshot.cursor, resumed.live_cursor);
    assert!(resumed.tail.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_snapshot_offsets_are_absolute_without_newline_trimming() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 1,
        })
        .await
        .expect("open local terminal");

    let cursor = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("capture stream cursor")
        .live_cursor;
    spawner.emit(b"first\n");
    spawner.emit(b"second\n");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resumed = authority
        .attach_with_scrollback(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
            1,
            Some(cursor),
        )
        .await
        .expect("resume positioned output");
    assert!(resumed.snapshot.data.contains("first"));
    assert!(resumed.snapshot.data.contains("second"));
    assert_eq!(resumed.snapshot.cursor.next_output_offset, 13);
    assert_eq!(resumed.snapshot.cursor, resumed.live_cursor);
    assert!(resumed.tail.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn updating_scrollback_keeps_absolute_snapshot_cursor() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 3,
        })
        .await
        .expect("open local terminal");

    let cursor = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("capture stream cursor")
        .live_cursor;
    let output = b"first\nsecond\nthird\n";
    spawner.emit(output);
    tokio::time::sleep(Duration::from_millis(20)).await;
    authority
        .set_scrollback_lines(&opened.session_id, 1)
        .await
        .expect("update terminal scrollback");

    let resumed = authority
        .attach_with_scrollback(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
            1,
            Some(cursor),
        )
        .await
        .expect("resume terminal output");
    assert!(resumed.snapshot.data.contains("third"));
    assert_eq!(
        resumed.snapshot.cursor.next_output_offset,
        output.len() as u64
    );
    assert_eq!(resumed.snapshot.cursor, resumed.live_cursor);
    assert!(resumed.tail.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn headless_snapshot_uses_requested_scrollback_lines() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 2,
        })
        .await
        .expect("open local terminal");

    let output = (0..30)
        .map(|index| format!("snapshot-line-{index}\n"))
        .collect::<String>();
    spawner.emit(output.as_bytes());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let attached = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("attach terminal");
    let snapshot = attached.snapshot;
    assert!(!snapshot.data.contains("snapshot-line-0"));
    assert!(snapshot.data.contains("snapshot-line-29"));
    assert_eq!(snapshot.cursor, attached.live_cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn detached_session_is_cleaned_up_after_ttl() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_millis(25),
        },
        Arc::new(spawner.clone()),
    );
    let viewer = TerminalViewerIdentity::local("browser-a");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: viewer.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    authority
        .detach(&opened.session_id, &viewer)
        .await
        .expect("detach only viewer");
    assert!(!spawner.was_killed(), "detach starts TTL but does not kill");
    assert_eq!(
        authority.list().await.len(),
        1,
        "detached sessions remain discoverable during the TTL"
    );

    tokio::time::sleep(Duration::from_millis(60)).await;

    assert!(spawner.was_killed(), "TTL expiry must kill the PTY");
    assert!(
        authority.list().await.is_empty(),
        "TTL-cleaned sessions are removed from the live list"
    );
    assert!(matches!(
        authority
            .attach(
                &opened.session_id,
                TerminalViewerIdentity::local("browser-b"),
            )
            .await,
        Err(TerminalError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn input_lease_stays_with_creator_until_takeover() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");

    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    let attached = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");
    assert_eq!(attached.session.input_lease_holder.as_ref(), Some(&creator));

    let after_takeover = authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease");
    assert_eq!(
        after_takeover.input_lease_holder.as_ref(),
        Some(&second),
        "takeover transfers future input rights"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn only_input_lease_holder_can_send_input() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");
    authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");

    authority
        .send_input(&opened.session_id, &creator, b"echo creator\n")
        .await
        .expect("creator can input");
    assert!(
        authority
            .send_input(&opened.session_id, &second, b"echo stale\n")
            .await
            .is_err(),
        "non-holder input must be rejected"
    );
    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease");
    authority
        .send_input(&opened.session_id, &second, b"echo second\n")
        .await
        .expect("new holder can input");

    assert_eq!(
        spawner.inputs(),
        vec![b"echo creator\n".to_vec(), b"echo second\n".to_vec()],
        "rejected stale input must not reach the PTY"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn pty_geometry_is_session_level_and_only_lease_holder_can_adapt_it() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");
    assert_eq!(
        opened.pty_geometry,
        TerminalPtyGeometry { cols: 80, rows: 24 },
        "viewer viewport size must not define initial PTY geometry"
    );
    authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");

    let stale = authority
        .adapt_pty_geometry(
            &opened.session_id,
            &second,
            TerminalPtyGeometry {
                cols: 120,
                rows: 40,
            },
        )
        .await;
    assert!(
        stale.is_err(),
        "non-holder viewport changes must not resize the session PTY"
    );
    assert!(
        spawner.geometry_changes().is_empty(),
        "rejected geometry changes must not reach the PTY"
    );
    let cursor = authority
        .attach(&opened.session_id, creator.clone())
        .await
        .expect("capture current stream cursor")
        .live_cursor;
    let mut activity_rx = authority
        .subscribe_activity(&opened.session_id, cursor)
        .await
        .expect("subscribe terminal activity");

    let adapted = authority
        .adapt_pty_geometry(
            &opened.session_id,
            &creator,
            TerminalPtyGeometry {
                cols: 100,
                rows: 30,
            },
        )
        .await
        .expect("lease holder can adapt PTY geometry");
    assert_eq!(
        adapted.pty_geometry,
        TerminalPtyGeometry {
            cols: 100,
            rows: 30
        }
    );
    assert_eq!(
        spawner.geometry_changes(),
        vec![TerminalPtyGeometry {
            cols: 100,
            rows: 30
        }]
    );
    assert_eq!(
        activity_rx.recv().await,
        Some(TerminalActivityFrame::PtyGeometryChanged {
            activity_sequence: cursor.next_activity_sequence,
            geometry: TerminalPtyGeometry {
                cols: 100,
                rows: 30
            }
        })
    );

    let unchanged = authority
        .adapt_pty_geometry(
            &opened.session_id,
            &creator,
            TerminalPtyGeometry {
                cols: 100,
                rows: 30,
            },
        )
        .await
        .expect("matching geometry is a no-op");
    assert_eq!(
        unchanged.pty_geometry,
        TerminalPtyGeometry {
            cols: 100,
            rows: 30,
        }
    );
    assert_eq!(
        spawner.geometry_changes(),
        vec![TerminalPtyGeometry {
            cols: 100,
            rows: 30,
        }]
    );
    assert!(activity_rx.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_authority_publishes_live_activity_to_active_stream_broadcast_hub() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    )
    .with_active_stream_broadcast_hub(Arc::clone(&hub));
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    let mut subscription = hub
        .subscribe_terminal(
            &opened.session_id,
            &terminal_replay_cursors(&opened.session_id, 0),
        )
        .await;
    let rx = subscription
        .rx
        .as_mut()
        .expect("terminal activity subscription");

    spawner.emit(b"live-output-1\nlive-output-2\n");
    let output = next_terminal_activity_payload(rx).await;
    assert_eq!(output["type"], "output");
    let output_data = BASE64_STANDARD
        .decode(output["data_base64"].as_str().expect("output base64"))
        .expect("decode output");
    assert_eq!(output_data, b"live-output-1\nlive-output-2\n");
    assert_eq!(output["activity_sequence"], 0);
    assert_eq!(output["start_offset"], 0);
    assert_eq!(output["end_offset"], output_data.len() as u64);

    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease");
    let lease = next_terminal_activity_payload(rx).await;
    assert_eq!(lease["type"], "input_lease_changed");
    assert_eq!(lease["holder"]["browser_id"], "browser-b");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn remote_activity_cursor_replays_the_same_next_frame_as_local_subscription() {
    let hub = ActiveStreamBroadcastHub::new();
    let session_id = "terminal-session-cursor";
    let stream_id = "terminal-activity:terminal-session-cursor";
    for activity_sequence in 0..3 {
        let frame = TerminalActivityFrame::Output {
            activity_sequence,
            start_offset: activity_sequence,
            end_offset: activity_sequence + 1,
            data: vec![activity_sequence as u8],
        };
        hub.publish_terminal_event_with_replay_max_bytes(
            stream_id,
            session_id,
            terminal_activity_frame_json(&frame).to_string(),
            1024,
        )
        .await;
    }
    let cursor = TerminalStreamCursor {
        next_activity_sequence: 2,
        next_output_offset: 2,
    };
    let subscription = hub
        .subscribe_terminal(
            session_id,
            &terminal_replay_cursors(session_id, terminal_activity_broadcast_after_seq(cursor)),
        )
        .await;
    let replay = subscription
        .replay
        .into_iter()
        .filter_map(terminal_activity_from_remote_stream_frame)
        .collect::<Vec<_>>();

    assert_eq!(
        replay,
        vec![TerminalActivityFrame::Output {
            activity_sequence: 2,
            start_offset: 2,
            end_offset: 3,
            data: vec![2],
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_broadcast_active_replay_is_not_byte_capped() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    )
    .with_active_stream_broadcast_hub(Arc::clone(&hub));
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");

    let long_line = format!("{}\n", "x".repeat(1024));
    spawner.emit(long_line.as_bytes());

    let replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let subscription = hub
                .subscribe_terminal(
                    &opened.session_id,
                    &terminal_replay_cursors(&opened.session_id, 0),
                )
                .await;
            let saw_long_line = subscription
                .replay
                .iter()
                .any(|frame| active_broadcast_event_data(frame).as_deref() == Some(&long_line));
            if saw_long_line {
                break subscription.replay;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal broadcast replay should retain the complete output frame");
    assert!(
        !replay
            .iter()
            .any(|frame| matches!(frame, ActiveStreamBroadcastFrame::Gap { .. })),
        "terminal activity replay must not introduce a byte-cap gap"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_broadcast_replays_multiline_pty_output_as_one_event() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    )
    .with_active_stream_broadcast_hub(Arc::clone(&hub));
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 2,
        })
        .await
        .expect("open terminal");

    spawner.emit(b"line-0\nline-1\nline-2\nline-3\n");

    let replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let stale = hub
                .subscribe_terminal(
                    &opened.session_id,
                    &terminal_replay_cursors(&opened.session_id, 0),
                )
                .await;
            let after_seq = match stale.replay.first() {
                Some(ActiveStreamBroadcastFrame::Gap {
                    first_available_seq,
                    ..
                }) => first_available_seq.saturating_sub(1),
                _ => 0,
            };
            let tail = hub
                .subscribe_terminal(
                    &opened.session_id,
                    &terminal_replay_cursors(&opened.session_id, after_seq),
                )
                .await;
            let saw_latest = tail.replay.iter().any(|frame| {
                active_broadcast_event_data(frame).as_deref()
                    == Some("line-0\nline-1\nline-2\nline-3\n")
            });
            if saw_latest {
                break tail.replay;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal broadcast replay should retain the configured line tail");

    assert_eq!(
        replay
            .iter()
            .filter_map(active_broadcast_event_data)
            .collect::<Vec<_>>(),
        vec!["line-0\nline-1\nline-2\nline-3\n"]
    );

    authority
        .set_scrollback_lines(&opened.session_id, 1)
        .await
        .expect("reduce terminal scrollback");
    let stale = hub
        .subscribe_terminal(
            &opened.session_id,
            &terminal_replay_cursors(&opened.session_id, 0),
        )
        .await;
    let after_seq = match stale.replay.first() {
        Some(ActiveStreamBroadcastFrame::Gap {
            first_available_seq,
            ..
        }) => first_available_seq.saturating_sub(1),
        _ => 0,
    };
    let reduced = hub
        .subscribe_terminal(
            &opened.session_id,
            &terminal_replay_cursors(&opened.session_id, after_seq),
        )
        .await;
    assert_eq!(
        reduced
            .replay
            .iter()
            .filter_map(active_broadcast_event_data)
            .collect::<Vec<_>>(),
        vec!["line-0\nline-1\nline-2\nline-3\n"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_process_exit_emits_activity_and_removes_live_session() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = LiveOutputSpawner::new();
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    )
    .with_active_stream_broadcast_hub(Arc::clone(&hub));
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: TerminalViewerIdentity::local("browser-a"),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    let cursor = authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-a"),
        )
        .await
        .expect("capture exit cursor")
        .live_cursor;
    let mut local_activity = authority
        .subscribe_activity(&opened.session_id, cursor)
        .await
        .expect("subscribe local activity");
    let mut remote_subscription = hub
        .subscribe_terminal(
            &opened.session_id,
            &terminal_replay_cursors(&opened.session_id, 0),
        )
        .await;
    let remote_rx = remote_subscription
        .rx
        .as_mut()
        .expect("remote terminal activity subscription");

    spawner.close();

    let local_frame = tokio::time::timeout(Duration::from_secs(1), local_activity.recv())
        .await
        .expect("local exit activity")
        .expect("local activity channel");
    assert_eq!(
        local_frame,
        TerminalActivityFrame::Exited {
            activity_sequence: cursor.next_activity_sequence
        }
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if authority.list().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exited terminal should be removed from live list");

    let remote_exit = next_terminal_activity_payload(remote_rx).await;
    assert_eq!(remote_exit["type"], "exited");
    assert!(matches!(
        authority
            .attach(
                &opened.session_id,
                TerminalViewerIdentity::local("browser-b"),
            )
            .await,
        Err(TerminalError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn explicit_kill_requires_the_input_lease_and_notifies_viewers_once() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open terminal");
    let attached = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");

    assert!(matches!(
        authority.kill(&opened.session_id, &second).await,
        Err(TerminalError::InputLeaseRequired)
    ));
    assert!(!spawner.was_killed());
    assert_eq!(authority.list().await.len(), 1);

    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease");
    let current = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("capture current kill cursor");
    let mut activity = authority
        .subscribe_activity(&opened.session_id, current.live_cursor)
        .await
        .expect("subscribe to kill activity");

    authority
        .kill(&opened.session_id, &second)
        .await
        .expect("kill with input lease");

    assert_eq!(
        activity.recv().await,
        Some(TerminalActivityFrame::Exited {
            activity_sequence: current.live_cursor.next_activity_sequence,
        })
    );
    assert_eq!(activity.recv().await, None);
    assert!(spawner.was_killed());
    assert!(authority.list().await.is_empty());
    assert!(matches!(
        authority.attach(&opened.session_id, creator).await,
        Err(TerminalError::NotFound(_))
    ));
    assert!(matches!(
        authority.kill(&opened.session_id, &second).await,
        Err(TerminalError::NotFound(_))
    ));
    assert_eq!(
        attached.session.session_id, opened.session_id,
        "the original attachment remains the same authority session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn input_lease_takeover_notifies_attached_viewers() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");

    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");
    let attached = authority
        .attach(&opened.session_id, second.clone())
        .await
        .expect("attach second viewer");
    let mut creator_activity = authority
        .subscribe_activity(&opened.session_id, attached.live_cursor)
        .await
        .expect("subscribe creator activity");

    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take input lease");

    let frame = tokio::time::timeout(Duration::from_secs(1), creator_activity.recv())
        .await
        .expect("activity frame should arrive")
        .expect("activity channel stays open");
    assert_eq!(
        frame,
        TerminalActivityFrame::InputLeaseChanged {
            activity_sequence: attached.live_cursor.next_activity_sequence,
            holder: Some(second)
        },
        "previous holder needs an activity update so it can lock input"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn reattach_before_detached_ttl_keeps_session_alive() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_millis(50),
        },
        Arc::new(spawner.clone()),
    );
    let viewer = TerminalViewerIdentity::local("browser-a");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: viewer.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    authority
        .detach(&opened.session_id, &viewer)
        .await
        .expect("detach only viewer");
    authority
        .attach(
            &opened.session_id,
            TerminalViewerIdentity::local("browser-b"),
        )
        .await
        .expect("reattach before TTL");

    tokio::time::sleep(Duration::from_millis(90)).await;

    assert!(
        !spawner.was_killed(),
        "reattach before TTL must prevent detached cleanup"
    );
    let sessions = authority.list().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, opened.session_id);
    assert_eq!(sessions[0].viewer_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn input_lease_takeover_cancels_pending_detached_cleanup() {
    let home = tempfile::tempdir().expect("home tempdir");
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_millis(50),
        },
        Arc::new(spawner.clone()),
    );
    let creator = TerminalViewerIdentity::local("browser-a");
    let second = TerminalViewerIdentity::local("browser-b");
    let opened = authority
        .open(OpenTerminalSessionRequest {
            viewer: creator.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .expect("open local terminal");

    authority
        .detach(&opened.session_id, &creator)
        .await
        .expect("detach only viewer");
    authority
        .take_input_lease(&opened.session_id, second.clone())
        .await
        .expect("take lease during detached TTL");

    tokio::time::sleep(Duration::from_millis(90)).await;

    assert!(!spawner.was_killed());
    let sessions = authority.list().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].viewer_count, 1);
    assert_eq!(sessions[0].input_lease_holder, Some(second));
}

#[test]
#[serial]
fn native_process_spawner_uses_runtime_layout_shell_and_requested_cwd() {
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let shell_path = if cfg!(unix) { "/bin/sh" } else { "cmd.exe" };
    let _shell = EnvGuard::set("SHELL", shell_path);
    let request =
        NativeTerminalProcessSpawner::default_spawn_request(cwd.path().to_path_buf(), None)
            .expect("build terminal spawn request");

    assert_eq!(request.cwd, cwd.path());
    if cfg!(unix) {
        assert_eq!(request.shell.program, shell_path);
    }
    assert!(
        request.shell.args.is_empty(),
        "interactive terminal shell must not be launched with command-mode args"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn mobile_terminal_defaults_to_debian_and_rejects_host_profiles() {
    let temp = tempfile::tempdir().expect("mobile tempdir");
    let data = temp.path().join("files");
    let native = temp.path().join("native");
    let rootfs = data.join("runtime/debian");
    for directory in [
        &native,
        &rootfs.join("bin"),
        &rootfs.join("usr/bin"),
        &data.join("app-resources"),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    for file in [
        native.join("libproot.so"),
        rootfs.join("usr/bin/env"),
        rootfs.join("bin/bash"),
    ] {
        std::fs::write(file, b"").unwrap();
    }
    let workspace = data.join("home/.rua/workspaces/default");
    std::fs::create_dir_all(&workspace).unwrap();
    let _profile = EnvGuard::set("RUA_HOST_PROFILE", "mobile-apk");
    let _native = EnvGuard::set("RUA_ANDROID_NATIVE_DIR", &native);
    let _data = EnvGuard::set("RUA_ANDROID_DATA_DIR", &data);
    let _home = EnvGuard::set("HOME", data.join("home"));
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: workspace.clone(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(NativeTerminalProcessSpawner),
    );
    let profiles = authority.list_profiles();
    assert_eq!(profiles.default_profile_id, "proot");
    assert_eq!(profiles.profiles.len(), 1);
    assert_eq!(profiles.profiles[0].label, "Debian");
    assert!(NativeTerminalProcessSpawner::default_spawn_request(
        workspace.clone(),
        Some("unknown-profile")
    )
    .is_err());
    let request =
        NativeTerminalProcessSpawner::default_spawn_request(workspace.clone(), None).unwrap();
    assert_eq!(
        request.shell.program,
        native.join("librua-runtime-gate.so").to_string_lossy()
    );
    assert!(request
        .shell
        .args
        .contains(&format!("--cwd={}", workspace.display())));
    assert_eq!(
        request.shell.args.last().map(String::as_str),
        Some("/bin/bash")
    );
    std::fs::remove_file(rootfs.join("usr/bin/env")).unwrap();
    assert!(NativeTerminalProcessSpawner::default_spawn_request(workspace, None).is_err());
    authority.shutdown_all().await;
}

#[tokio::test]
#[serial]
async fn terminal_bridge_resolves_after_the_environment_becomes_available() {
    let home = tempfile::tempdir().unwrap();
    let unavailable_path = EnvGuard::set("PATH", home.path());
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    drop(unavailable_path);
    let viewer = TerminalViewerIdentity::local("installed-after-start");
    let session = authority
        .open(OpenTerminalSessionRequest {
            viewer: viewer.clone(),
            cwd: None,
            terminal_profile_id: None,
            scrollback_lines: 100,
        })
        .await
        .unwrap();
    spawner.emit(b"installed-runtime-ready");
    tokio::time::sleep(Duration::from_millis(30)).await;
    let attached = authority.attach(&session.session_id, viewer).await.unwrap();
    assert!(attached.snapshot.data.contains("installed-runtime-ready"));
    authority.shutdown_all().await;
}

#[tokio::test]
#[serial]
async fn owner_shutdown_closes_sessions_and_rejects_new_sessions() {
    let home = tempfile::tempdir().unwrap();
    let spawner = FakeProcessSpawner::new();
    let authority = TerminalSessionAuthority::new(
        TerminalAuthorityConfig {
            default_home: home.path().to_path_buf(),
            detached_ttl: Duration::from_secs(600),
        },
        Arc::new(spawner.clone()),
    );
    let request = || OpenTerminalSessionRequest {
        viewer: TerminalViewerIdentity::local("owner-shutdown"),
        cwd: None,
        terminal_profile_id: None,
        scrollback_lines: 100,
    };
    authority.open(request()).await.unwrap();
    authority.open(request()).await.unwrap();
    authority.shutdown_all().await;
    assert!(authority.list().await.is_empty());
    assert!(spawner.was_killed());
    assert!(authority.open(request()).await.is_err());
    authority.shutdown_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn native_process_spawner_runs_shell_and_captures_output() {
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let spawner = NativeTerminalProcessSpawner;
    let mut handle = spawner
        .spawn(rua_server::services::terminal::TerminalSpawnRequest {
            cwd: cwd.path().to_path_buf(),
            terminal_profile_id: None,
        })
        .expect("spawn native shell");

    handle
        .control
        .write_input(b"printf rua-terminal-ready\\n\nexit\n")
        .expect("write shell input");

    let mut captured = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            chunk = handle.output.recv() => {
                let Some(chunk) = chunk else { break };
                captured.extend(chunk);
                if String::from_utf8_lossy(&captured).contains("rua-terminal-ready") {
                    break;
                }
            }
        }
    }

    handle.control.kill();
    assert!(
        String::from_utf8_lossy(&captured).contains("rua-terminal-ready"),
        "native shell output should contain marker; got {:?}",
        String::from_utf8_lossy(&captured)
    );
}

#[test]
fn terminal_placement_routes_explicit_local_without_sync_identity() {
    let route = route_terminal_execution_device(TerminalExecutionDevice::Local, None);
    assert_eq!(route, TerminalPlacementRoute::Local);
}

#[test]
fn terminal_placement_requires_source_identity_for_remote_device() {
    let route = route_terminal_execution_device(
        TerminalExecutionDevice::Remote {
            device_id: "target-device".to_string(),
        },
        None,
    );
    assert_eq!(
        route,
        TerminalPlacementRoute::MissingSourceDevice {
            target_device_id: "target-device".to_string()
        }
    );
}

#[test]
fn terminal_placement_never_falls_back_to_local_for_unreachable_remote_shape() {
    let route = route_terminal_execution_device(
        TerminalExecutionDevice::Remote {
            device_id: "target-device".to_string(),
        },
        Some("source-device"),
    );
    assert_eq!(
        route,
        TerminalPlacementRoute::Remote {
            source_device_id: "source-device".to_string(),
            target_device_id: "target-device".to_string()
        }
    );
}

#[test]
fn terminal_placement_routes_selected_self_device_to_local_authority() {
    let route = route_terminal_execution_device(
        TerminalExecutionDevice::Remote {
            device_id: "self-device".to_string(),
        },
        Some("self-device"),
    );
    assert_eq!(route, TerminalPlacementRoute::Local);
}

#[test]
fn remote_stream_error_maps_to_terminal_domain_error_activity() {
    let frame = ActiveStreamBroadcastFrame::Error {
        device_delivery_id: "internal-stream".to_string(),
        owner: Some(ActiveStreamOwner::terminal("terminal-session-1")),
        authority_epoch: None,
        seq: 7,
        detail: "remote terminal stream failed".to_string(),
    };

    let activity =
        terminal_activity_from_remote_stream_frame(frame).expect("terminal activity frame");

    assert_eq!(
        activity,
        TerminalActivityFrame::Error {
            activity_sequence: 7,
            detail: "remote terminal stream failed".to_string()
        }
    );
}

#[test]
fn remote_stream_gap_is_ignored_for_terminal_activity() {
    let frame = ActiveStreamBroadcastFrame::Gap {
        device_delivery_id: "internal-stream".to_string(),
        requested_after_seq: 0,
        first_available_seq: 547,
    };

    assert_eq!(terminal_activity_from_remote_stream_frame(frame), None);
}

#[test]
fn terminal_diagnostics_do_not_record_terminal_content() {
    let sources = [
        include_str!("../../src/services/terminal.rs"),
        include_str!("../../src/services/device_command_bus.rs"),
    ];
    let diagnostic_blocks = sources
        .into_iter()
        .flat_map(|source| {
            source
                .match_indices("tracing::")
                .map(move |(start, _)| &source[start..])
        })
        .map(|block| {
            block
                .split_once(");")
                .map_or(block, |(macro_body, _)| macro_body)
        })
        .filter(|block| block.contains("event = \"terminal_"))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in [
        "data = %",
        "data = ?",
        "snapshot_data",
        "payload =",
        "input =",
        "command =",
        "clipboard",
    ] {
        assert!(
            !diagnostic_blocks.contains(forbidden),
            "terminal diagnostic block contains forbidden content field: {forbidden}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn foreground_terminal_wait_is_bounded_and_suspended_runtime_fails_immediately() {
    use rua_server::services::device_command_bus::{TerminalDeviceAction, TerminalDeviceCommand};
    use rua_server::services::device_network_availability::DeviceNetworkAvailability;
    use rua_server::services::terminal::{
        DeviceSessionTerminalRemoteCommandClient, TerminalRemoteCommandClient,
    };
    let availability = Arc::new(DeviceNetworkAvailability::new());
    let client = DeviceSessionTerminalRemoteCommandClient::new(Arc::new(
        rua_server::services::device_session_access::DeviceNetworkAccess::new(
            None,
            Arc::clone(&availability),
        ),
    ));
    let command = || TerminalDeviceCommand {
        command_id: "foreground-timeout".into(),
        source_device_id: "source".into(),
        target_device_id: "target".into(),
        action: TerminalDeviceAction::ListProfiles,
    };
    let started = tokio::time::Instant::now();
    assert!(client.deliver(command()).await.is_err());
    assert_eq!(started.elapsed(), Duration::ZERO);
    availability.mark_starting();
    let error = client.deliver(command()).await.unwrap_err();
    assert!(error.to_string().contains("readiness timed out"));
    assert_eq!(started.elapsed(), Duration::from_secs(10));
}
