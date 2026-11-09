use std::path::Path;

#[test]
fn injected_dir_takes_priority() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("console");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<html></html>").unwrap();

    let result = rua_server::static_assets::resolve_static_dir(Some(dir.as_path()));
    assert_eq!(result, Some(dir));
}

#[test]
fn injected_dir_none_falls_through() {
    let result = rua_server::static_assets::resolve_static_dir(None);

    let _ = result;
}

#[test]
fn injected_dir_missing_index_html_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("empty-console");
    std::fs::create_dir_all(&dir).unwrap();

    let result = rua_server::static_assets::resolve_static_dir(Some(dir.as_path()));

    assert_ne!(result, Some(dir));
}

#[test]
fn has_index_html_true() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
    assert!(rua_server::static_assets::has_index_html(tmp.path()));
}

#[test]
fn has_index_html_false_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!rua_server::static_assets::has_index_html(tmp.path()));
}

#[test]
fn has_index_html_false_nonexistent() {
    assert!(!rua_server::static_assets::has_index_html(Path::new(
        "/nonexistent"
    )));
}
