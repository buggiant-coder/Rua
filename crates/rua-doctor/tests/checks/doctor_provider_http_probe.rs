use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rua_config::dirs::Paths;
use rua_doctor::provider_http::clear_cache;
use rua_doctor::registry::{run_all_checks_filtered, CheckContext, CheckStatus, RegisteredCheck};

fn provider_http_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn spawn_mock_server(status: u16) -> (String, Arc<AtomicUsize>) {
    spawn_counting_http_server(status)
}

fn spawn_counting_proxy(status: u16) -> (String, Arc<AtomicUsize>) {
    spawn_counting_http_server(status)
}

fn spawn_counting_http_server(status: u16) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = "{}";
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    (format!("http://{addr}"), counter)
}

fn rewrite_loopback_host(url: &str, host: &str) -> String {
    let parsed = reqwest::Url::parse(url).expect("test url");
    format!("{}://{}:{}", parsed.scheme(), host, parsed.port().unwrap())
}

struct EnvGuard {
    entries: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set_proxy(proxy_url: &str) -> Self {
        let keys = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ];
        let entries = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            unsafe {
                std::env::set_var(key, proxy_url);
            }
        }
        for key in ["NO_PROXY", "no_proxy"] {
            unsafe {
                std::env::remove_var(key);
            }
        }
        Self { entries }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.entries.drain(..) {
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
}

fn setup_provider_dir(base_url: &str, provider_id: &str) -> std::path::PathBuf {
    let base = std::path::PathBuf::from(format!(
        "/tmp/rua-doctor-provider-http-{}-{provider_id}",
        std::process::id(),
    ));
    let _ = std::fs::remove_dir_all(&base);
    let secret_dir = base.join("secret");
    let providers_dir = secret_dir.join("providers");
    let _ = std::fs::create_dir_all(&providers_dir);

    let config = serde_json::json!({
        "id": provider_id,
        "name": "Test Provider",
        "base_url": base_url,
        "api_key": "",
        "models": []
    });

    std::fs::write(
        providers_dir.join(format!("{provider_id}.json")),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    base
}

fn cleanup(base: &std::path::Path) {
    let _ = std::fs::remove_dir_all(base);
}

fn make_paths(base: &std::path::Path) -> Paths {
    Paths {
        rua_root: base.to_path_buf(),
        secret_dir: base.join("secret"),
        cache_dir: base.join("cache"),
    }
}

fn run_provider_http_check(paths: &Paths, deep: bool) -> Option<rua_doctor::registry::CheckResult> {
    let ctx = CheckContext { paths };
    let (results, _) = run_all_checks_filtered(&ctx, deep);
    results.into_iter().find(|r| r.name == "provider_http")
}

fn run_provider_http_only(paths: &Paths) -> rua_doctor::registry::CheckResult {
    let ctx = CheckContext { paths };
    let check = inventory::iter::<RegisteredCheck>
        .into_iter()
        .find(|check| check.name == "provider_http")
        .expect("provider_http check must be registered");
    (check.run)(&ctx)
}

#[test]
fn provider_http_pass_on_200() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (url, _counter) = spawn_mock_server(200);
    let base = setup_provider_dir(&url, "test-pass");
    let paths = make_paths(&base);

    let result = run_provider_http_only(&paths);

    assert_eq!(result.status, CheckStatus::Pass);
    assert!(result.message.contains("1/1"));

    cleanup(&base);
}

#[test]
fn provider_http_fail_on_refused() {
    let _guard = provider_http_test_lock();
    clear_cache();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let url = format!("http://127.0.0.1:{port}");
    let base = setup_provider_dir(&url, "test-refused");
    let paths = make_paths(&base);

    let result = run_provider_http_only(&paths);

    assert_eq!(result.status, CheckStatus::Fail);
    assert!(result.message.contains("0/1"));

    let details: serde_json::Value = result.details.unwrap();
    let provider = &details["providers"][0];
    assert_eq!(provider["provider_id"], "test-refused");
    assert!(!provider["reachable"].as_bool().unwrap());
    assert!(!provider["error"].as_str().unwrap().is_empty());

    cleanup(&base);
}

#[test]
fn provider_http_cache_prevents_second_request() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (url, counter) = spawn_mock_server(200);
    let base = setup_provider_dir(&url, "test-cache");
    let paths = make_paths(&base);

    let r1 = run_provider_http_only(&paths);
    assert_eq!(r1.status, CheckStatus::Pass);
    let count_after_first = counter.load(Ordering::SeqCst);
    assert!(count_after_first >= 1, "at least one HTTP request expected");

    let r2 = run_provider_http_only(&paths);
    assert_eq!(r2.status, CheckStatus::Pass);
    let count_after_second = counter.load(Ordering::SeqCst);
    assert_eq!(
        count_after_first, count_after_second,
        "cache must prevent second HTTP request"
    );

    cleanup(&base);
}

#[test]
fn provider_http_bypasses_proxy_for_loopback_provider_url() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (provider_url, provider_counter) = spawn_mock_server(200);
    let (proxy_url, proxy_counter) = spawn_counting_proxy(502);
    let _env = EnvGuard::set_proxy(&proxy_url);
    let base = setup_provider_dir(&provider_url, "test-loopback-proxy");
    let paths = make_paths(&base);

    let result = run_provider_http_only(&paths);

    assert_eq!(result.status, CheckStatus::Pass);
    assert!(
        provider_counter.load(Ordering::SeqCst) >= 1,
        "loopback provider should be contacted directly"
    );
    assert_eq!(
        proxy_counter.load(Ordering::SeqCst),
        0,
        "loopback provider probes must not inherit HTTP proxy env"
    );

    cleanup(&base);
}

#[test]
fn provider_http_bypasses_proxy_for_localhost_provider_url() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (provider_url, provider_counter) = spawn_mock_server(200);
    let provider_url = rewrite_loopback_host(&provider_url, "localhost.");
    let (proxy_url, proxy_counter) = spawn_counting_proxy(502);
    let _env = EnvGuard::set_proxy(&proxy_url);
    let base = setup_provider_dir(&provider_url, "test-localhost-proxy");
    let paths = make_paths(&base);

    let result = run_provider_http_only(&paths);

    assert_eq!(result.status, CheckStatus::Pass);
    assert!(
        provider_counter.load(Ordering::SeqCst) >= 1,
        "localhost provider should be contacted directly"
    );
    assert_eq!(
        proxy_counter.load(Ordering::SeqCst),
        0,
        "localhost provider probes must not inherit HTTP proxy env"
    );

    cleanup(&base);
}

#[test]
fn provider_http_keeps_proxy_for_remote_provider_url() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (proxy_url, proxy_counter) = spawn_counting_proxy(200);
    let _env = EnvGuard::set_proxy(&proxy_url);
    let base = setup_provider_dir("http://example.test", "test-remote-proxy");
    let paths = make_paths(&base);

    let result = run_provider_http_only(&paths);

    assert_eq!(result.status, CheckStatus::Pass);
    assert!(
        proxy_counter.load(Ordering::SeqCst) >= 1,
        "remote provider probes should preserve user proxy env"
    );

    cleanup(&base);
}

#[test]
fn provider_http_skipped_without_deep() {
    let _guard = provider_http_test_lock();
    clear_cache();
    let (url, _counter) = spawn_mock_server(200);
    let base = setup_provider_dir(&url, "test-no-deep");
    let paths = make_paths(&base);

    let result = run_provider_http_check(&paths, false);
    assert!(
        result.is_none(),
        "provider_http must be skipped when deep=false"
    );

    cleanup(&base);
}
