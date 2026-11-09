use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::{Path, PathBuf};

pub const HASHED_ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

pub fn resolve_static_dir(injected_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = injected_dir {
        if has_index_html(dir) {
            return Some(dir.to_path_buf());
        }
        tracing::warn!(
            "injected console-static-dir {:?} does not contain index.html",
            dir
        );
    }

    if let Ok(env_dir) = std::env::var("RUA_CONSOLE_STATIC_DIR") {
        let path = PathBuf::from(&env_dir);
        if has_index_html(&path) {
            return Some(path);
        }
        tracing::warn!(
            "RUA_CONSOLE_STATIC_DIR={:?} does not contain index.html",
            env_dir
        );
    }

    if let Some(dir) = resolve_binary_relative() {
        return Some(dir);
    }

    let fallbacks = ["console/dist", "console_dist"];
    for fallback in &fallbacks {
        let path = PathBuf::from(fallback);
        if has_index_html(&path) {
            tracing::info!("using dev fallback static dir: {:?}", path);
            return Some(path);
        }
    }

    tracing::info!("no WebUI static directory found; SPA fallback will return placeholder HTML");
    None
}

pub fn has_index_html(dir: &Path) -> bool {
    dir.is_dir() && dir.join("index.html").is_file()
}

fn resolve_binary_relative() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidates = [
        exe_dir.join("console/dist"),
        exe_dir.join("console_dist"),
        exe_dir.join("console"),
    ];
    for candidate in &candidates {
        if has_index_html(candidate) {
            tracing::info!("using binary-relative static dir: {:?}", candidate);
            return Some(candidate.clone());
        }
    }
    None
}

pub fn serve_static_file(static_dir: &Path, request_path: &str) -> Option<Response> {
    let clean_path = sanitize_path(request_path)?;
    let file_path = static_dir.join(&clean_path);

    let canonical_dir = static_dir.canonicalize().ok()?;
    let canonical_file = file_path.canonicalize().ok()?;
    if !canonical_file.starts_with(&canonical_dir) {
        return None;
    }

    if !file_path.is_file() {
        return None;
    }

    let content = std::fs::read(&file_path).ok()?;
    let mime = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .to_string();

    let is_index = clean_path == "index.html";
    let is_hashed_asset = clean_path.starts_with("assets/");

    let cache_control = if is_index {
        "no-cache, no-store, must-revalidate"
    } else if is_hashed_asset {
        HASHED_ASSET_CACHE_CONTROL
    } else {
        "public, max-age=3600"
    };

    Some(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, cache_control.to_string()),
            ],
            content,
        )
            .into_response(),
    )
}

pub fn serve_index(static_dir: &Path) -> Response {
    let index_path = static_dir.join("index.html");
    match std::fs::read(&index_path) {
        Ok(content) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (
                    header::CACHE_CONTROL,
                    "no-cache, no-store, must-revalidate".to_string(),
                ),
            ],
            content,
        )
            .into_response(),
        Err(_) => placeholder_index(),
    }
}

pub fn placeholder_index() -> Response {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>rua</title></head>
<body>
<h1>rua WebUI</h1>
<p>WebUI static files not found. Run <code>cd console &amp;&amp; npm run build</code> or set RUA_CONSOLE_STATIC_DIR.</p>
</body>
</html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
        html,
    )
        .into_response()
}

pub const ROOT_ASSETS: &[&str] = &[
    "favicon.ico",
    "logo.png",
    "dark-logo.png",
    "rua-symbol.svg",
    "rua-dark.png",
];

pub fn is_root_asset(path: &str) -> bool {
    let clean = path.strip_prefix('/').unwrap_or(path);
    ROOT_ASSETS.contains(&clean)
}

pub fn sanitize_path(path: &str) -> Option<String> {
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.contains('\0') {
        return None;
    }

    for component in path.split('/') {
        if component == ".." {
            return None;
        }
    }

    if path.is_empty() {
        return Some("index.html".to_string());
    }

    Some(path.to_string())
}
