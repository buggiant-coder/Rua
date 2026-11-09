#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use rua_local_model::{DownloadStatus, DownloadSubprocess, IpcMessage};
use tempfile::TempDir;
use tokio::io::BufReader;

fn write_executable_script(tmp: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = tmp.path().join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[tokio::test]
async fn spawn_script_and_drain_ipc_stdout() {
    let tmp = TempDir::new().unwrap();
    let script = write_executable_script(
        &tmp,
        "fake-rua-download",
        r#"#!/bin/sh
echo '{"type":"status","id":"test","status":"started"}'
echo '{"type":"progress","id":"test","bytes_downloaded":100,"bytes_total":200}'
echo '{"type":"status","id":"test","status":"completed"}'
"#,
    );
    let mut subprocess = DownloadSubprocess::spawn(script.to_str().unwrap(), "model-id").unwrap();
    let stdout = subprocess.take_stdout().expect("stdout should be piped");
    let mut reader = BufReader::new(stdout);

    let mut messages = Vec::new();
    while let Some(msg) = DownloadSubprocess::read_message(&mut reader)
        .await
        .expect("read")
    {
        messages.push(msg);
    }

    assert_eq!(messages.len(), 3);
    assert!(matches!(&messages[0], IpcMessage::Status(s) if s.status == DownloadStatus::Started));
    assert!(matches!(&messages[1], IpcMessage::Progress(p) if p.bytes_downloaded == 100));
    assert!(matches!(&messages[2], IpcMessage::Status(s) if s.status == DownloadStatus::Completed));
}

#[tokio::test]
async fn kill_removes_partial_file() {
    let tmp = TempDir::new().unwrap();
    let script = write_executable_script(
        &tmp,
        "fake-rua-download",
        r#"#!/bin/sh
sleep 60
"#,
    );
    let partial_path = tmp.path().join("partial.gguf");
    std::fs::write(&partial_path, b"partial").unwrap();
    assert!(partial_path.exists());

    let mut subprocess = DownloadSubprocess::spawn(script.to_str().unwrap(), "model-id").unwrap();
    subprocess.set_partial_path(partial_path.clone());

    subprocess.kill().await.expect("kill");

    assert!(!partial_path.exists());
}
