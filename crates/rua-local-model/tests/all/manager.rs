use std::time::Duration;

use rua_local_model::{LocalModelManager, StartServerRequest};

fn manager_for_test() -> LocalModelManager {
    LocalModelManager::new_for_test(std::env::temp_dir())
        .with_llama_binary("sleep")
        .with_health_timeout(Duration::from_millis(200))
        .with_skip_health_check(true)
        .with_watchdog_interval(Duration::from_millis(100))
}

#[tokio::test]
async fn start_status_stop_lifecycle() {
    let mgr = manager_for_test();

    let initial = mgr.server_status().await;
    assert!(!initial.running);
    assert!(initial.pid.is_none());

    let started = mgr
        .start_server(StartServerRequest {
            model_id: None,
            port: Some(0),
        })
        .await
        .expect("start should succeed when health check is skipped");
    assert!(started.running);
    assert!(started.pid.is_some());
    assert_eq!(started.port, Some(0));

    let mid = mgr.server_status().await;
    assert!(mid.running);
    assert_eq!(mid.pid, started.pid);

    mgr.stop_server().await.expect("stop should succeed");

    let after = mgr.server_status().await;
    assert!(!after.running);
    assert!(after.pid.is_none());
}

#[tokio::test]
async fn double_start_is_conflict() {
    let mgr = manager_for_test();
    mgr.start_server(StartServerRequest::default())
        .await
        .expect("first start ok");
    let err = mgr
        .start_server(StartServerRequest::default())
        .await
        .expect_err("second start must conflict");
    let msg = format!("{err}");
    assert!(msg.contains("already running"), "got: {msg}");
    mgr.stop_server().await.unwrap();
}

#[tokio::test]
async fn stop_without_start_is_conflict() {
    let mgr = manager_for_test();
    let err = mgr
        .stop_server()
        .await
        .expect_err("stop without start must error");
    assert!(format!("{err}").contains("not running"));
}

#[tokio::test]
async fn scan_indexes_gguf_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("foo.gguf"), b"fake-gguf").unwrap();
    std::fs::write(dir.path().join("bar.bin"), b"fake-bin").unwrap();
    std::fs::write(dir.path().join("note.txt"), b"ignore-me").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    let result = mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    assert_eq!(result.errors.len(), 0);
    assert_eq!(result.found.len(), 2);
    let ids: Vec<&str> = result.found.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.iter().any(|i| i.ends_with("foo")));
    assert!(ids.iter().any(|i| i.ends_with("bar")));

    let listed = mgr.list_models().await;
    assert_eq!(listed.len(), 2);
}

#[tokio::test]
async fn scan_ignores_directories_even_when_their_names_look_like_model_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("partial-download.gguf")).unwrap();
    std::fs::create_dir(dir.path().join("repo-cache.bin")).unwrap();
    std::fs::write(dir.path().join("real-model.gguf"), b"fake-gguf").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    let result = mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    assert_eq!(result.errors.len(), 0);
    assert_eq!(
        result
            .found
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["local-real-model"],
        "download/cache directories must not be advertised as downloaded local models"
    );
    assert_eq!(mgr.list_models().await.len(), 1);
}

#[tokio::test]
async fn scan_ignores_mmproj_projector_files_without_advertising_them_as_models() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("MMPROJ-F16.gguf"), b"projector").unwrap();
    std::fs::write(dir.path().join("mmproj-model-f16.gguf"), b"projector").unwrap();
    std::fs::write(dir.path().join("qwen2vl-model.gguf"), b"real-model").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    let result = mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    assert_eq!(result.errors.len(), 0);
    assert_eq!(
        result
            .found
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["local-qwen2vl-model"],
        "projector GGUF files should not be exposed as directly runnable local models"
    );
    assert_eq!(mgr.list_models().await.len(), 1);
}

#[tokio::test]
async fn scan_disabled_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("foo.gguf"), b"fake").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path()).with_scan_enabled(false);
    let result = mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;
    assert!(result.found.is_empty());
}

#[tokio::test]
async fn scan_records_directory_errors_without_failing_other_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ok.gguf"), b"fake").unwrap();
    let missing = dir.path().join("missing");

    let mgr = LocalModelManager::new_for_test(dir.path());
    let result = mgr
        .scan(&[
            missing.to_string_lossy().to_string(),
            dir.path().to_string_lossy().to_string(),
        ])
        .await;

    assert_eq!(result.found.len(), 1);
    assert_eq!(result.found[0].id, "local-ok");
    assert_eq!(result.errors.len(), 1);
    assert!(
        result.errors[0].contains("missing"),
        "got: {:?}",
        result.errors
    );
}

