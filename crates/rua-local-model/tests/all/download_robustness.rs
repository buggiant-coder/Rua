use std::io::Write;

use rua_local_model::robustness::{check_disk_space, compute_sha256, resume_offset};

#[test]
fn resume_offset_uses_existing_partial_file_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.bin");
    let data = vec![0u8; 4096];
    std::fs::write(&path, &data).unwrap();

    assert_eq!(resume_offset(&path), 4096);
}

#[test]
fn resume_offset_missing_file_starts_from_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.bin");

    assert_eq!(resume_offset(&path), 0);
}

#[test]
fn compute_sha256_matches_known_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"hello world").unwrap();
    drop(file);

    let hash = compute_sha256(&path).unwrap();

    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn check_disk_space_rejects_impossible_requirement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big_file.bin");

    let result = check_disk_space(&path, u64::MAX);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("insufficient disk space"));
}
