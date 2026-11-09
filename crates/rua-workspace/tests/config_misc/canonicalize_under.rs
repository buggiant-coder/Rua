use std::fs;

use rua_workspace::{DefaultWorkspaceFs, WorkspaceFs, WorkspaceRoot};
use tempfile::TempDir;

fn build_fs(root: &std::path::Path) -> DefaultWorkspaceFs {
    let workspace_root = WorkspaceRoot::new(root);
    DefaultWorkspaceFs::new(workspace_root)
}

#[test]
fn accepts_valid_path_under_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("child")).unwrap();
    fs::write(root.join("child/file.txt"), b"hello").unwrap();

    let fs_impl = build_fs(&root);
    let resolved = fs_impl
        .canonicalize_under(&root, "child/file.txt")
        .expect("valid path should resolve");

    assert!(resolved.starts_with(&root));
    assert_eq!(resolved.file_name().unwrap(), "file.txt");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escaping_root() {
    let scratch = TempDir::new().unwrap();
    let base = scratch.path().canonicalize().unwrap();
    let root = base.join("inside");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"nope").unwrap();

    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("escape.txt")).unwrap();

    let fs_impl = build_fs(&root);
    let err = fs_impl
        .canonicalize_under(&root, "escape.txt")
        .expect_err("symlink escape must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("escape") || msg.contains("escapes") || msg.contains("path"),
        "expected path-escape error, got: {msg}"
    );
}

#[test]
fn rejects_parent_traversal() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("child")).unwrap();

    let fs_impl = build_fs(&root);
    let err = fs_impl
        .canonicalize_under(&root, "../etc/passwd")
        .expect_err("parent traversal must be rejected");
    assert!(err.to_string().to_lowercase().contains("path"));
}

#[test]
fn rejects_nonexistent_target() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();

    let fs_impl = build_fs(&root);
    let err = fs_impl
        .canonicalize_under(&root, "does-not-exist.txt")
        .expect_err("missing target must error");
    assert!(err.to_string().to_lowercase().contains("canonicalize"));
}
