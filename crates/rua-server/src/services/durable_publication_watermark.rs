use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rua_core::{ChatAddress, Result, RuaError};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::conversation_manifest::ConversationManifest;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

const WATERMARK_DIR: &str = "conversation-publication-watermarks";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurablePublicationWatermark {
    pub address: ChatAddress,
    pub durable_sequence: u64,
    pub manifest_hash: String,
    pub manifest: ConversationManifest,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct DurablePublicationWatermarkStore {
    root: PathBuf,
    transaction: Arc<Mutex<()>>,
}

impl DurablePublicationWatermarkStore {
    pub fn new(sync_dir: impl AsRef<Path>) -> Result<Self> {
        let sync_dir = sync_dir.as_ref();
        let root = sync_dir.join(WATERMARK_DIR);
        migrate_legacy_watermarks(sync_dir, &root)?;
        Ok(Self {
            root,
            transaction: Arc::new(Mutex::new(())),
        })
    }

    pub fn path_for(&self, address: &ChatAddress) -> Result<PathBuf> {
        let scope = checked_path_segment(address.scope().kind(), "Chat scope")?;
        let owner = checked_path_segment(address.scope().owner_id(), "Chat owner")?;
        let chat_id = checked_path_segment(address.chat_id().as_str(), "Chat id")?;
        Ok(self
            .root
            .join(scope)
            .join(owner)
            .join(format!("{chat_id}.json")))
    }

    pub async fn load(&self, address: &ChatAddress) -> Result<Option<DurablePublicationWatermark>> {
        let _guard = self.transaction.lock().await;
        self.load_locked(address).await
    }

    pub async fn advance(
        &self,
        address: &ChatAddress,
        manifest: &ConversationManifest,
    ) -> Result<DurablePublicationWatermark> {
        if manifest.address != *address {
            return Err(RuaError::Validation {
                message: format!(
                    "Durable Publication Watermark Chat Address mismatch: expected {address}, got {}",
                    manifest.address
                ),
            });
        }

        let manifest_hash = manifest.hash()?;
        let _guard = self.transaction.lock().await;
        let previous = self.load_locked(address).await?;
        if let Some(previous) = previous {
            if manifest.latest_sequence < previous.durable_sequence {
                return Err(RuaError::Conflict {
                    resource: "durable publication watermark".to_string(),
                    detail: format!(
                        "cannot regress {} from sequence {} to {}",
                        address, previous.durable_sequence, manifest.latest_sequence
                    ),
                });
            }
            if manifest.latest_sequence == previous.durable_sequence {
                if manifest_hash == previous.manifest_hash && *manifest == previous.manifest {
                    return Ok(previous);
                }
                return Err(RuaError::Conflict {
                    resource: "durable publication watermark".to_string(),
                    detail: format!(
                        "manifest divergence for {} at durable sequence {}",
                        address, manifest.latest_sequence
                    ),
                });
            }

            let next = DurablePublicationWatermark {
                address: address.clone(),
                durable_sequence: manifest.latest_sequence,
                manifest_hash,
                manifest: manifest.clone(),
                extra: previous.extra,
            };
            self.write_locked(address, &next).await?;
            return Ok(next);
        }

        let next = DurablePublicationWatermark {
            address: address.clone(),
            durable_sequence: manifest.latest_sequence,
            manifest_hash,
            manifest: manifest.clone(),
            extra: BTreeMap::new(),
        };
        self.write_locked(address, &next).await?;
        Ok(next)
    }

    async fn load_locked(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<DurablePublicationWatermark>> {
        let path = self.path_for(address)?;
        let expected = address.clone();
        tokio::task::spawn_blocking(move || load_watermark(&path, &expected))
            .await
            .map_err(|error| RuaError::Runtime {
                message: format!("watermark load task failed: {error}"),
            })?
    }

    async fn write_locked(
        &self,
        address: &ChatAddress,
        watermark: &DurablePublicationWatermark,
    ) -> Result<()> {
        let path = self.path_for(address)?;
        let bytes = serde_json::to_vec_pretty(watermark).map_err(|error| RuaError::Io {
            message: format!("serialize Durable Publication Watermark: {error}"),
        })?;
        tokio::task::spawn_blocking(move || atomic_write(&path, &bytes))
            .await
            .map_err(|error| RuaError::Runtime {
                message: format!("watermark write task failed: {error}"),
            })??;
        Ok(())
    }
}

fn migrate_legacy_watermarks(sync_dir: &Path, root: &Path) -> Result<()> {
    let data_root = sync_dir.parent().ok_or_else(|| RuaError::Validation {
        message: format!(
            "sync directory has no parent for Durable Publication Watermark migration: {}",
            sync_dir.display()
        ),
    })?;
    let legacy_root = data_root.join(WATERMARK_DIR);
    let legacy_exists = legacy_root.try_exists()?;
    let root_exists = root.try_exists()?;
    if legacy_exists && root_exists {
        return Err(RuaError::Conflict {
            resource: "durable publication watermark storage".to_string(),
            detail: format!(
                "both legacy {} and current {} directories exist; refusing to merge",
                legacy_root.display(),
                root.display()
            ),
        });
    }
    if !legacy_exists {
        return Ok(());
    }

    fs::create_dir_all(sync_dir)?;
    fs::rename(&legacy_root, root)?;
    sync_parent(data_root)?;
    sync_parent(sync_dir)?;
    Ok(())
}

fn checked_path_segment<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(RuaError::Validation {
            message: format!("{label} is not a safe path segment"),
        });
    }
    Ok(value)
}

fn load_watermark(
    path: &Path,
    expected: &ChatAddress,
) -> Result<Option<DurablePublicationWatermark>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let watermark: DurablePublicationWatermark =
        serde_json::from_slice(&bytes).map_err(|error| RuaError::Io {
            message: format!(
                "parse Durable Publication Watermark {}: {error}",
                path.display()
            ),
        })?;
    validate_watermark(&watermark, expected)?;
    Ok(Some(watermark))
}

fn validate_watermark(
    watermark: &DurablePublicationWatermark,
    expected: &ChatAddress,
) -> Result<()> {
    if watermark.address != *expected || watermark.manifest.address != *expected {
        return Err(RuaError::Validation {
            message: format!("Durable Publication Watermark Chat Address mismatch for {expected}"),
        });
    }
    if watermark.durable_sequence != watermark.manifest.latest_sequence {
        return Err(RuaError::Validation {
            message: format!(
                "Durable Publication Watermark sequence {} does not match manifest sequence {}",
                watermark.durable_sequence, watermark.manifest.latest_sequence
            ),
        });
    }
    let actual_hash = watermark.manifest.hash()?;
    if watermark.manifest_hash != actual_hash {
        return Err(RuaError::Validation {
            message: "Durable Publication Watermark manifest hash mismatch".to_string(),
        });
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp_path = parent.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        sync_parent(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(Into::into)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}
