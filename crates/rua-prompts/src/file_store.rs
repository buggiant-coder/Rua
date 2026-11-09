//! Shared durable Prompt file commits; Agent and Library roots remain separate.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Commit timestamp plus optional replication provenance. Local mutations use
/// increasing file timestamps; the publication adapter supplies Device identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptFileVersion {
    pub modified_ms: u64,
    #[serde(default)]
    pub counter: u32,
    #[serde(default)]
    pub device_id: Option<[u8; 8]>,
}

impl PromptFileVersion {
    pub fn local(modified_ms: u64) -> Self {
        Self {
            modified_ms,
            counter: 0,
            device_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptFileChange {
    pub path: String,
    /// None represents a deletion, including the old name of a rename.
    pub content: Option<String>,
    pub version: PromptFileVersion,
    pub deleted_before: Option<PromptFileVersion>,
}

#[derive(Default, Serialize, Deserialize)]
struct FileState {
    path: String,
    live_version: Option<PromptFileVersion>,
    live_hash: Option<String>,
    prepared_live_version: Option<PromptFileVersion>,
    prepared_live_hash: Option<String>,
    deletion_version: Option<PromptFileVersion>,
    live_deleted_before: Option<PromptFileVersion>,
    prepared_deleted_before: Option<PromptFileVersion>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct PromptFileStore {
    files: PathBuf,
    state_dir: PathBuf,
    agent_scope: bool,
}

pub struct PromptFileGuard {
    files: PathBuf,
    state_dir: PathBuf,
    agent_scope: bool,
    _guard: OwnedMutexGuard<()>,
}

type Locks = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;
static LOCKS: OnceLock<Locks> = OnceLock::new();

impl PromptFileStore {
    pub fn for_library(data_dir: impl Into<PathBuf>) -> Self {
        let root = data_dir.into();
        Self {
            files: root.join("prompt_library"),
            state_dir: root.join(".prompt-library-state"),
            agent_scope: false,
        }
    }

    pub(crate) fn for_agent(workspace: impl Into<PathBuf>) -> Self {
        let root = workspace.into();
        Self {
            files: root.join("prompts"),
            state_dir: root.join(".prompt-file-state"),
            agent_scope: true,
        }
    }

    pub async fn lock(&self) -> PromptFileGuard {
        let key = lock_key(&self.files);
        let lock = {
            let mut locks = LOCKS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            locks.retain(|_, value| value.strong_count() > 0);
            match locks.get(&key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(AsyncMutex::new(()));
                    locks.insert(key, Arc::downgrade(&lock));
                    lock
                }
            }
        };
        PromptFileGuard {
            files: self.files.clone(),
            state_dir: self.state_dir.clone(),
            agent_scope: self.agent_scope,
            _guard: lock.lock_owned().await,
        }
    }
}

impl PromptFileGuard {
    pub fn file_path(&self, relative: &str) -> io::Result<PathBuf> {
        let parts: Vec<_> = Path::new(relative).components().collect();
        if relative.contains('\\')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || parts.is_empty()
            || (self.agent_scope && parts.len() > 2)
            || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
            || if self.agent_scope {
                !relative.ends_with(".md")
            } else {
                !relative.to_ascii_lowercase().ends_with(".md")
            }
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid Prompt file path",
            ));
        }
        Ok(self.files.join(relative))
    }

    fn state_path(&self, relative: &str) -> PathBuf {
        let key = format!("{:x}", Sha256::digest(relative.as_bytes()));
        self.state_dir.join(format!("{key}.json"))
    }

    fn state(&self, relative: &str) -> io::Result<FileState> {
        self.file_path(relative)?;
        match fs::read(self.state_path(relative)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(FileState {
                path: relative.into(),
                ..Default::default()
            }),
            Err(e) => Err(e),
        }
    }

    fn save_state(&self, state: &FileState) -> io::Result<()> {
        rua_config::migration::atomic::atomic_write(
            &self.state_path(&state.path),
            &serde_json::to_vec(state)?,
        )?;
        sync_dir(self.state_dir.parent().expect("Prompt state parent"))
    }

