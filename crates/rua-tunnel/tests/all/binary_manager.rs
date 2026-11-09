use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rua_tunnel::{
    cloudflared_release_targets, extract_cloudflared_from_tgz, verify_checksum, BinaryManager,
    BinaryManagerError,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, MutexGuard};

static PATH_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

async fn path_test_guard() -> MutexGuard<'static, ()> {
    PATH_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await
}

struct PathGuard {
    previous: Option<OsString>,
}

impl PathGuard {
    fn set(path: impl AsRef<Path>) -> Self {
        let previous = std::env::var_os("PATH");
        std::env::set_var("PATH", path.as_ref());
        Self { previous }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var("PATH", previous);
        } else {
            std::env::remove_var("PATH");
        }
    }
}

#[cfg(unix)]
fn write_executable(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn build_test_tgz(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.finish().unwrap();
}

#[test]
fn cloudflared_release_manifest_pins_supported_platform_artifacts() {
    let targets = cloudflared_release_targets();
    assert_eq!(targets.len(), 5);

    for target in targets {
        assert_eq!(target.version, "2026.2.0");
        assert!(
            target.download_url.starts_with(
                "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/"
            ),
            "unexpected URL for {}/{}: {}",
            target.os,
            target.arch,
            target.download_url
        );
        assert_eq!(
            target.sha256.len(),
            64,
            "checksum must be a sha256 hex digest for {}/{}",
            target.os,
            target.arch
        );
        assert!(target.sha256.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    let darwin: Vec<_> = targets
        .iter()
        .filter(|target| target.os == "macos")
        .collect();
    assert_eq!(darwin.len(), 2);
    assert!(darwin.iter().all(|target| target.is_tgz));
    assert!(darwin.iter().any(|target| target
        .download_url
        .ends_with("cloudflared-darwin-amd64.tgz")));
    assert!(darwin.iter().any(|target| target
        .download_url
        .ends_with("cloudflared-darwin-arm64.tgz")));

    let linux: Vec<_> = targets
        .iter()
        .filter(|target| target.os == "linux")
        .collect();
    assert_eq!(linux.len(), 2);
    assert!(linux.iter().all(|target| !target.is_tgz));
    assert!(linux
        .iter()
        .any(|target| target.download_url.ends_with("cloudflared-linux-amd64")));
    assert!(linux
        .iter()
        .any(|target| target.download_url.ends_with("cloudflared-linux-arm64")));

    let windows = targets
        .iter()
        .find(|target| target.os == "windows" && target.arch == "x86_64")
        .expect("windows x86_64 target");
    assert_eq!(windows.bin_name, "cloudflared.exe");
    assert!(windows
        .download_url
        .ends_with("cloudflared-windows-amd64.exe"));
    assert!(!windows.is_tgz);
}

#[test]
fn cloudflared_release_manifest_has_unique_platform_keys_and_binary_names() {
    let targets = cloudflared_release_targets();
    let mut platform_keys = HashSet::new();
    let mut download_urls = HashSet::new();

    for target in targets {
        assert!(
            platform_keys.insert((target.os, target.arch)),
            "duplicate cloudflared target for {}/{}",
            target.os,
            target.arch
        );
        assert!(
            download_urls.insert(target.download_url),
            "duplicate cloudflared download URL: {}",
            target.download_url
        );
        assert!(
            matches!(target.bin_name, "cloudflared" | "cloudflared.exe"),
            "unexpected binary name for {}/{}: {}",
            target.os,
            target.arch,
            target.bin_name
        );
        if target.os == "windows" {
            assert_eq!(target.bin_name, "cloudflared.exe");
            assert!(!target.is_tgz);
        } else {
            assert_eq!(target.bin_name, "cloudflared");
        }
    }
}

#[test]
fn verify_checksum_accepts_expected_sha256_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.bin");
    std::fs::write(&path, b"hello world").unwrap();

    let mut hasher = Sha256::new();
    hasher.update(b"hello world");
    let expected = format!("{:x}", hasher.finalize());

    verify_checksum(&path, &expected).expect("checksum should match");
    assert!(path.exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn ensure_binary_prefers_path_cloudflared() {
    let _guard = path_test_guard().await;
    let path_dir = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let path_binary = write_executable(path_dir.path(), "cloudflared", b"path binary");
    let local_binary = write_executable(bin_dir.path(), "cloudflared", b"local binary");
    let _path_guard = PathGuard::set(path_dir.path());

    let manager = BinaryManager::new(bin_dir.path().to_path_buf());
    let resolved = manager.ensure_binary().await.unwrap();

    assert_eq!(resolved, path_binary);
    assert_ne!(resolved, local_binary);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn ensure_binary_uses_cached_local_executable_when_path_misses() {
    let _guard = path_test_guard().await;
    let empty_path_dir = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let local_binary = write_executable(bin_dir.path(), "cloudflared", b"local binary");
    let _path_guard = PathGuard::set(empty_path_dir.path());

    let manager = BinaryManager::new(bin_dir.path().to_path_buf());
    let resolved = manager.ensure_binary().await.unwrap();

    assert_eq!(resolved, local_binary);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn ensure_binary_ignores_non_executable_path_candidate() {
    let _guard = path_test_guard().await;
    let path_dir = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let path_candidate = path_dir.path().join("cloudflared");
    std::fs::write(&path_candidate, b"not executable").unwrap();
    let local_binary = write_executable(bin_dir.path(), "cloudflared", b"local binary");
    let _path_guard = PathGuard::set(path_dir.path());

    let manager = BinaryManager::new(bin_dir.path().to_path_buf());
    let resolved = manager.ensure_binary().await.unwrap();

    assert_eq!(resolved, local_binary);
    assert_ne!(resolved, path_candidate);
}

#[test]
fn checksum_mismatch_deletes_suspect_download() {
    let bin_dir = tempfile::tempdir().unwrap();
    let downloaded = bin_dir.path().join("cloudflared");
    std::fs::write(&downloaded, b"corrupt binary").unwrap();

    let err = verify_checksum(
        &downloaded,
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .expect_err("bad checksum must be rejected");

    assert!(
        err.to_string().contains("SHA256 mismatch"),
        "unexpected checksum error: {err}"
    );
    assert!(
        !downloaded.exists(),
        "checksum mismatch must remove the suspect cached binary"
    );
}

#[test]
fn tgz_extraction_writes_cloudflared_binary() {
    let dir = tempfile::tempdir().unwrap();
    let tgz_path = dir.path().join("cf.tgz");
    let dest = dir.path().join("cloudflared");
    let content = b"fake-cloudflared-binary";

    build_test_tgz(&tgz_path, &[("nested/cloudflared", content)]);

    extract_cloudflared_from_tgz(&tgz_path, &dest).expect("extract cloudflared from tgz");

    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), content);
}

#[test]
fn tgz_extraction_rejects_archives_without_cloudflared_binary() {
    let dir = tempfile::tempdir().unwrap();
    let tgz_path = dir.path().join("empty.tgz");
    let dest = dir.path().join("cloudflared");

    build_test_tgz(&tgz_path, &[("somefile.txt", b"not cloudflared")]);

    let result = extract_cloudflared_from_tgz(&tgz_path, &dest);

    assert!(matches!(
        result,
        Err(BinaryManagerError::ArchiveMissingBinary)
    ));
    assert!(!dest.exists());
}
