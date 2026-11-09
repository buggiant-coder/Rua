#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rua_local_model::process::LlamaProcess;
use tempfile::TempDir;

fn write_executable_script(tmp: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = tmp.path().join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn spawn_test_process() -> (TempDir, LlamaProcess) {
    let tmp = TempDir::new().unwrap();
    let script = write_executable_script(
        &tmp,
        "fake-llama-server",
        r#"#!/bin/sh
trap 'exit 0' TERM INT
while true; do sleep 1; done
"#,
    );
    let proc =
        LlamaProcess::spawn(script.to_str().unwrap(), 0, None, Some("test-model".into())).unwrap();
    (tmp, proc)
}

#[tokio::test]
async fn watchdog_detects_process_exit() {
    let (_tmp, mut proc) = spawn_test_process();
    proc.start_watchdog(Duration::from_millis(100));
    assert!(proc.is_healthy());

    std::process::Command::new("kill")
        .arg("-9")
        .arg(proc.pid.to_string())
        .output()
        .expect("kill should succeed");

    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(!proc.is_healthy());
}

#[tokio::test]
async fn is_healthy_true_while_running() {
    let (_tmp, mut proc) = spawn_test_process();
    proc.start_watchdog(Duration::from_millis(100));
    assert!(proc.is_healthy());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(proc.is_healthy());

    proc.shutdown().await;
}

#[tokio::test]
async fn shutdown_aborts_watchdog_without_panic() {
    let (_tmp, mut proc) = spawn_test_process();
    proc.start_watchdog(Duration::from_millis(100));
    assert!(proc.is_healthy());
    proc.shutdown().await;
}

#[tokio::test]
async fn spawn_forwards_host_port_and_model_arguments_to_llama_server() {
    let tmp = TempDir::new().unwrap();
    let script = write_executable_script(
        &tmp,
        "fake-llama-server",
        r#"#!/bin/sh
printf '%s\n' "$@" > "$(dirname "$0")/args.txt"
trap 'exit 0' TERM INT
while true; do sleep 1; done
"#,
    );
    let model_path = tmp.path().join("demo.gguf");
    std::fs::write(&model_path, b"fake-model").unwrap();

    let proc = LlamaProcess::spawn(
        script.to_str().unwrap(),
        43110,
        Some(model_path.to_str().unwrap()),
        Some("demo-model".into()),
    )
    .unwrap();

    let args_path = tmp.path().join("args.txt");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if std::fs::read_to_string(&args_path).is_ok_and(|content| content.contains("--host")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake llama-server should record arguments");

    let args = std::fs::read_to_string(args_path).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            "--host",
            "127.0.0.1",
            "--port",
            "43110",
            "--model",
            model_path.to_str().unwrap()
        ]
    );
    assert_eq!(proc.port, 43110);
    assert_eq!(proc.model.as_deref(), Some("demo-model"));

    proc.shutdown().await;
}
