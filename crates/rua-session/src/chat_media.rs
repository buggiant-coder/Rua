use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::display_media::MediaOrigin;

pub const MAX_CHAT_MEDIA_BYTES: usize = 50 * 1024 * 1024;

// Sync apply, uploads and on-demand reads may register the same object concurrently.
static REGISTRATION: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMediaManifest {
    pub media_ref: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub sha256: String,
    pub chat_ids: BTreeSet<String>,
    pub device_ids: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<MediaOrigin>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatMediaManifest {
    pub fn new(
        id: Uuid,
        chat_id: &str,
        device_id: &str,
        mime_type: String,
        bytes: &[u8],
    ) -> io::Result<Self> {
        let manifest = Self {
            media_ref: format!("media/{id}"),
            mime_type,
            byte_size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            chat_ids: BTreeSet::from([chat_id.to_owned()]),
            device_ids: BTreeSet::from([device_id.to_owned()]),
            original_filename: None,
            origin: None,
            extra: Default::default(),
        };
        manifest.id()?;
        Ok(manifest)
    }

    pub fn id(&self) -> io::Result<Uuid> {
        let id = self
            .media_ref
            .strip_prefix("media/")
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| invalid("Invalid Chat Media reference"))?;
        if self.media_ref != format!("media/{id}")
            || self.byte_size == 0
            || self.byte_size > MAX_CHAT_MEDIA_BYTES as u64
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || self.mime_type.trim().is_empty()
            || self.chat_ids.is_empty()
            || self.chat_ids.iter().any(|s| s.trim().is_empty())
            || self.device_ids.is_empty()
            || self
                .device_ids
                .iter()
                .any(|s| s.trim().is_empty() || s == "unknown")
        {
            return Err(invalid("Incomplete or invalid Chat Media metadata"));
        }
        Ok(id)
    }

    pub fn load(media_dir: &Path, id: Uuid) -> io::Result<Option<Self>> {
        let path = media_dir.join(format!(".{id}.media.json"));
        match fs::symlink_metadata(&path) {
            Ok(m) if !m.is_file() || m.is_symlink() => {
                return Err(invalid("Invalid Chat Media metadata file"))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            _ => {}
        }
        let item: Self = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
        if item.id()? != id {
            return Err(invalid("Chat Media metadata reference mismatch"));
        }
        Ok(Some(item))
    }

    pub fn load_reference(chat_dir: &Path, id: Uuid) -> io::Result<Option<Self>> {
        Self::load(&chat_dir.join("media_refs"), id)
    }

    pub fn record_reference(&self, chat_dir: &Path, chat_id: &str) -> io::Result<()> {
        if !self.chat_ids.contains(chat_id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Chat does not own this Media reference",
            ));
        }
        let mut reference = self.clone();
        reference.chat_ids = BTreeSet::from([chat_id.to_owned()]);
        reference.register(&chat_dir.join("media_refs"))
    }

    fn merged(&self, previous: Option<Self>) -> io::Result<Self> {
        let Some(mut existing) = previous else {
            return Ok(self.clone());
        };
        if self.sha256 != existing.sha256
            || self.byte_size != existing.byte_size
            || self.mime_type != existing.mime_type
            || self.original_filename != existing.original_filename
            || self.origin != existing.origin
        {
            return Err(invalid(
                "Chat Media identity conflicts with existing metadata",
            ));
        }
        existing.chat_ids.extend(self.chat_ids.iter().cloned());
        existing.device_ids.extend(self.device_ids.iter().cloned());
        existing.extra.extend(self.extra.clone());
        Ok(existing)
    }

    /// Register authorized references and known locations without opening or transferring bytes.
    pub fn register(&self, media_dir: &Path) -> io::Result<()> {
        let id = self.id()?;
        let _guard = REGISTRATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let existing = Self::load(media_dir, id)?;
        let merged = self.merged(existing.clone())?;
        if existing.as_ref() != Some(&merged) {
            merged.write_metadata(media_dir, id)?;
        }
        Ok(())
    }

    /// Publish metadata only after the verified bytes have reached stable storage.
    pub fn store_copy(&self, media_dir: &Path, bytes: &[u8]) -> io::Result<()> {
        let id = self.id()?;
        if self.byte_size != bytes.len() as u64
            || format!("{:x}", Sha256::digest(bytes)) != self.sha256
        {
            return Err(invalid("Chat Media content failed integrity verification"));
        }
        let _guard = REGISTRATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let existing = Self::load(media_dir, id)?;
        let merged = self.merged(existing.clone())?;
        let path = media_dir.join(id.to_string());
        rua_config::atomic_write(&path, bytes)?;
        if existing.as_ref() != Some(&merged) {
            if let Err(error) = merged.write_metadata(media_dir, id) {
                if existing.is_none() && Self::load(media_dir, id).is_ok_and(|item| item.is_none())
                {
                    fs::remove_file(&path)?;
                    rua_core::fs::sync_directory(media_dir)?;
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn write_metadata(&self, media_dir: &Path, id: Uuid) -> io::Result<()> {
        rua_config::atomic_write(
            &media_dir.join(format!(".{id}.media.json")),
            &serde_json::to_vec(self)?,
        )
    }

    pub fn verify_copy(&self, media_dir: &Path) -> io::Result<()> {
        use std::io::Read;
        let path = media_dir.join(self.id()?.to_string());
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.is_symlink() || meta.len() != self.byte_size {
            return Err(invalid("Chat Media content failed integrity verification"));
        }
        let mut digest = Sha256::new();
        let count = io::copy(
            &mut fs::File::open(path)?.take(self.byte_size + 1),
            &mut digest,
        )?;
        if count != self.byte_size || format!("{:x}", digest.finalize()) != self.sha256 {
            return Err(invalid("Chat Media content failed integrity verification"));
        }
        Ok(())
    }
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}

/// Only mentions of portable media are indexed. Ordinary paths and message text stay untouched.
pub fn referenced_ids(value: &serde_json::Value, ids: &mut BTreeSet<Uuid>) {
    match value {
        serde_json::Value::Array(values) => {
            for part in values {
                referenced_ids(part, ids);
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if key == "text" {
                    if let Some(text) = value.as_str().filter(|text| text.contains("media/")) {
                        for reference in crate::display_media::managed_image_refs(text) {
                            if let Some(id) = agent_types::content::managed_media_id(&reference) {
                                ids.insert(id);
                            }
                        }
                    }
                } else {
                    referenced_ids(value, ids);
                }
            }
        }
        serde_json::Value::String(value) => {
            if let Some(id) = agent_types::content::managed_media_id(value) {
                ids.insert(id);
            }
        }
        _ => {}
    }
}