    /// Only matching on-disk bytes and timestamp commit a prepared live version.
    /// Both the old and prepared timestamp survive a failed atomic replacement.
    pub fn local_revision(&self, relative: &str) -> io::Result<Option<(PromptFileVersion, bool)>> {
        let state = self.state(relative)?;
        match fs::metadata(self.file_path(relative)?) {
            Ok(metadata) => Ok(self
                .matching_live(relative, &state, &metadata)?
                .map(|(version, _)| (version, false))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(state.deletion_version.map(|ms| (ms, true)))
            }
            Err(e) => Err(e),
        }
    }

    fn matching_live(
        &self,
        relative: &str,
        state: &FileState,
        metadata: &fs::Metadata,
    ) -> io::Result<Option<(PromptFileVersion, Option<PromptFileVersion>)>> {
        let ms = modified_ms(metadata)?;
        let hash = format!("{:x}", Sha256::digest(fs::read(self.file_path(relative)?)?));
        Ok([
            (
                state.prepared_live_version,
                &state.prepared_live_hash,
                state.prepared_deleted_before,
            ),
            (
                state.live_version,
                &state.live_hash,
                state.live_deleted_before,
            ),
        ]
        .into_iter()
        .find_map(|(version, expected_hash, fence)| {
            version
                .filter(|version| {
                    version.modified_ms == ms && expected_hash.as_deref() == Some(&hash)
                })
                .map(|version| (version, fence))
        }))
    }

