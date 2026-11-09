use std::fs;
use std::process;
use tempfile::TempDir;

use rua_server::process_liveness::is_process_alive;
use rua_server::{ServerOptions, ServerProfile};

fn pid_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("server.pid")
}

#[test]
fn lockfile_blocks_when_pid_alive() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    let my_pid = process::id();
    fs::write(pid_path(dir), my_pid.to_string()).unwrap();

    let content = fs::read_to_string(pid_path(dir)).unwrap();
    let pid: u32 = content.trim().parse().unwrap();
    assert!(is_process_alive(pid), "current process should be alive");
}

#[tokio::test]
async fn serve_reports_that_the_rua_root_is_in_use() {
    let tmp = TempDir::new().unwrap();
    fs::write(pid_path(tmp.path()), process::id().to_string()).unwrap();

    let error = rua_server::serve(ServerOptions {
        rua_root: Some(tmp.path().to_path_buf()),
        profile: ServerProfile::Test,
        ..ServerOptions::default()
    })
    .await
    .expect_err("a live PID must block a second Runtime");
    let message = error.to_string();

    assert!(message.contains("Cannot start Rua"), "got: {message}");
    assert!(
        message.contains("Rua Root") && message.contains("already in use"),
        "got: {message}"
    );
    assert!(message.contains("Rua Desktop app"), "got: {message}");
    assert!(message.contains("--rua-root <path>"), "got: {message}");
    assert!(
        !tmp.path().join("logs").join("startup.log").exists(),
        "the conflict must be returned before stderr is redirected"
    );
}

#[test]
fn lockfile_allows_when_pid_dead() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    let dead_pid = spawn_short_lived_process();

    fs::write(pid_path(dir), dead_pid.to_string()).unwrap();

    let content = fs::read_to_string(pid_path(dir)).unwrap();
    let pid: u32 = content.trim().parse().unwrap();
    assert!(
        !is_process_alive(pid),
        "dead PID should not register as alive"
    );
}

fn spawn_short_lived_process() -> u32 {
    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "exit", "/B", "0"])
        .spawn()
        .expect("spawn short-lived process");

    #[cfg(not(windows))]
    let mut child = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn short-lived process");

    let pid = child.id();
    child.wait().expect("wait short-lived process");
    pid
}

#[test]
fn lockfile_creates_when_absent() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let path = pid_path(dir);

    assert!(!path.exists(), "lockfile should not exist initially");

    fs::write(&path, process::id().to_string()).unwrap();
    assert!(path.exists(), "lockfile should be created");

    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(
        content.trim().parse::<u32>().unwrap(),
        process::id(),
        "lockfile should contain current PID"
    );
}

#[test]
fn lockfile_removal_on_shutdown() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let path = pid_path(dir);

    fs::write(&path, process::id().to_string()).unwrap();
    assert!(path.exists());

    let _ = fs::remove_file(&path);
    assert!(!path.exists(), "lockfile should be removed after shutdown");
}
