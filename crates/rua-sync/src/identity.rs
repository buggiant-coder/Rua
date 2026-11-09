use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::crypto::{DeviceKey, SyncKey};
use crate::SyncError;

const NETWORK_SECRET_FILENAME: &str = "network_secret.key";

pub struct NetworkSecret {
    inner: iroh_docs::NamespaceSecret,
}

impl NetworkSecret {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut bytes);
        Self {
            inner: iroh_docs::NamespaceSecret::from_bytes(&bytes),
        }
    }

    pub fn from_namespace_secret(ns: iroh_docs::NamespaceSecret) -> Self {
        Self { inner: ns }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            inner: iroh_docs::NamespaceSecret::from_bytes(bytes),
        }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    pub fn namespace_secret(&self) -> &iroh_docs::NamespaceSecret {
        &self.inner
    }

    pub fn into_namespace_secret(self) -> iroh_docs::NamespaceSecret {
        self.inner
    }

    pub fn namespace_id(&self) -> iroh_docs::NamespaceId {
        self.inner.id()
    }

    pub fn derive_sync_key(&self) -> SyncKey {
        use hkdf::Hkdf;
        use sha2::Sha256;

        // Persisted ciphertext and entry keys depend on this protocol salt.
        // Keep it stable across product renames.
        let hk = Hkdf::<Sha256>::new(Some(b"copal-sync-v3"), &self.to_bytes());
        let mut okm = [0u8; 32];
        hk.expand(b"chacha20poly1305", &mut okm)
            .expect("32-byte output is always valid for HKDF-SHA256");
        SyncKey::from_bytes(okm)
    }

    pub fn load(sync_dir: &Path) -> Result<Option<Self>, SyncError> {
        let path = Self::file_path(sync_dir);
        if !path.exists() {
            return Ok(None);
        }
        let hex_str = std::fs::read_to_string(&path)?;
        let bytes: [u8; 32] = hex::decode(hex_str.trim())
            .map_err(|e| SyncError::Keystore(format!("invalid network_secret hex: {e}")))?
            .try_into()
            .map_err(|_| SyncError::Keystore("network_secret.key: expected 32 bytes".into()))?;
        Ok(Some(Self::from_bytes(&bytes)))
    }

    pub fn save(&self, sync_dir: &Path) -> Result<PathBuf, SyncError> {
        std::fs::create_dir_all(sync_dir)?;
        let path = Self::file_path(sync_dir);
        let hex_str = hex::encode(self.to_bytes());
        atomic_write_private_file(&path, hex_str.as_bytes())?;
        tracing::info!("[sync] saved network secret to {}", path.display());
        Ok(path)
    }

    pub fn delete(sync_dir: &Path) -> Result<(), SyncError> {
        let path = Self::file_path(sync_dir);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn file_path(sync_dir: &Path) -> PathBuf {
        sync_dir.join(NETWORK_SECRET_FILENAME)
    }
}

impl DeviceKey {
    pub fn generate_or_load(secret_dir: &Path) -> Result<Self, SyncError> {
        let path = secret_dir.join("device_key.key");
        if path.exists() {
            let hex_str = std::fs::read_to_string(&path)?;
            let bytes: [u8; 32] = hex::decode(hex_str.trim())
                .map_err(|e| SyncError::Keystore(format!("invalid device_key hex: {e}")))?
                .try_into()
                .map_err(|_| SyncError::Keystore("device_key.key: expected 32 bytes".into()))?;
            return Ok(Self::from_bytes(bytes));
        }
        let key = Self::generate();
        let hex_str = hex::encode(key.as_bytes());
        atomic_write_private_file(&path, hex_str.as_bytes())?;
        tracing::info!("[sync] generated new device key at {}", path.display());
        Ok(key)
    }
}

#[derive(Serialize, Deserialize)]
pub struct DeviceIdentity {
    secret_bytes: [u8; 32],
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let key = SigningKey::generate(&mut OsRng);
        Self {
            secret_bytes: key.to_bytes(),
        }
    }

    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.secret_bytes)
    }

    pub fn public_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signing_key().verifying_key()
    }

    pub fn device_id_short(&self) -> [u8; 8] {
        let pk = self.public_key();
        let mut id = [0u8; 8];
        id.copy_from_slice(&pk.as_bytes()[..8]);
        id
    }

    pub fn to_iroh_author(&self) -> iroh_docs::Author {
        iroh_docs::Author::from_bytes(&self.secret_bytes)
    }

    pub fn generate_or_load(data_dir: &Path) -> Result<Self, SyncError> {
        let path = data_dir.join("device_identity.json");
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let identity: Self = serde_json::from_str(&content)?;
            return Ok(identity);
        }
        let identity = Self::generate();
        let content = serde_json::to_string_pretty(&identity)?;
        atomic_write_private_file(&path, content.as_bytes())?;
        Ok(identity)
    }
}

fn atomic_write_private_file(path: &Path, bytes: &[u8]) -> Result<(), SyncError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp_path = private_tmp_path_for(path);
    {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp_path, path)?;
    #[cfg(unix)]
    {
        let parent_dir = std::fs::File::open(parent)?;
        parent_dir.sync_all()?;
    }
    Ok(())
}

fn private_tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sync-identity");
    path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
