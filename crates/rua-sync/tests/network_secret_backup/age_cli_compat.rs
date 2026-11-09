use std::io::Write;
use std::process::Command;

use rua_sync::export::encrypt_for_export;

fn age_cli_available() -> bool {
    Command::new("age")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn decrypt_with_age_cli() {
    if !age_cli_available() {
        eprintln!("WARNING: age CLI not installed, skipping age_cli_compat test");
        return;
    }

    let key_bytes: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];
    let passphrase = "test-passphrase-long-enough";

    let armored = encrypt_for_export(&key_bytes, passphrase).expect("encrypt_for_export failed");

    let tmp_dir = tempfile::tempdir().expect("create tmpdir");
    let age_file = tmp_dir.path().join("export.age");
    std::fs::write(&age_file, &armored).expect("write .age file");

    let mut child = Command::new("age")
        .arg("-d")
        .arg(age_file.to_str().unwrap())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn age -d");

    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "{}", passphrase).expect("write passphrase to stdin");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait age -d");

    assert!(
        output.status.success(),
        "age -d failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        output.stdout.as_slice(),
        &key_bytes,
        "decrypted output does not match original key bytes"
    );
}
