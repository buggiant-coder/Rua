use std::io::{Cursor, Write};

use tempfile::TempDir;

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buf = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buf);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn rejects_zip_with_dot_dot_traversal() {
    let tmp = TempDir::new().unwrap();
    let extract_dir = tmp.path().join("extracted");

    let zip_bytes = make_zip(&[("../../../evil.txt", b"pwned")]);

    let result = rua_cli::plugin::safe_extract_zip(&zip_bytes, &extract_dir);
    assert!(result.is_err(), "should reject zip with ../ traversal");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("zip-slip"),
        "error should mention zip-slip: {err_msg}"
    );

    assert!(
        !tmp.path().join("evil.txt").exists(),
        "evil.txt must not exist outside extract_dir"
    );
}

#[test]
fn rejects_zip_with_absolute_path() {
    let tmp = TempDir::new().unwrap();
    let extract_dir = tmp.path().join("extracted");

    let zip_bytes = make_zip(&[("/etc/evil.txt", b"pwned")]);

    let result = rua_cli::plugin::safe_extract_zip(&zip_bytes, &extract_dir);
    assert!(result.is_err(), "should reject zip with absolute path");
}

#[test]
fn extracts_normal_zip_successfully() {
    let tmp = TempDir::new().unwrap();
    let extract_dir = tmp.path().join("extracted");

    let zip_bytes = make_zip(&[
        ("plugin.json", b"{\"id\":\"test\",\"name\":\"Test\"}"),
        ("subdir/file.txt", b"hello"),
    ]);

    let result = rua_cli::plugin::safe_extract_zip(&zip_bytes, &extract_dir);
    assert!(result.is_ok(), "normal zip should extract: {result:?}");
    assert!(extract_dir.join("plugin.json").is_file());
    assert!(extract_dir.join("subdir/file.txt").is_file());
}

#[test]
fn rejects_zip_with_mixed_good_and_bad_entries() {
    let tmp = TempDir::new().unwrap();
    let extract_dir = tmp.path().join("extracted");

    let zip_bytes = make_zip(&[("good.txt", b"safe"), ("../../evil.txt", b"pwned")]);

    let result = rua_cli::plugin::safe_extract_zip(&zip_bytes, &extract_dir);
    assert!(result.is_err(), "should reject zip with any bad entry");
}
