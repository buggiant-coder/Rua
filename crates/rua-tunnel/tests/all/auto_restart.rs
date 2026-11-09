use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use rua_tunnel::{BinaryManager, CloudflareTunnelDriver, TunnelEvent};
use tokio::sync::{Mutex, MutexGuard};

static TUNNEL_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

async fn tunnel_test_guard() -> MutexGuard<'static, ()> {
    TUNNEL_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await
}

#[cfg(unix)]
fn write_fixture_script(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    script
}

#[cfg(unix)]
#[tokio::test]
async fn restart_after_crash_updates_url() {
    let _guard = tunnel_test_guard().await;
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");

    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE_DIR='{state}'\n\
             if mkdir \"$STATE_DIR\" 2>/dev/null; then\n\
                 echo 'https://first-run.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'https://restarted.trycloudflare.com' >&2\n\
                 sleep 60\n\
             fi\n",
            state = state_dir.display(),
        ),
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(150),
    ]);
    let mut events = driver.subscribe_events();

    let info = driver.start_tunnel(&script, 8080).await.unwrap();
    assert_eq!(info.public_url, "https://first-run.trycloudflare.com");

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for Started event")
        .expect("event channel closed");
    assert!(
        matches!(event, TunnelEvent::Started { .. }),
        "expected Started, got {event:?}"
    );

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout waiting for restart event")
        .expect("event channel closed");

    match event {
        TunnelEvent::Restarted { url } => {
            assert_eq!(url, "https://restarted.trycloudflare.com");
        }
        other => panic!("expected Restarted, got {other:?}"),
    }

    let info = driver
        .get_info()
        .await
        .expect("info should be set after restart");
    assert_eq!(info.public_url, "https://restarted.trycloudflare.com");
    assert_eq!(info.public_wss_url, "wss://restarted.trycloudflare.com");

    driver.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn all_retries_exhausted_sends_alert() {
    let _guard = tunnel_test_guard().await;
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");

    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE_DIR='{state}'\n\
             if mkdir \"$STATE_DIR\" 2>/dev/null; then\n\
                 echo 'https://initial.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'no url here' >&2\n\
                 exit 1\n\
             fi\n",
            state = state_dir.display(),
        ),
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![
        Duration::from_millis(50),
        Duration::from_millis(50),
        Duration::from_millis(50),
    ]);
    let mut events = driver.subscribe_events();

    let info = driver.start_tunnel(&script, 9090).await.unwrap();
    assert_eq!(info.public_url, "https://initial.trycloudflare.com");

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for Started event")
        .expect("event channel closed");
    assert!(
        matches!(event, TunnelEvent::Started { .. }),
        "expected Started, got {event:?}"
    );

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout waiting for alert event")
        .expect("event channel closed");

    assert!(
        matches!(event, TunnelEvent::AllRetriesFailed),
        "expected AllRetriesFailed, got {event:?}",
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(driver.get_info().await.is_none());
    assert!(!driver.is_running().await);
}

#[cfg(unix)]
#[tokio::test]
async fn user_stop_does_not_trigger_restart() {
    let _guard = tunnel_test_guard().await;
    let dir = tempfile::tempdir().unwrap();
    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        "#!/bin/sh\n\
         echo 'https://stop-no-restart.trycloudflare.com' >&2\n\
         sleep 300\n",
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver =
        CloudflareTunnelDriver::new(mgr).with_retry_backoffs(vec![Duration::from_millis(50)]);
    let mut events = driver.subscribe_events();

    driver.start_tunnel(&script, 7070).await.unwrap();
    assert!(driver.is_running().await);

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for Started event")
        .expect("event channel closed");
    assert!(
        matches!(event, TunnelEvent::Started { .. }),
        "expected Started, got {event:?}"
    );

    driver.stop().await;
    assert!(!driver.is_running().await);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        events.try_recv().is_err(),
        "no events should fire on user-initiated stop",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn started_event_emitted_on_first_connect() {
    let _guard = tunnel_test_guard().await;
    let dir = tempfile::tempdir().unwrap();
    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        "#!/bin/sh\n\
         echo 'https://started-test.trycloudflare.com' >&2\n\
         sleep 60\n",
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr);
    let mut events = driver.subscribe_events();

    let info = driver.start_tunnel(&script, 8080).await.unwrap();
    assert_eq!(info.public_url, "https://started-test.trycloudflare.com");

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for Started event")
        .expect("event channel closed");

    match event {
        TunnelEvent::Started { url } => {
            assert_eq!(url, "https://started-test.trycloudflare.com");
        }
        other => panic!("expected Started, got {other:?}"),
    }

    driver.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn crash_then_restart_emits_started_then_restarted() {
    let _guard = tunnel_test_guard().await;
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");

    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        &format!(
            "#!/bin/sh\n\
             STATE_DIR='{state}'\n\
             if mkdir \"$STATE_DIR\" 2>/dev/null; then\n\
                 echo 'https://first.trycloudflare.com' >&2\n\
                 exit 1\n\
             else\n\
                 echo 'https://second.trycloudflare.com' >&2\n\
                 sleep 60\n\
             fi\n",
            state = state_dir.display(),
        ),
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr)
        .with_retry_backoffs(vec![Duration::from_millis(50), Duration::from_millis(100)]);
    let mut events = driver.subscribe_events();

    let info = driver.start_tunnel(&script, 8080).await.unwrap();
    assert_eq!(info.public_url, "https://first.trycloudflare.com");

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for Started event")
        .expect("event channel closed");
    assert!(
        matches!(event, TunnelEvent::Started { .. }),
        "expected Started, got {event:?}"
    );

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout waiting for Restarted event")
        .expect("event channel closed");
    match event {
        TunnelEvent::Restarted { url } => {
            assert_eq!(url, "https://second.trycloudflare.com");
        }
        other => panic!("expected Restarted, got {other:?}"),
    }

    driver.stop().await;
}
