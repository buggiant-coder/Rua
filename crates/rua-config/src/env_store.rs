use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::encrypted_io::EncryptedFileIO;
use crate::secrets::mask_env_value;

pub struct EnvStore {
    secret_dir: PathBuf,
    key: Option<[u8; 32]>,
}

impl EnvStore {
    pub fn new(secret_dir: PathBuf) -> Self {
        Self {
            secret_dir,
            key: None,
        }
    }

    pub fn with_key(secret_dir: PathBuf, key: [u8; 32]) -> Self {
        Self {
            secret_dir,
            key: Some(key),
        }
    }

    fn envs_path(&self) -> PathBuf {
        self.secret_dir.join("envs.json")
    }

    pub fn list(&self) -> std::io::Result<BTreeMap<String, String>> {
        load_envs_map(&self.envs_path(), self.key.as_ref())
    }

    pub fn list_masked(&self) -> std::io::Result<BTreeMap<String, String>> {
        Ok(self
            .list()?
            .into_iter()
            .map(|(k, v)| (k, mask_env_value(&v)))
            .collect())
    }

    pub fn get(&self, key: &str) -> std::io::Result<Option<String>> {
        Ok(self.list()?.remove(key))
    }

    pub fn save(&self, envs: BTreeMap<String, String>) -> std::io::Result<()> {
        let path = self.envs_path();
        write_envs(&path, &envs, self.key.as_ref())?;
        chmod_600(&path);
        Ok(())
    }

    pub fn set(&self, key: &str, value: String) -> std::io::Result<()> {
        let mut envs = self.list()?;
        envs.insert(key.to_string(), value);
        self.save(envs)
    }

    pub fn delete(&self, key: &str) -> std::io::Result<bool> {
        let mut envs = self.list()?;
        let removed = envs.remove(key).is_some();
        self.save(envs)?;
        Ok(removed)
    }
}

fn load_envs_map(path: &Path, key: Option<&[u8; 32]>) -> std::io::Result<BTreeMap<String, String>> {
    match EncryptedFileIO::read_json::<Value>(path, key) {
        Ok(Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if let Value::String(s) = v {
                    out.insert(k, s);
                }
            }
            Ok(out)
        }
        Ok(_) => Err(std::io::Error::other(format!(
            "{} is not a JSON object",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

fn write_envs(
    path: &Path,
    envs: &BTreeMap<String, String>,
    key: Option<&[u8; 32]>,
) -> std::io::Result<()> {
    EncryptedFileIO::write_json(path, envs, key)
}

fn chmod_600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(path, perms) {
            tracing::warn!("EnvStore: failed to chmod 600 on {}: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
