use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn business_code_does_not_call_legacy_v4_emit_helpers() {
    for path in rust_files(Path::new("src/endpoints")) {
        let src = fs::read_to_string(&path).expect("read source file");
        assert!(
            !src.contains("sync_ops::emit_") && !src.contains("services::sync_ops::emit_"),
            "{} must publish typed facts through the Replication Publication Seam, not call legacy emit helpers",
            path.display()
        );
    }
}

#[test]
fn sync_ops_does_not_expose_domain_specific_emit_helpers() {
    let src = include_str!("../../src/services/sync_ops.rs");
    assert!(
        !src.contains("pub async fn emit_") && !src.contains("pub fn normalize_"),
        "sync_ops.rs must keep runtime operations such as full export, not public domain-specific emit helpers"
    );
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rust_files(root, &mut out);
    out
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("read source dir");
    for entry in entries {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}
