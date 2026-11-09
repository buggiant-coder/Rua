use rua_cli::http_client::should_bypass_proxy_for_base_url;

#[test]
fn loopback_base_urls_bypass_proxy_environment() {
    for url in [
        "http://127.1.2.3:8088",
        "http://127.0.0.1:8765",
        "http://localhost:8765",
        "http://localhost.:8765",
        "http://[::1]:8765",
    ] {
        assert!(
            should_bypass_proxy_for_base_url(url),
            "{url} should not inherit HTTP proxy env for local CLI traffic"
        );
    }
}

#[test]
fn remote_base_urls_keep_proxy_environment() {
    for url in [
        "http://192.168.1.10:8088",
        "http://example.test:8765",
        "https://rua.internal.example",
    ] {
        assert!(
            !should_bypass_proxy_for_base_url(url),
            "{url} should preserve user proxy env for explicit remote targets"
        );
    }
}

#[test]
fn malformed_or_hostless_base_urls_keep_proxy_environment() {
    for url in ["not a url", "http:///missing-host", "unix:/tmp/rua.sock"] {
        assert!(
            !should_bypass_proxy_for_base_url(url),
            "{url} should fail open to reqwest's normal proxy policy"
        );
    }
}
