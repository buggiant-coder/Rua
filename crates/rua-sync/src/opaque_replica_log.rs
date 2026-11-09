use bytes::Bytes;
use iroh::EndpointId;
use iroh_blobs::Hash;
use iroh_docs::api::Doc;
pub use iroh_docs::AuthorId;
use iroh_docs::{Author, Entry};
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

use crate::crypto::{self, SyncKey};
use crate::endpoint::BlobsApiStore;
use crate::hlc::HlcTimestamp;
use crate::stage_diagnostics::{wall_now_ms, SyncSourceTiming};
use crate::sync_blob_store::StatefulBlobDownloader;
use crate::SyncError;
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::api::TempTag;

pub const SYNC_CHANGE_SCHEMA_VERSION: u16 = 1;
const REPLICA_LOG_BLOB_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const ENCRYPTED_BLOB_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);
const TEMP_ENCRYPTED_BLOB_RETENTION_DEFAULT_SECS: u64 = 300;

static TEMP_ENCRYPTED_BLOBS: LazyLock<Mutex<Vec<RetainedTempBlob>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

struct RetainedTempBlob {
    expires_at: Instant,
    _tag: TempTag,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeBase {
    pub hlc: HlcTimestamp,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncChange {
    pub schema_version: u16,
    pub op_id: String,
    pub domain: String,
    pub record_id: String,
    pub kind: String,
    pub device_id: String,
    pub hlc: HlcTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ChangeBase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_timing: Option<SyncSourceTiming>,
    pub payload: serde_json::Value,
}

impl SyncChange {
    pub fn new(
        op_id: impl Into<String>,
        domain: impl Into<String>,
        record_id: impl Into<String>,
        kind: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            schema_version: SYNC_CHANGE_SCHEMA_VERSION,
            op_id: op_id.into(),
            domain: domain.into(),
            record_id: record_id.into(),
            kind: kind.into(),
            device_id: device_id.into(),
            hlc,
            base: None,
            source_timing: None,
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncReplicaLogFrame {
    Single(SyncChange),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaLogReadDiagnostics {
    pub content_ready_at_ms: u64,
    pub decrypt_read_completed_at_ms: u64,
    pub content_wait_ms: u64,
    pub content_read_ms: u64,
    pub decrypt_ms: u64,
    pub used_peer_download: bool,
}

impl SyncReplicaLogFrame {
    pub fn into_changes(self) -> Vec<SyncChange> {
        match self {
            Self::Single(change) => vec![change],
        }
    }
}

pub struct OpaqueReplicaLog {
    doc: Doc,
    author: Author,
    sync_key: SyncKey,
    blobs_store: BlobsApiStore,
}

impl OpaqueReplicaLog {
    pub fn new(doc: Doc, author: Author, sync_key: SyncKey, blobs_store: BlobsApiStore) -> Self {
        Self {
            doc,
            author,
            sync_key,
            blobs_store,
        }
    }

    pub fn record_entry_key(&self, domain: &str, record_id: &str) -> Vec<u8> {
        crypto::hmac_key(&self.sync_key, &format!("{domain}\0{record_id}"))
    }

    pub fn blob_context(&self) -> (BlobsApiStore, crypto::SyncKey) {
        (self.blobs_store.clone(), self.sync_key.clone())
    }

    pub async fn put_encrypted_blob(&self, plaintext: &[u8]) -> Result<String, SyncError> {
        put_encrypted_blob(&self.blobs_store, &self.sync_key, plaintext).await
    }

    pub async fn read_encrypted_blob(&self, hash_hex: &str) -> Result<Option<Vec<u8>>, SyncError> {
        read_encrypted_blob(&self.blobs_store, &self.sync_key, hash_hex).await
    }

    pub async fn append_change(&self, change: &SyncChange) -> Result<Vec<u8>, SyncError> {
        validate_change_schema(change)?;

        let key = self.record_entry_key(&change.domain, &change.record_id);
        self.append_plaintext_with_key(key.clone(), &serde_json::to_vec(change)?)
            .await?;
        Ok(key)
    }

    pub async fn append_change_batch(
        &self,
        _batch_id: &str,
        _lane: Option<&str>,
        changes: &[SyncChange],
    ) -> Result<Vec<u8>, SyncError> {
        let mut last_key = Vec::new();
        for change in changes {
            last_key = self.append_change(change).await?;
        }
        Ok(last_key)
    }

    async fn append_plaintext_with_key(
        &self,
        key: Vec<u8>,
        plaintext: &[u8],
    ) -> Result<(), SyncError> {
        let encrypted = crypto::symmetric_encrypt(plaintext, &self.sync_key)?;
        let size = encrypted.len() as u64;
        let hash =
            store_encrypted_blob_bytes(&self.blobs_store, Bytes::copy_from_slice(&encrypted))
                .await?;
        self.doc
            .set_hash(self.author.id(), Bytes::from(key), hash, size)
            .await
            .map_err(|e| SyncError::Store(format!("replica log set_hash: {e}")))?;
        Ok(())
    }

    pub async fn read_change(
        &self,
        author: AuthorId,
        key: &[u8],
    ) -> Result<Option<SyncChange>, SyncError> {
        let entry = self
            .doc
            .get_exact(author, key, false)
            .await
            .map_err(|e| SyncError::Store(e.to_string()))?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        let frame = self.read_entry_frame(&entry).await?;
        match frame {
            SyncReplicaLogFrame::Single(change) => Ok(Some(change)),
        }
    }

    pub async fn read_change_with_peer_download(
        &self,
        author: AuthorId,
        key: &[u8],
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<Option<SyncChange>, SyncError> {
        let entry = self
            .doc
            .get_exact(author, key, false)
            .await
            .map_err(|e| SyncError::Store(e.to_string()))?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        match self.read_entry_frame(&entry).await {
            Ok(SyncReplicaLogFrame::Single(change)) => Ok(Some(change)),
            Err(first_error) if is_blob_read_error(&first_error) && !providers.is_empty() => {
                if let Err(download_error) = self
                    .download_blob_from_peers(entry.content_hash(), downloader, providers)
                    .await
                {
                    return Err(SyncError::ReplicaLogContentUnavailable(format!(
                        "{first_error}; active blob download failed: {download_error}"
                    )));
                }
                match self.read_entry_frame(&entry).await? {
                    SyncReplicaLogFrame::Single(change) => Ok(Some(change)),
                }
            }
            Err(e) => Err(e),
        }
    }

    pub async fn read_frame(
        &self,
        author: AuthorId,
        key: &[u8],
    ) -> Result<Option<SyncReplicaLogFrame>, SyncError> {
        Ok(self
            .read_frame_with_diagnostics(author, key)
            .await?
            .map(|(frame, _)| frame))
    }

    pub async fn read_frame_with_diagnostics(
        &self,
        author: AuthorId,
        key: &[u8],
    ) -> Result<Option<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics)>, SyncError> {
        let entry = self
            .doc
            .get_exact(author, key, false)
            .await
            .map_err(|e| SyncError::Store(e.to_string()))?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        Ok(Some(
            self.read_frame_hash_with_diagnostics(entry.content_hash())
                .await?,
        ))
    }

    pub async fn read_frame_by_content_hash_with_diagnostics(
        &self,
        content_hash: &str,
    ) -> Result<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics), SyncError> {
        self.read_frame_hash_with_diagnostics(parse_hash(content_hash)?)
            .await
    }

    async fn read_frame_hash_with_diagnostics(
        &self,
        content_hash: Hash,
    ) -> Result<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics), SyncError> {
        if !matches!(
            self.blobs_store.status(content_hash).await,
            Ok(BlobStatus::Complete { .. })
        ) {
            return Err(SyncError::ReplicaLogContentUnavailable(
                "replica log content is incomplete and no active download is available".to_string(),
            ));
        }
        let content_ready_at_ms = wall_now_ms();
        let (frame, content_read_ms, decrypt_ms) = self
            .read_content_hash_with_diagnostics(content_hash)
            .await?;
        Ok((
            frame,
            ReplicaLogReadDiagnostics {
                content_ready_at_ms,
                decrypt_read_completed_at_ms: wall_now_ms(),
                content_wait_ms: 0,
                content_read_ms,
                decrypt_ms,
                used_peer_download: false,
            },
        ))
    }

    pub async fn read_frame_with_peer_download(
        &self,
        author: AuthorId,
        key: &[u8],
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<Option<SyncReplicaLogFrame>, SyncError> {
        Ok(self
            .read_frame_with_peer_download_diagnostics(author, key, downloader, providers)
            .await?
            .map(|(frame, _)| frame))
    }

    pub async fn read_frame_with_peer_download_diagnostics(
        &self,
        author: AuthorId,
        key: &[u8],
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<Option<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics)>, SyncError> {
        let entry = self
            .doc
            .get_exact(author, key, false)
            .await
            .map_err(|e| SyncError::Store(e.to_string()))?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        Ok(Some(
            self.read_frame_hash_with_peer_download_diagnostics(
                entry.content_hash(),
                downloader,
                providers,
            )
            .await?,
        ))
    }

    pub async fn read_frame_by_content_hash_with_peer_download_diagnostics(
        &self,
        content_hash: &str,
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics), SyncError> {
        self.read_frame_hash_with_peer_download_diagnostics(
            parse_hash(content_hash)?,
            downloader,
            providers,
        )
        .await
    }

    async fn read_frame_hash_with_peer_download_diagnostics(
        &self,
        content_hash: Hash,
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<(SyncReplicaLogFrame, ReplicaLogReadDiagnostics), SyncError> {
        let initial_content_ready_at_ms = wall_now_ms();
        let local_complete = matches!(
            self.blobs_store.status(content_hash).await,
            Ok(BlobStatus::Complete { .. })
        );
        if !local_complete {
            if providers.is_empty() {
                return Err(SyncError::ReplicaLogContentUnavailable(
                    "replica log content is incomplete and no providers are available".to_string(),
                ));
            }
            let content_wait_started = Instant::now();
            self.download_blob_from_peers(content_hash, downloader, providers)
                .await
                .map_err(|download_error| {
                    SyncError::ReplicaLogContentUnavailable(format!(
                        "replica log content is incomplete; active blob download failed: {download_error}"
                    ))
                })?;
            let content_wait_ms = crate::stage_diagnostics::millis(content_wait_started.elapsed());
            let content_ready_at_ms = wall_now_ms();
            let (frame, content_read_ms, decrypt_ms) = self
                .read_content_hash_with_diagnostics(content_hash)
                .await?;
            return Ok((
                frame,
                ReplicaLogReadDiagnostics {
                    content_ready_at_ms,
                    decrypt_read_completed_at_ms: wall_now_ms(),
                    content_wait_ms,
                    content_read_ms,
                    decrypt_ms,
                    used_peer_download: true,
                },
            ));
        }

        match self.read_content_hash_with_diagnostics(content_hash).await {
            Ok((frame, content_read_ms, decrypt_ms)) => Ok((
                frame,
                ReplicaLogReadDiagnostics {
                    content_ready_at_ms: initial_content_ready_at_ms,
                    decrypt_read_completed_at_ms: wall_now_ms(),
                    content_wait_ms: 0,
                    content_read_ms,
                    decrypt_ms,
                    used_peer_download: false,
                },
            )),
            Err(first_error) if is_blob_read_error(&first_error) && !providers.is_empty() => {
                let content_wait_started = Instant::now();
                if let Err(download_error) = self
                    .download_blob_from_peers(content_hash, downloader, providers)
                    .await
                {
                    return Err(SyncError::ReplicaLogContentUnavailable(format!(
                        "{first_error}; active blob download failed: {download_error}"
                    )));
                }
                let content_wait_ms =
                    crate::stage_diagnostics::millis(content_wait_started.elapsed());
                let content_ready_at_ms = wall_now_ms();
                let (frame, content_read_ms, decrypt_ms) = self
                    .read_content_hash_with_diagnostics(content_hash)
                    .await?;
                Ok((
                    frame,
                    ReplicaLogReadDiagnostics {
                        content_ready_at_ms,
                        decrypt_read_completed_at_ms: wall_now_ms(),
                        content_wait_ms,
                        content_read_ms,
                        decrypt_ms,
                        used_peer_download: true,
                    },
                ))
            }
            Err(e) => Err(e),
        }
    }

    async fn read_entry_frame(&self, entry: &Entry) -> Result<SyncReplicaLogFrame, SyncError> {
        self.read_entry_frame_with_diagnostics(entry)
            .await
            .map(|(frame, _, _)| frame)
    }

    async fn read_entry_frame_with_diagnostics(
        &self,
        entry: &Entry,
    ) -> Result<(SyncReplicaLogFrame, u64, u64), SyncError> {
        self.read_content_hash_with_diagnostics(entry.content_hash())
            .await
    }

    async fn read_content_hash_with_diagnostics(
        &self,
        content_hash: Hash,
    ) -> Result<(SyncReplicaLogFrame, u64, u64), SyncError> {
        let content_read_started = Instant::now();
        let mut reader = self.blobs_store.reader(content_hash);
        let mut raw = Vec::new();
        reader
            .read_to_end(&mut raw)
            .await
            .map_err(|e| SyncError::Store(format!("read replica log blob: {e}")))?;
        let content_read_ms = crate::stage_diagnostics::millis(content_read_started.elapsed());
        let decrypt_started = Instant::now();
        let decrypted = crypto::symmetric_decrypt(&raw, &self.sync_key)?;
        let frame = decode_replica_log_frame(&decrypted)?;
        let decrypt_ms = crate::stage_diagnostics::millis(decrypt_started.elapsed());
        Ok((frame, content_read_ms, decrypt_ms))
    }

    async fn download_blob_from_peers(
        &self,
        hash: Hash,
        downloader: &StatefulBlobDownloader,
        providers: Vec<EndpointId>,
    ) -> Result<(), SyncError> {
        download_hash_from_peers(
            &self.blobs_store,
            downloader,
            hash,
            providers,
            REPLICA_LOG_BLOB_DOWNLOAD_TIMEOUT,
            "replica log blob",
        )
        .await
    }
}

fn validate_change_schema(change: &SyncChange) -> Result<(), SyncError> {
    if change.schema_version != SYNC_CHANGE_SCHEMA_VERSION {
        return Err(SyncError::Store(format!(
            "unsupported sync change schema version: {}",
            change.schema_version
        )));
    }
    Ok(())
}

fn decode_replica_log_frame(decrypted: &[u8]) -> Result<SyncReplicaLogFrame, SyncError> {
    let change: SyncChange = serde_json::from_slice(decrypted)?;
    validate_change_schema(&change)?;
    Ok(SyncReplicaLogFrame::Single(change))
}

pub fn should_download_replica_log_content_after_read_error(error: &SyncError) -> bool {
    matches!(error, SyncError::Store(message) if message.starts_with("read replica log blob:"))
        || matches!(error, SyncError::Crypto(message) if message == "ciphertext too short")
}

fn is_blob_read_error(error: &SyncError) -> bool {
    should_download_replica_log_content_after_read_error(error)
}

pub async fn put_encrypted_blob(
    blobs_store: &BlobsApiStore,
    sync_key: &crypto::SyncKey,
    plaintext: &[u8],
) -> Result<String, SyncError> {
    let encrypted = crypto::symmetric_encrypt(plaintext, sync_key)?;
    Ok(
        store_encrypted_blob_bytes(blobs_store, Bytes::from(encrypted))
            .await?
            .to_hex(),
    )
}

pub async fn store_encrypted_blob_bytes(
    blobs_store: &BlobsApiStore,
    encrypted: Bytes,
) -> Result<Hash, SyncError> {
    let temp_tag = blobs_store
        .add_bytes(encrypted)
        .temp_tag()
        .await
        .map_err(|e| SyncError::Store(format!("add encrypted blob: {e}")))?;
    let hash = temp_tag.hash();
    retain_temp_encrypted_blob(temp_tag);
    Ok(hash)
}

fn retain_temp_encrypted_blob(temp_tag: TempTag) {
    let retention = temp_encrypted_blob_retention();
    let expires_at = Instant::now() + retention;
    let mut retained = TEMP_ENCRYPTED_BLOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    retained.retain(|entry| entry.expires_at > now);
    retained.push(RetainedTempBlob {
        expires_at,
        _tag: temp_tag,
    });
    tokio::spawn(async move {
        tokio::time::sleep(retention).await;
        prune_expired_temp_encrypted_blobs();
    });
}

fn temp_encrypted_blob_retention() -> Duration {
    let secs = std::env::var("RUA_SYNC_TEMP_BLOB_RETENTION_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(TEMP_ENCRYPTED_BLOB_RETENTION_DEFAULT_SECS);
    Duration::from_secs(secs)
}

fn prune_expired_temp_encrypted_blobs() {
    let mut retained = TEMP_ENCRYPTED_BLOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    retained.retain(|entry| entry.expires_at > now);
}

pub async fn read_encrypted_blob(
    blobs_store: &BlobsApiStore,
    sync_key: &crypto::SyncKey,
    hash_hex: &str,
) -> Result<Option<Vec<u8>>, SyncError> {
    let hash = parse_hash(hash_hex)?;
    read_encrypted_blob_hash(blobs_store, sync_key, hash).await
}

pub async fn read_encrypted_blob_with_peer_download(
    blobs_store: &BlobsApiStore,
    sync_key: &crypto::SyncKey,
    hash_hex: &str,
    downloader: &StatefulBlobDownloader,
    providers: Vec<EndpointId>,
) -> Result<Option<Vec<u8>>, SyncError> {
    let hash = parse_hash(hash_hex)?;
    if let Some(content) = read_encrypted_blob_hash(blobs_store, sync_key, hash).await? {
        return Ok(Some(content));
    }
    if providers.is_empty() {
        return Ok(None);
    }
    download_hash_from_peers(
        blobs_store,
        downloader,
        hash,
        providers,
        ENCRYPTED_BLOB_DOWNLOAD_TIMEOUT,
        "encrypted blob",
    )
    .await?;
    read_encrypted_blob_hash(blobs_store, sync_key, hash).await
}

async fn read_encrypted_blob_hash(
    blobs_store: &BlobsApiStore,
    sync_key: &crypto::SyncKey,
    hash: Hash,
) -> Result<Option<Vec<u8>>, SyncError> {
    let mut reader = blobs_store.reader(hash);
    let mut raw = Vec::new();
    match reader.read_to_end(&mut raw).await {
        Ok(_) => {}
        Err(e) => {
            tracing::debug!(hash = %hash, error = %e, "[sync-v4] encrypted blob not readable");
            return Ok(None);
        }
    }
    let decrypted = crypto::symmetric_decrypt(&raw, sync_key)?;
    Ok(Some(decrypted))
}

async fn download_hash_from_peers(
    blobs_store: &BlobsApiStore,
    downloader: &StatefulBlobDownloader,
    hash: Hash,
    providers: Vec<EndpointId>,
    timeout: Duration,
    label: &str,
) -> Result<(), SyncError> {
    let before = blob_status_label(blobs_store, hash).await;
    match downloader.download(hash, providers, timeout, label).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let after = blob_status_label(blobs_store, hash).await;
            if is_blob_complete_label(&after) {
                tracing::debug!(
                    %hash,
                    before,
                    after,
                    error = %error,
                    "[sync-v4] blob download reported error after local completion"
                );
                return Ok(());
            }
            Err(SyncError::Store(format!(
                "download {label} {hash}: {error}; local status {before} -> {after}"
            )))
        }
    }
}

fn is_blob_complete_label(status: &str) -> bool {
    status.starts_with("complete(")
}

async fn blob_status_label(blobs_store: &BlobsApiStore, hash: Hash) -> String {
    match blobs_store.status(hash).await {
        Ok(BlobStatus::NotFound) => "not_found".to_string(),
        Ok(BlobStatus::Partial { size }) => format!("partial({})", size_label(size)),
        Ok(BlobStatus::Complete { size }) => format!("complete({size})"),
        Err(error) => format!("status_error({error})"),
    }
}

fn size_label(size: Option<u64>) -> String {
    size.map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_hash(hex_str: &str) -> Result<Hash, SyncError> {
    let bytes = hex::decode(hex_str).map_err(|e| SyncError::Store(format!("invalid hash: {e}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SyncError::Store("invalid hash length".to_string()))?;
    Ok(Hash::from_bytes(bytes))
}
