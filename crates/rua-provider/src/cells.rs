use std::path::{Path, PathBuf};
use std::sync::Arc;

use cell_store::FileSystemCellStore;
use dashmap::DashMap;
use llm_multiplex::LlmError;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config_loader::ProviderConfig;
use crate::manager::{create_provider_from_config, ProviderManager};
use crate::secret_field_io::{llm_config_error, read_provider_config, write_provider_config};

pub struct ProviderCells {
    #[allow(dead_code)]
    store: Arc<FileSystemCellStore>,
    provider_manager: Arc<ProviderManager>,
    base_dir: PathBuf,
    encryption_key: Option<[u8; 32]>,
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl ProviderCells {
    pub fn new(
        store: Arc<FileSystemCellStore>,
        provider_manager: Arc<ProviderManager>,
        base_dir: PathBuf,
    ) -> Self {
        Self::with_optional_key(store, provider_manager, base_dir, None)
    }

    pub fn with_key(
        store: Arc<FileSystemCellStore>,
        provider_manager: Arc<ProviderManager>,
        base_dir: PathBuf,
        key: [u8; 32],
    ) -> Self {
        Self::with_optional_key(store, provider_manager, base_dir, Some(key))
    }

    fn with_optional_key(
        store: Arc<FileSystemCellStore>,
        provider_manager: Arc<ProviderManager>,
        base_dir: PathBuf,
        encryption_key: Option<[u8; 32]>,
    ) -> Self {
        Self {
            store,
            provider_manager,
            base_dir,
            encryption_key,
            locks: DashMap::new(),
        }
    }

    pub fn cell<'a>(&'a self, provider_id: &str) -> ProviderCell<'a> {
        ProviderCell {
            cells: self,
            provider_id: provider_id.to_string(),
        }
    }

    pub async fn list(&self) -> Result<Vec<ProviderConfig>, LlmError> {
        let mut configs = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(configs),
            Err(err) => {
                return Err(LlmError::Config {
                    message: format!(
                        "failed to read providers directory {}: {err}",
                        self.base_dir.display()
                    ),
                });
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let provider_id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let lock = self.lock_for(&path);
            let _guard = lock.lock().await;
            match self.read_path(&path).await {
                Ok(Some(cfg)) => {
                    debug!(provider_id = %provider_id, "listed provider from disk");
                    configs.push(cfg);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping invalid provider config"
                    );
                }
            }
        }
        Ok(configs)
    }

    async fn read_path(&self, path: &Path) -> Result<Option<ProviderConfig>, LlmError> {
        match read_provider_config(path, self.encryption_key.as_ref()) {
            Ok(read) => {
                if read.needs_rewrite {
                    self.write_path(path, &read.config).await?;
                    debug!(
                        path = %path.display(),
                        "migrated provider api_key to encrypted field storage"
                    );
                }
                Ok(Some(read.config))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(llm_config_error("read provider config", path, err)),
        }
    }

    async fn write_path(&self, path: &Path, config: &ProviderConfig) -> Result<(), LlmError> {
        write_provider_config(path, config, self.encryption_key.as_ref())
            .map_err(|err| llm_config_error("write provider config", path, err))?;
        set_secret_file_permissions(path)
            .map_err(|err| llm_config_error("chmod provider config", path, err))?;
        Ok(())
    }

    fn lock_for(&self, path: &Path) -> Arc<Mutex<()>> {
        Arc::clone(
            self.locks
                .entry(path.to_path_buf())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .value(),
        )
    }

    fn file_for(&self, provider_id: &str) -> PathBuf {
        self.base_dir.join(format!("{provider_id}.json"))
    }
}

pub struct ProviderCell<'a> {
    cells: &'a ProviderCells,
    provider_id: String,
}

impl<'a> ProviderCell<'a> {
    pub async fn read(&self) -> Result<Option<ProviderConfig>, LlmError> {
        let path = self.cells.file_for(&self.provider_id);
        let lock = self.cells.lock_for(&path);
        let _guard = lock.lock().await;
        self.cells.read_path(&path).await
    }

    pub async fn write(&self, mut config: ProviderConfig) -> Result<(), LlmError> {
        let path = self.cells.file_for(&self.provider_id);
        let lock = self.cells.lock_for(&path);
        let _guard = lock.lock().await;
        config.apply_model_defaults();
        self.cells.write_path(&path, &config).await?;
        let provider = create_provider_from_config(&config)?;
        self.cells.provider_manager.register(provider);
        debug!(
            provider_id = %self.provider_id,
            "wrote provider config and registered runtime instance"
        );
        Ok(())
    }

    pub async fn update<F>(&self, mutator: F) -> Result<ProviderConfig, LlmError>
    where
        F: FnOnce(Option<ProviderConfig>) -> ProviderConfig + Send,
    {
        let path = self.cells.file_for(&self.provider_id);
        let lock = self.cells.lock_for(&path);
        let _guard = lock.lock().await;
        let existing = self.cells.read_path(&path).await?;
        let mut new_config = mutator(existing);
        new_config.apply_model_defaults();
        self.cells.write_path(&path, &new_config).await?;
        let provider = create_provider_from_config(&new_config)?;
        self.cells.provider_manager.register(provider);
        debug!(
            provider_id = %self.provider_id,
            "updated provider config and re-registered runtime instance"
        );
        Ok(new_config)
    }

    pub async fn delete(&self) -> Result<(), LlmError> {
        let path = self.cells.file_for(&self.provider_id);
        let lock = self.cells.lock_for(&path);
        let _guard = lock.lock().await;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(llm_config_error("delete provider config", &path, err)),
        }
        self.cells.provider_manager.remove(&self.provider_id);
        debug!(
            provider_id = %self.provider_id,
            "deleted provider config and unregistered runtime instance"
        );
        Ok(())
    }
}

fn set_secret_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
