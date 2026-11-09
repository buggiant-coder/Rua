use axum::http::header;
use http_body_util::BodyExt;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let old = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, old }
    }

    fn remove(key: &'static str) -> Self {
        let old = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            std::env::set_var(self.key, old);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

async fn read_text(resp: axum::response::Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

#[test]
fn static_asset_path_sanitization_rejects_traversal_and_null_bytes() {
    assert_eq!(
        rua_server::static_assets::sanitize_path(""),
        Some("index.html".to_string())
    );
    assert_eq!(
        rua_server::static_assets::sanitize_path("/assets/app.js"),
        Some("assets/app.js".to_string())
    );
    assert!(rua_server::static_assets::sanitize_path("/../secret.txt").is_none());
    assert!(rua_server::static_assets::sanitize_path("/assets/../secret.txt").is_none());
    assert!(rua_server::static_assets::sanitize_path("/assets/app.js\0.txt").is_none());
}

#[test]
#[serial]
fn resolve_static_dir_prefers_valid_injected_dir_over_environment() {
    let injected = tempfile::tempdir().expect("injected dir");
    std::fs::write(injected.path().join("index.html"), "<html>injected</html>")
        .expect("write injected index");
    let env_dir = tempfile::tempdir().expect("env dir");
    std::fs::write(env_dir.path().join("index.html"), "<html>env</html>").expect("write env index");
    let _env = EnvGuard::set("RUA_CONSOLE_STATIC_DIR", env_dir.path());

    let resolved = rua_server::static_assets::resolve_static_dir(Some(injected.path()));

    assert_eq!(resolved.as_deref(), Some(injected.path()));
}

#[test]
#[serial]
fn resolve_static_dir_uses_environment_when_injected_dir_is_invalid() {
    let injected = tempfile::tempdir().expect("injected dir");
    let env_dir = tempfile::tempdir().expect("env dir");
    std::fs::write(env_dir.path().join("index.html"), "<html>env</html>").expect("write env index");
    let _env = EnvGuard::set("RUA_CONSOLE_STATIC_DIR", env_dir.path());

    let resolved = rua_server::static_assets::resolve_static_dir(Some(injected.path()));

    assert_eq!(resolved.as_deref(), Some(env_dir.path()));
}

#[test]
#[serial]
fn resolve_static_dir_returns_none_when_no_candidate_has_index_html() {
    let injected = tempfile::tempdir().expect("injected dir");
    let _env = EnvGuard::remove("RUA_CONSOLE_STATIC_DIR");

    let resolved = rua_server::static_assets::resolve_static_dir(Some(injected.path()));

    assert_ne!(resolved.as_deref(), Some(injected.path()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_static_file_sets_cache_policy_by_asset_kind() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets")).expect("assets dir");
    std::fs::write(tmp.path().join("index.html"), "<!doctype html>").expect("index");
    std::fs::write(tmp.path().join("logo.png"), b"png").expect("logo");
    std::fs::write(tmp.path().join("assets").join("app.123.js"), "bundle").expect("asset");

    let index = rua_server::static_assets::serve_static_file(tmp.path(), "/index.html")
        .expect("index response");
    assert_eq!(
        index.headers().get(header::CACHE_CONTROL),
        Some(&"no-cache, no-store, must-revalidate".parse().unwrap())
    );
    assert_eq!(read_text(index).await, "<!doctype html>");

    let root_asset =
        rua_server::static_assets::serve_static_file(tmp.path(), "/logo.png").expect("logo");
    assert_eq!(
        root_asset.headers().get(header::CACHE_CONTROL),
        Some(&"public, max-age=3600".parse().unwrap())
    );
    let root_bytes = root_asset
        .into_body()
        .collect()
        .await
        .expect("collect logo")
        .to_bytes();
    assert_eq!(root_bytes.as_ref(), b"png");

    let hashed_asset =
        rua_server::static_assets::serve_static_file(tmp.path(), "/assets/app.123.js")
            .expect("hashed asset");
    assert_eq!(
        hashed_asset.headers().get(header::CACHE_CONTROL),
        Some(&"public, max-age=31536000, immutable".parse().unwrap())
    );
    assert_eq!(read_text(hashed_asset).await, "bundle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn serve_static_file_rejects_paths_outside_static_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let static_dir = tmp.path().join("static");
    let outside_dir = tmp.path().join("outside");
    std::fs::create_dir_all(&static_dir).expect("static dir");
    std::fs::create_dir_all(&outside_dir).expect("outside dir");
    std::fs::write(outside_dir.join("secret.txt"), "secret").expect("secret");
    std::os::unix::fs::symlink(
        outside_dir.join("secret.txt"),
        static_dir.join("secret-link.txt"),
    )
    .expect("symlink");

    let resp = rua_server::static_assets::serve_static_file(&static_dir, "/secret-link.txt");
    assert!(resp.is_none(), "symlink escape must not be served");
}
