use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::{error::CellStoreError, json_path, store::CellStore};

struct CachedFile {
    value: Arc<Value>,
    mtime: SystemTime,
}

pub struct FileSystemCellStore {
    cache: DashMap<PathBuf, CachedFile>,
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl FileSystemCellStore {
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    pub fn shared() -> Arc<Self> {
        static STORE: OnceLock<Arc<FileSystemCellStore>> = OnceLock::new();
        Arc::clone(STORE.get_or_init(|| Arc::new(FileSystemCellStore::new())))
    }

    fn lock_for(&self, file: &Path) -> Arc<Mutex<()>> {
        let entry = self
            .locks
            .entry(file.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())));
        Arc::clone(entry.value())
    }

    async fn load_value(&self, file: &Path) -> Result<Option<Arc<Value>>, CellStoreError> {
        let meta = match tokio::fs::metadata(file).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CellStoreError::io(file, e)),
        };
        let mtime = meta.modified().map_err(|e| CellStoreError::io(file, e))?;
        if let Some(cached) = self.cache.get(file) {
            if cached.mtime == mtime {
                return Ok(Some(Arc::clone(&cached.value)));
            }
        }
        let bytes = tokio::fs::read(file)
            .await
            .map_err(|e| CellStoreError::io(file, e))?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|e| CellStoreError::serde(file, e))?;
        let arc = Arc::new(value);
        self.cache.insert(
            file.to_path_buf(),
            CachedFile {
                value: Arc::clone(&arc),
                mtime,
            },
        );
        Ok(Some(arc))
    }

    async fn store_value(&self, file: &Path, value: Value) -> Result<(), CellStoreError> {
        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| CellStoreError::io(parent, e))?;
            }
        }
        let bytes =
            serde_json::to_vec_pretty(&value).map_err(|e| CellStoreError::serde(file, e))?;
        let tmp = temp_path(file);
        let mut tmp_file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await
        {
            Ok(file) => file,
            Err(e) => return Err(CellStoreError::io(&tmp, e)),
        };
        if let Err(e) = tmp_file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CellStoreError::io(&tmp, e));
        }
        if let Err(e) = tmp_file.flush().await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CellStoreError::io(&tmp, e));
        }
        if let Err(e) = tmp_file.sync_all().await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CellStoreError::io(&tmp, e));
        }
        drop(tmp_file);
        if let Err(e) = tokio::fs::rename(&tmp, file).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CellStoreError::io(file, e));
        }
        if let Some(parent) = file.parent() {
            sync_parent_dir(parent).await?;
        }
        let meta = tokio::fs::metadata(file)
            .await
            .map_err(|e| CellStoreError::io(file, e))?;
        let mtime = meta.modified().map_err(|e| CellStoreError::io(file, e))?;
        self.cache.insert(
            file.to_path_buf(),
            CachedFile {
                value: Arc::new(value),
                mtime,
            },
        );
        Ok(())
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

impl Default for FileSystemCellStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CellStore for FileSystemCellStore {
    async fn read<V>(&self, file: &Path, json_path: &str) -> Result<Option<V>, CellStoreError>
    where
        V: DeserializeOwned + Send + 'static,
    {
        let segments = json_path::parse(json_path)?;
        let arc = match self.load_value(file).await? {
            Some(v) => v,
            None => return Ok(None),
        };
        let inner = match json_path::get(&arc, &segments) {
            Some(v) => v,
            None => return Ok(None),
        };
        let v: V =
            serde_json::from_value(inner.clone()).map_err(|e| CellStoreError::serde(file, e))?;
        Ok(Some(v))
    }

    async fn write<V>(&self, file: &Path, json_path: &str, value: V) -> Result<(), CellStoreError>
    where
        V: Serialize + Send + 'static,
    {
        let segments = json_path::parse(json_path)?;
        let lock = self.lock_for(file);
        let _guard = lock.lock().await;
        let new_value = serde_json::to_value(&value).map_err(|e| CellStoreError::serde(file, e))?;
        if segments.is_empty() {
            return self.store_value(file, new_value).await;
        }
        let mut current = match self.load_value(file).await? {
            Some(arc) => (*arc).clone(),
            None => Value::Null,
        };
        json_path::set(&mut current, &segments, new_value, json_path)?;
        self.store_value(file, current).await
    }

    async fn update<V, F>(
        &self,
        file: &Path,
        json_path: &str,
        mutator: F,
    ) -> Result<V, CellStoreError>
    where
        V: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(Option<V>) -> V + Send,
    {
        let segments = json_path::parse(json_path)?;
        let lock = self.lock_for(file);
        let _guard = lock.lock().await;
        let arc_opt = self.load_value(file).await?;
        let current_v: Option<V> = match &arc_opt {
            Some(arc) => match json_path::get(arc, &segments) {
                Some(v) => Some(
                    serde_json::from_value(v.clone())
                        .map_err(|e| CellStoreError::serde(file, e))?,
                ),
                None => None,
            },
            None => None,
        };
        let new_v = mutator(current_v);
        let new_json = serde_json::to_value(&new_v).map_err(|e| CellStoreError::serde(file, e))?;
        let mut current = match arc_opt {
            Some(arc) => (*arc).clone(),
            None => Value::Null,
        };
        json_path::set(&mut current, &segments, new_json, json_path)?;
        self.store_value(file, current).await?;
        Ok(new_v)
    }

    async fn delete(&self, file: &Path, json_path: &str) -> Result<(), CellStoreError> {
        let segments = json_path::parse(json_path)?;
        let lock = self.lock_for(file);
        let _guard = lock.lock().await;
        if segments.is_empty() {
            match tokio::fs::remove_file(file).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(CellStoreError::io(file, e)),
            }
            self.cache.remove(file);
            return Ok(());
        }
        let mut current = match self.load_value(file).await? {
            Some(arc) => (*arc).clone(),
            None => return Ok(()),
        };
        let _ = json_path::remove(&mut current, &segments);
        self.store_value(file, current).await
    }
}

fn temp_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(format!(
        ".tmp.{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    file.with_file_name(name)
}

#[cfg(unix)]
async fn sync_parent_dir(parent: &Path) -> Result<(), CellStoreError> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    let parent = parent.to_path_buf();
    let error_path = parent.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| CellStoreError::io(parent, e))
    })
    .await
    .map_err(|e| CellStoreError::io(error_path, std::io::Error::other(e)))?
}

#[cfg(not(unix))]
async fn sync_parent_dir(_parent: &Path) -> Result<(), CellStoreError> {
    Ok(())
}
