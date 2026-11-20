use std::path::PathBuf;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{ChatAddress, Result};

/// Verified local bytes for one portable reference owned by a Chat.
pub struct ResolvedChatMedia {
    pub path: PathBuf,
    pub mime_type: String,
}

#[async_trait]
pub trait ChatMediaLoader: Send + Sync {
    async fn open(&self, address: &ChatAddress, id: Uuid) -> Result<ResolvedChatMedia>;
}

/// Only UUID references are reserved in a tool's ordinary filesystem path argument.
pub fn reference_id(path: &str) -> Option<Uuid> {
    agent_types::content::managed_media_id(path)
}
