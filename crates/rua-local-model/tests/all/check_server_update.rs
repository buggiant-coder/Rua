use std::time::Duration;

use rua_local_model::manager::LocalModelManager;
use tempfile::TempDir;
use tokio::net::TcpListener;

async fn start_mock_api(tag: &str) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(r#"{{"tag_name":"{tag}"}}"#);

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let body = body.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });

    (port, handle)
}

fn manager_with_api(dir: &std::path::Path, port: u16) -> LocalModelManager {
    LocalModelManager::new_for_test(dir)
        .with_server_api_url(format!("http://127.0.0.1:{port}/api/latest"))
        .with_health_timeout(Duration::from_millis(100))
        .with_skip_health_check(true)
}

async fn write_version_file(dir: &std::path::Path, tag: &str) {
    let server_dir = dir.join("server");
    tokio::fs::create_dir_all(&server_dir).await.unwrap();
    tokio::fs::write(server_dir.join("version.txt"), tag)
        .await
        .unwrap();
}

#[tokio::test]
async fn update_available_when_latest_is_newer() {
    let tmp = TempDir::new().unwrap();
    let (port, _handle) = start_mock_api("b4000").await;

    write_version_file(tmp.path(), "b3000").await;
    let mgr = manager_with_api(tmp.path(), port);

    let info = mgr.check_server_update().await;
    assert_eq!(info.has_update, Some(true));
    assert_eq!(info.current_version.as_deref(), Some("b3000"));
    assert_eq!(info.latest_version.as_deref(), Some("b4000"));
}

#[tokio::test]
async fn no_update_when_versions_equal() {
    let tmp = TempDir::new().unwrap();
    let (port, _handle) = start_mock_api("b3000").await;

    write_version_file(tmp.path(), "b3000").await;
    let mgr = manager_with_api(tmp.path(), port);

    let info = mgr.check_server_update().await;
    assert_eq!(info.has_update, Some(false));
    assert_eq!(info.current_version.as_deref(), Some("b3000"));
    assert_eq!(info.latest_version.as_deref(), Some("b3000"));
}

#[tokio::test]
async fn no_update_when_current_is_newer() {
    let tmp = TempDir::new().unwrap();
    let (port, _handle) = start_mock_api("b2000").await;

    write_version_file(tmp.path(), "b3000").await;
    let mgr = manager_with_api(tmp.path(), port);

    let info = mgr.check_server_update().await;
    assert_eq!(info.has_update, Some(false));
    assert_eq!(info.current_version.as_deref(), Some("b3000"));
    assert_eq!(info.latest_version.as_deref(), Some("b2000"));
}

#[tokio::test]
async fn raw_llama_build_numbers_compare_without_fixed_width_assumptions() {
    let tmp = TempDir::new().unwrap();
    let (port, _handle) = start_mock_api("10000").await;

    write_version_file(tmp.path(), "8514").await;
    let mgr = manager_with_api(tmp.path(), port);

    let info = mgr.check_server_update().await;
    assert_eq!(info.has_update, Some(true));
    assert_eq!(info.current_version.as_deref(), Some("8514"));
    assert_eq!(info.latest_version.as_deref(), Some("10000"));
}

#[tokio::test]
async fn network_failure_returns_unknown() {
    let tmp = TempDir::new().unwrap();
    write_version_file(tmp.path(), "b3000").await;

    let mgr = LocalModelManager::new_for_test(tmp.path())
        .with_server_api_url("http://127.0.0.1:1/api/latest")
        .with_health_timeout(Duration::from_millis(100))
        .with_skip_health_check(true);

    let info = mgr.check_server_update().await;
    assert_eq!(info.has_update, None, "network failure should yield None");
    assert_eq!(info.current_version.as_deref(), Some("b3000"));
    assert!(info.latest_version.is_none());
}

#[tokio::test]
async fn no_version_file_returns_unknown_update_status() {
    let tmp = TempDir::new().unwrap();
    let (port, _handle) = start_mock_api("b4000").await;

    let mgr = manager_with_api(tmp.path(), port);

    let info = mgr.check_server_update().await;

    assert_eq!(info.has_update, None);
    assert!(info.current_version.is_none());
    assert_eq!(info.latest_version.as_deref(), Some("b4000"));
}