    /// Last committed deletion carried by the currently installed live version.
    pub fn deletion_fence(&self, relative: &str) -> io::Result<Option<PromptFileVersion>> {
        let state = self.state(relative)?;
        match fs::metadata(self.file_path(relative)?) {
            Ok(metadata) => Ok(self
                .matching_live(relative, &state, &metadata)?
                .and_then(|(_, fence)| fence)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn next_ms(&self, paths: &[&str]) -> io::Result<u64> {
        let mut latest = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis() as u64;
        for path in paths {
            let state = self.state(path)?;
            for ms in [
                state.live_version,
                state.prepared_live_version,
                state.deletion_version,
            ]
            .into_iter()
            .flatten()
            {
                latest = latest.max(ms.modified_ms.saturating_add(1));
            }
            match fs::metadata(self.file_path(path)?) {
                Ok(meta) => latest = latest.max(modified_ms(&meta)?.saturating_add(1)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(latest)
    }

    fn prepare_live(
        &self,
        relative: &str,
        content: &str,
        version: PromptFileVersion,
        deleted_before: Option<PromptFileVersion>,
    ) -> io::Result<()> {
        let revision = self.local_revision(relative)?;
        let current = revision
            .filter(|(_, deleted)| !deleted)
            .map(|(version, _)| version);
        let mut state = self.state(relative)?;
        state.live_deleted_before = self.deletion_fence(relative)?;
        state.prepared_deleted_before = deleted_before
            .or_else(|| {
                revision
                    .filter(|(_, deleted)| *deleted)
                    .map(|(version, _)| version)
            })
            .or(state.live_deleted_before);
        state.live_hash = if current.is_some() {
            Some(format!(
                "{:x}",
                Sha256::digest(fs::read(self.file_path(relative)?)?)
            ))
        } else {
            None
        };
        state.live_version = current;
        state.prepared_live_hash = Some(format!("{:x}", Sha256::digest(content.as_bytes())));
        state.prepared_live_version = Some(version);
        self.save_state(&state)
    }

    fn prepare_delete(&self, relative: &str, version: PromptFileVersion) -> io::Result<()> {
        let mut state = self.state(relative)?;
        state.deletion_version = Some(version);
        self.save_state(&state)
    }

    pub fn write(
        &self,
        relative: &str,
        content: &str,
        create_only: bool,
    ) -> io::Result<PromptFileChange> {
        let path = self.file_path(relative)?;
        if create_only && path.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Prompt file already exists",
            ));
        }
        let ms = self.next_ms(&[relative])?;
        self.apply_write(relative, content, PromptFileVersion::local(ms), None)?;
        Ok(PromptFileChange {
            path: relative.into(),
            content: Some(content.into()),
            version: PromptFileVersion::local(ms),
            deleted_before: self.deletion_fence(relative)?,
        })
    }

    /// Canonical inbound persistence; deliberately does not publish a local fact.
    /// The adapter validates and merges any incoming deletion fence with local history.
    pub fn apply_write(
        &self,
        relative: &str,
        content: &str,
        version: PromptFileVersion,
        deleted_before: Option<PromptFileVersion>,
    ) -> io::Result<()> {
        let path = self.file_path(relative)?;
        self.prepare_live(relative, content, version, deleted_before)?;
        write_atomic_at(&path, content.as_bytes(), version.modified_ms)?;
        let mut parent = path.parent();
        while let Some(dir) = parent {
            sync_dir(dir)?;
            if dir == self.files {
                break;
            }
            parent = dir.parent();
        }
        sync_dir(self.state_dir.parent().expect("Prompt state parent"))
    }

    pub fn delete(&self, relative: &str) -> io::Result<PromptFileChange> {
        fs::metadata(self.file_path(relative)?)?;
        let ms = self.next_ms(&[relative])?;
        self.apply_delete(relative, PromptFileVersion::local(ms))?;
        Ok(PromptFileChange {
            path: relative.into(),
            content: None,
            version: PromptFileVersion::local(ms),
            deleted_before: None,
        })
    }

    pub fn apply_delete(&self, relative: &str, version: PromptFileVersion) -> io::Result<()> {
        let path = self.file_path(relative)?;
        self.prepare_delete(relative, version)?;
        match fs::remove_file(&path) {
            Ok(()) => sync_dir(path.parent().expect("Prompt parent")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn rename(&self, old: &str, new: &str) -> io::Result<Vec<PromptFileChange>> {
        let old_path = self.file_path(old)?;
        let new_path = self.file_path(new)?;
        let content = fs::read_to_string(&old_path)?;
        if new_path.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Prompt file already exists",
            ));
        }
        let ms = self.next_ms(&[old, new])?;
        self.prepare_delete(old, PromptFileVersion::local(ms))?;
        self.prepare_live(old, &content, PromptFileVersion::local(ms), None)?;
        self.prepare_live(new, &content, PromptFileVersion::local(ms), None)?;
        // Rename retains timestamps. Stamp before the atomic move so a crash
        // after the move still identifies the destination's committed version.
        let file = File::options().write(true).open(&old_path)?;
        file.set_modified(UNIX_EPOCH + Duration::from_millis(ms))?;
        file.sync_all()?;
        fs::rename(&old_path, &new_path)?;
        sync_dir(new_path.parent().expect("Prompt parent"))?;
        if old_path.parent() != new_path.parent() {
            sync_dir(old_path.parent().expect("Prompt parent"))?;
        }
        Ok(vec![
            PromptFileChange {
                path: old.into(),
                content: None,
                version: PromptFileVersion::local(ms),
                deleted_before: None,
            },
            PromptFileChange {
                path: new.into(),
                content: Some(content),
                version: PromptFileVersion::local(ms),
                deleted_before: self.deletion_fence(new)?,
            },
        ])
    }

    pub fn deletions(&self) -> io::Result<Vec<PromptFileChange>> {
        let entries = match fs::read_dir(&self.state_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut deleted = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let state: FileState = serde_json::from_slice(&fs::read(entry.path())?)?;
            if let Some((ms, true)) = self.local_revision(&state.path)? {
                deleted.push(PromptFileChange {
                    path: state.path,
                    content: None,
                    version: ms,
                    deleted_before: None,
                });
            }
        }
        Ok(deleted)
    }
}

fn modified_ms(meta: &fs::Metadata) -> io::Result<u64> {
    Ok(meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis() as u64)
}

fn write_atomic_at(path: &Path, data: &[u8], ms: u64) -> io::Result<()> {
    let parent = path.parent().expect("Prompt parent");
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(data)?;
    temp.as_file()
        .set_modified(UNIX_EPOCH + Duration::from_millis(ms))?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    sync_dir(parent)
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn lock_key(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => lock_key(parent).join(name),
        _ => std::env::current_dir().unwrap_or_default().join(path),
    }
}
