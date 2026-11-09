use std::path::Path;

use rua_tunnel::{
    parse_tunnel_url, BinaryManager, CloudflareTunnelDriver, DriverError, TunnelInfo, TunnelStatus,
};

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

#[test]
fn tunnel_info_builds_wss_url_from_public_https_url() {
    let info = TunnelInfo::new("https://abc-123.trycloudflare.com".to_string(), 8080);

    assert_eq!(info.public_url, "https://abc-123.trycloudflare.com");
    assert_eq!(info.public_wss_url, "wss://abc-123.trycloudflare.com");
    assert_eq!(info.local_port, 8080);
    assert_eq!(info.status, TunnelStatus::Running);
}

#[test]
fn parser_accepts_cloudflared_trycloudflare_https_url_lines() {
    let cases = [
        (
            "2024-01-01T00:00:00Z INF |  https://foo-bar-baz.trycloudflare.com  |",
            "https://foo-bar-baz.trycloudflare.com",
        ),
        (
            "https://abc123.trycloudflare.com",
            "https://abc123.trycloudflare.com",
        ),
    ];

    for (line, expected) in cases {
        assert_eq!(parse_tunnel_url(line), Some(expected.to_string()));
    }
}

#[test]
fn parser_rejects_non_trycloudflare_https_tunnel_lines() {
    for line in [
        "http://abc.trycloudflare.com",
        "https://example.com",
        "https://.trycloudflare.com",
        "https://bad_sub.trycloudflare.com",
        "no url here",
    ] {
        assert_eq!(
            parse_tunnel_url(line),
            None,
            "line should not parse: {line}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn driver_start_parses_url_from_cloudflared_stderr_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        "#!/bin/sh\n\
         echo '2024-01-01T00:00:00Z INF |  https://test-abc123.trycloudflare.com  |' >&2\n\
         sleep 60\n",
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr);

    let info = driver.start_tunnel(&script, 8080).await.unwrap();
    assert_eq!(info.public_url, "https://test-abc123.trycloudflare.com");
    assert_eq!(info.public_wss_url, "wss://test-abc123.trycloudflare.com");
    assert_eq!(info.local_port, 8080);
    assert_eq!(info.status, TunnelStatus::Running);

    assert!(driver.is_running().await);
    assert!(driver.get_info().await.is_some());

    driver.stop().await;
    assert!(!driver.is_running().await);
    assert!(driver.get_info().await.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn driver_start_returns_timeout_when_cloudflared_exits_without_url() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        "#!/bin/sh\n\
         echo 'no url here' >&2\n\
         exit 0\n",
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr);

    let err = driver
        .start_tunnel(&script, 8080)
        .await
        .expect_err("cloudflared output without a public URL should fail");

    assert!(
        matches!(err, DriverError::UrlTimeout(30)),
        "unexpected driver error: {err}",
    );
    assert!(!driver.is_running().await);
    assert!(driver.get_info().await.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn driver_stop_clears_running_info() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fixture_script(
        dir.path(),
        "cloudflared",
        "#!/bin/sh\n\
         echo 'https://stop-test.trycloudflare.com' >&2\n\
         sleep 300\n",
    );

    let mgr = BinaryManager::new(dir.path().to_path_buf());
    let driver = CloudflareTunnelDriver::new(mgr);

    driver.start_tunnel(&script, 7070).await.unwrap();
    assert!(driver.is_running().await);

    driver.stop().await;
    assert!(!driver.is_running().await);
    assert!(driver.get_info().await.is_none());
}