#[tokio::test]
async fn delete_model_removes_scanned_entry_and_reports_missing_ids() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("remove-me.gguf"), b"fake").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    mgr.delete_model("local-remove-me")
        .await
        .expect("scanned model can be deleted from the manager index");
    assert!(mgr.list_models().await.is_empty());

    let err = mgr
        .delete_model("local-remove-me")
        .await
        .expect_err("deleting a missing model should be a not-found error");
    assert!(format!("{err}").contains("local-remove-me"));
}

#[tokio::test]
async fn delete_model_keeps_external_scanned_files_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let external_model = dir.path().join("external.gguf");
    std::fs::write(&external_model, b"user-owned-model").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path().join("rua-managed"));
    mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    mgr.delete_model("local-external")
        .await
        .expect("scanned external model should be removable from the index");

    assert!(mgr.list_models().await.is_empty());
    assert!(
        external_model.exists(),
        "deleting a scanned external model must not delete user-owned files outside working_dir/models"
    );
}

#[tokio::test]
async fn start_server_with_scanned_model_records_selected_model() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("demo.gguf"), b"fake").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path())
        .with_llama_binary("sleep")
        .with_skip_health_check(true)
        .with_watchdog_interval(Duration::from_secs(60));
    mgr.scan(&[dir.path().to_string_lossy().to_string()]).await;

    let started = mgr
        .start_server(StartServerRequest {
            model_id: Some("local-demo".into()),
            port: Some(0),
        })
        .await
        .expect("scanned model should be startable");

    assert!(started.running);
    assert_eq!(started.model.as_deref(), Some("local-demo"));
    assert_eq!(started.port, Some(0));

    mgr.stop_server().await.expect("server should stop cleanly");
}

#[tokio::test]
async fn start_server_rejects_unavailable_requested_port_before_spawning() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mgr = manager_for_test();

    let err = mgr
        .start_server(StartServerRequest {
            model_id: None,
            port: Some(port),
        })
        .await
        .expect_err("occupied ports should be rejected before llama-server spawn");

    let message = err.to_string();
    assert!(
        message.contains(&port.to_string()),
        "error should name the unavailable port, got: {message}"
    );
    assert!(
        !mgr.server_status().await.running,
        "port validation failure must not leave a running process"
    );
}

#[tokio::test]
async fn download_progress_lifecycle() {
    let mgr = LocalModelManager::new_for_test(std::env::temp_dir());

    let progress = mgr.download_model("test-model".into()).await;
    assert_eq!(progress.model_id, "test-model");
    assert_eq!(progress.status, "downloading");

    let snapshot = mgr.model_download_progress().await.unwrap();
    assert_eq!(snapshot.model_id, "test-model");

    mgr.cancel_model_download().await.unwrap();
    let err = mgr.model_download_progress().await.unwrap_err();
    assert!(format!("{err}").contains("model download"));
}

#[tokio::test]
async fn server_download_stops_running_server_first() {
    let mgr = manager_for_test().with_server_api_url("http://127.0.0.1:9/api/latest");

    let started = mgr
        .start_server(StartServerRequest {
            model_id: None,
            port: Some(0),
        })
        .await
        .expect("start should succeed");
    assert!(started.running);

    let progress = mgr.download_server("server-update".into()).await;
    assert_eq!(progress.model_id, "server-update");
    assert_eq!(progress.status, "downloading");

    let status = mgr.server_status().await;
    assert!(
        !status.running,
        "server download should stop a running server before replacing llama-server"
    );

    let _ = mgr.cancel_server_download().await;
}

#[tokio::test]
async fn watchdog_detects_process_exit() {
    let mgr = manager_for_test();

    let started = mgr
        .start_server(StartServerRequest {
            model_id: None,
            port: Some(0),
        })
        .await
        .expect("start should succeed");
    assert!(started.running);
    let pid = started.pid.expect("should have pid");

    std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .output()
        .expect("kill should succeed");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let status = mgr.server_status().await;
    assert!(!status.running, "server_status should reflect dead process");
}

#[tokio::test]
async fn check_server_health_returns_error_after_exit() {
    let mgr = manager_for_test();

    let err = mgr.check_server_health().await.unwrap_err();
    assert!(format!("{err}").contains("not running"));

    mgr.start_server(StartServerRequest {
        model_id: None,
        port: Some(0),
    })
    .await
    .expect("start should succeed");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let err = mgr.check_server_health().await.unwrap_err();
    assert!(
        format!("{err}").contains("exited unexpectedly"),
        "got: {err}"
    );
}

#[test]
fn cuda_check_returns_default_when_no_nvidia_smi() {
    let info = rua_local_model::detect_cuda();
    if !info.available {
        assert!(info.version.is_none());
        assert!(info.vram_mb.is_none());
    }
}
