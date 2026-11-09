use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::info;

const CLOUDFLARED_VERSION: &str = "2026.2.0";

const DOWNLOAD_TIMEOUT_SECS: u64 = 90;
const CONNECT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudflaredReleaseTarget {
    pub os: &'static str,
    pub arch: &'static str,
    pub version: &'static str,
    pub download_url: &'static str,
    pub sha256: &'static str,
    pub bin_name: &'static str,
    pub is_tgz: bool,
}

const RELEASE_TARGETS: &[CloudflaredReleaseTarget] = &[
    CloudflaredReleaseTarget {
        os: "macos",
        arch: "x86_64",
        version: CLOUDFLARED_VERSION,
        download_url: "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-darwin-amd64.tgz",
        sha256: "685688a260c324eb8d9c9434ca22f0ce4f504fd6acd0706787c4833de8d6eb17",
        bin_name: "cloudflared",
        is_tgz: true,
    },
    CloudflaredReleaseTarget {
        os: "macos",
        arch: "aarch64",
        version: CLOUDFLARED_VERSION,
        download_url: "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-darwin-arm64.tgz",
        sha256: "ba99c6f87320236b9f842c3ba4b9526f687560125b7b43a581201579543ca4ff",
        bin_name: "cloudflared",
        is_tgz: true,
    },
    CloudflaredReleaseTarget {
        os: "linux",
        arch: "x86_64",
        version: CLOUDFLARED_VERSION,
        download_url: "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-linux-amd64",
        sha256: "176746db3be7dc7bd48f3dd287c8930a4645ebb6e6700f883fddda5a4c307c16",
        bin_name: "cloudflared",
        is_tgz: false,
    },
    CloudflaredReleaseTarget {
        os: "linux",
        arch: "aarch64",
        version: CLOUDFLARED_VERSION,
        download_url: "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-linux-arm64",
        sha256: "03c5d58e283f521d752dc4436014eb341092edf076eb1095953ab82debe54a8e",
        bin_name: "cloudflared",
        is_tgz: false,
    },
    CloudflaredReleaseTarget {
        os: "windows",
        arch: "x86_64",
        version: CLOUDFLARED_VERSION,
        download_url: "https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-windows-amd64.exe",
        sha256: "b3279f2186a1c3c438ad5865e802bbbec26090c5d3fdb4ac1113f1143a94837a",
        bin_name: "cloudflared.exe",
        is_tgz: false,
    },
];

pub fn cloudflared_release_targets() -> &'static [CloudflaredReleaseTarget] {
    RELEASE_TARGETS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Platform {
    DarwinAmd64,
    DarwinArm64,
    LinuxAmd64,
    LinuxArm64,
    WindowsAmd64,
}

impl Platform {
    fn detect() -> Result<Self, Error> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        match (os, arch) {
            ("macos", "x86_64") => Ok(Self::DarwinAmd64),
            ("macos", "aarch64") => Ok(Self::DarwinArm64),
            ("linux", "x86_64") => Ok(Self::LinuxAmd64),
            ("linux", "aarch64") => Ok(Self::LinuxArm64),
            ("windows", "x86_64") => Ok(Self::WindowsAmd64),
            _ => Err(Error::UnsupportedPlatform {
                os: os.to_string(),
                arch: arch.to_string(),
            }),
        }
    }

    fn download_url(&self) -> String {
        self.release_target().download_url.to_string()
    }

    fn sha256(&self) -> &'static str {
        self.release_target().sha256
    }

    fn is_tgz(&self) -> bool {
        self.release_target().is_tgz
    }

    fn bin_name(&self) -> &'static str {
        self.release_target().bin_name
    }

    fn release_target(&self) -> &'static CloudflaredReleaseTarget {
        match self {
            Self::DarwinAmd64 => &RELEASE_TARGETS[0],
            Self::DarwinArm64 => &RELEASE_TARGETS[1],
            Self::LinuxAmd64 => &RELEASE_TARGETS[2],
            Self::LinuxArm64 => &RELEASE_TARGETS[3],
            Self::WindowsAmd64 => &RELEASE_TARGETS[4],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported platform: os={os}, arch={arch}")]
    UnsupportedPlatform { os: String, arch: String },
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("HTTP client build failed: {0}")]
    HttpClient(#[from] rua_http::HttpClientError),
    #[error("SHA256 mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive does not contain cloudflared binary")]
    ArchiveMissingBinary,
}

pub struct BinaryManager {
    bin_dir: PathBuf,
}

impl BinaryManager {
    pub fn new(bin_dir: PathBuf) -> Self {
        Self { bin_dir }
    }

    pub async fn ensure_binary(&self) -> Result<PathBuf, Error> {
        if let Some(path) = Self::find_in_path() {
            return Ok(path);
        }

        let platform = Platform::detect()?;
        let local = self.bin_dir.join(platform.bin_name());
        if local.is_file() && is_executable(&local) {
            return Ok(local);
        }

        self.download(platform).await
    }

    fn find_in_path() -> Option<PathBuf> {
        which("cloudflared")
    }

    async fn download(&self, platform: Platform) -> Result<PathBuf, Error> {
        let url = platform.download_url();
        let expected_hash = platform.sha256();

        tokio::fs::create_dir_all(&self.bin_dir).await?;
        let dest = self.bin_dir.join(platform.bin_name());

        info!("Downloading cloudflared {CLOUDFLARED_VERSION} from {url}");

        let client = rua_http::build_client(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
                .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS)),
        )?;

        if platform.is_tgz() {
            self.download_tgz(&client, &url, expected_hash, &dest)
                .await?;
        } else {
            download_file(&client, &url, &dest).await?;
            verify_checksum(&dest, expected_hash)?;
        }

        #[cfg(unix)]
        set_executable(&dest)?;

        info!(
            "cloudflared {CLOUDFLARED_VERSION} installed to {}",
            dest.display()
        );
        Ok(dest)
    }

    async fn download_tgz(
        &self,
        client: &reqwest::Client,
        url: &str,
        expected_hash: &str,
        dest: &Path,
    ) -> Result<(), Error> {
        let tmp_path = self.bin_dir.join("cloudflared.tgz.tmp");
        download_file(client, url, &tmp_path).await?;

        let verify_result = verify_checksum(&tmp_path, expected_hash);
        if verify_result.is_err() {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return verify_result;
        }

        extract_cloudflared_from_tgz(&tmp_path, dest)?;
        let _ = tokio::fs::remove_file(&tmp_path).await;
        Ok(())
    }
}

async fn download_file(client: &reqwest::Client, url: &str, dest: &Path) -> Result<(), Error> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    tokio::fs::write(dest, &bytes).await?;
    Ok(())
}

pub fn verify_checksum(path: &Path, expected: &str) -> Result<(), Error> {
    let data = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(path);
        return Err(Error::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

#[doc(hidden)]
pub fn extract_cloudflared_from_tgz(tgz_path: &Path, dest: &Path) -> Result<(), Error> {
    let file = std::fs::File::open(tgz_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "cloudflared" {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    Err(Error::ArchiveMissingBinary)
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() && is_executable(&full) {
                Some(full)
            } else {
                None
            }
        })
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path)?;
    let mut perms = meta.permissions();
    perms.set_mode(perms.mode() | 0o550);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
