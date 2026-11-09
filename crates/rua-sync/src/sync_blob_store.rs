use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use iroh::{Endpoint, EndpointId};
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::api::downloader::{
    DownloadOptions, DownloadProgressItem, Downloader, SplitStrategy,
};
use iroh_blobs::store::fs::{
    options::{CompactConfig, Options},
    CompactPolicy, FsStore,
};
use iroh_blobs::{Hash, HashAndFormat};
use serde::Serialize;
use tracing::{debug, warn};

use crate::SyncError;

#[derive(Debug, Clone, Default, Serialize)]
pub struct BlobDownloadDiagnostics {
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<BlobDownloadAttemptDiagnostics>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlobDownloadAttemptDiagnostics {
    pub hash: String,
    pub label: String,
    pub provider_order: Vec<String>,
    pub tried_providers: Vec<String>,
    pub failed_providers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_provider: Option<String>,
    pub bytes: u64,
    pub provider_selection_ms: u64,
    pub connection_ms: u64,
    pub download_ms: u64,
    pub duration_ms: u64,
    pub route: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatefulBlobDownloader {
    downloader: Downloader,
    endpoint: Endpoint,
    diagnostics: Arc<Mutex<BlobDownloadDiagnostics>>,
}

impl StatefulBlobDownloader {
    pub fn new(downloader: Downloader, endpoint: Endpoint) -> Self {
        Self {
            downloader,
            endpoint,
            diagnostics: Arc::new(Mutex::new(BlobDownloadDiagnostics::default())),
        }
    }

    pub fn raw_downloader(&self) -> Downloader {
        self.downloader.clone()
    }

    pub fn diagnostics(&self) -> BlobDownloadDiagnostics {
        self.diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub async fn download(
        &self,
        hash: Hash,
        providers: Vec<EndpointId>,
        timeout: Duration,
        label: &str,
    ) -> Result<(), SyncError> {
        if providers.is_empty() {
            return Err(SyncError::Store(format!(
                "download {label} {hash}: no providers"
            )));
        }
        let started = Instant::now();
        let provider_order = providers.iter().map(ToString::to_string).collect();
        let request =
            DownloadOptions::new(HashAndFormat::raw(hash), providers, SplitStrategy::None);
        let progress = self.downloader.download_with_opts(request);
        let summary = Arc::new(tokio::sync::Mutex::new(DownloadProgressSummary::default()));
        let summary_for_download = Arc::clone(&summary);
        let result = tokio::time::timeout(timeout, async {
            let stream = progress
                .stream()
                .await
                .map_err(|error| SyncError::Store(error.to_string()))?;
            tokio::pin!(stream);
            while let Some(item) = stream.next().await {
                match item {
                    DownloadProgressItem::Error(error) => {
                        return Err(SyncError::Store(error.to_string()));
                    }
                    DownloadProgressItem::TryProvider { id, .. } => {
                        let mut summary = summary_for_download.lock().await;
                        summary.first_provider_at.get_or_insert_with(Instant::now);
                        summary.selected_provider = Some(id);
                        summary.tried_providers.push(id.to_string());
                    }
                    DownloadProgressItem::ProviderFailed { id, .. } => {
                        summary_for_download
                            .lock()
                            .await
                            .failed_providers
                            .push(id.to_string());
                    }
                    DownloadProgressItem::Progress(progress) => {
                        let mut summary = summary_for_download.lock().await;
                        summary.first_progress_at.get_or_insert_with(Instant::now);
                        summary.bytes = summary.bytes.max(progress);
                    }
                    DownloadProgressItem::PartComplete { .. } => {}
                    DownloadProgressItem::DownloadError => {
                        return Err(SyncError::Store("downloader reported failure".to_string()));
                    }
                }
            }
            Ok(())
        })
        .await;

        let failure = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(_) => Some(format!("timed out after {timeout:?}")),
        };
        let summary = summary.lock().await.clone();
        let route = route_for_provider(&self.endpoint, summary.selected_provider).await;
        let duration_ms = millis(started.elapsed());
        let provider_selection_ms = summary
            .first_provider_at
            .map(|at| millis(at.duration_since(started)))
            .unwrap_or_default();
        let connection_ms = summary
            .first_progress_at
            .zip(summary.first_provider_at)
            .map(|(progress, provider)| millis(progress.duration_since(provider)))
            .unwrap_or_default();
        let download_ms = summary
            .first_progress_at
            .map(|progress| millis(progress.elapsed()))
            .unwrap_or_default();
        let mut diagnostics = self
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        diagnostics.requests = diagnostics.requests.saturating_add(1);
        if failure.is_none() {
            diagnostics.successes = diagnostics.successes.saturating_add(1);
        } else {
            diagnostics.failures = diagnostics.failures.saturating_add(1);
        }
        diagnostics.last = Some(BlobDownloadAttemptDiagnostics {
            hash: hash.to_string(),
            label: label.to_string(),
            provider_order,
            tried_providers: summary.tried_providers,
            failed_providers: summary.failed_providers,
            selected_provider: summary.selected_provider.map(|id| id.to_string()),
            bytes: summary.bytes,
            provider_selection_ms,
            connection_ms,
            download_ms,
            duration_ms,
            route,
            status: if failure.is_none() {
                "success"
            } else {
                "failure"
            },
            failure: failure.clone(),
        });
        drop(diagnostics);

        match failure {
            Some(error) => Err(SyncError::Store(format!(
                "download {label} {hash}: {error}"
            ))),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Default)]
struct DownloadProgressSummary {
    tried_providers: Vec<String>,
    failed_providers: Vec<String>,
    selected_provider: Option<EndpointId>,
    first_provider_at: Option<Instant>,
    first_progress_at: Option<Instant>,
    bytes: u64,
}

async fn route_for_provider(endpoint: &Endpoint, provider: Option<EndpointId>) -> &'static str {
    let Some(provider) = provider else {
        return "unknown";
    };
    let Some(info) = endpoint.remote_info(provider).await else {
        return "unknown";
    };
    let mut direct = false;
    let mut relay = false;
    for addr in info.addrs() {
        if matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active) {
            direct |= addr.addr().is_ip();
            relay |= addr.addr().is_relay();
        }
    }
    match (direct, relay) {
        (true, false) => "direct",
        (false, true) => "relay",
        (true, true) => "mixed",
        (false, false) => "unknown",
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncBlobFetchConfig {
    pub parallel_streams: u8,
}

impl Default for SyncBlobFetchConfig {
    fn default() -> Self {
        Self {
            parallel_streams: 4,
        }
    }
}

impl SyncBlobFetchConfig {
    pub fn new(parallel_streams: u8) -> Result<Self, SyncError> {
        match parallel_streams {
            1 | 2 | 4 | 8 | 16 => Ok(Self { parallel_streams }),
            other => Err(SyncError::Store(format!(
                "invalid parallel_streams {other}, must be 1/2/4/8/16"
            ))),
        }
    }
}

pub struct SyncBlobStore {
    store: FsStore,
    config: SyncBlobFetchConfig,
}

impl SyncBlobStore {
    pub async fn open(blobs_dir: &Path, config: SyncBlobFetchConfig) -> Result<Self, SyncError> {
        std::fs::create_dir_all(blobs_dir)?;
        let mut options = Options::new(blobs_dir);
        options.compact = Some(CompactConfig {
            policy: CompactPolicy {
                min_file_size: 256 * 1024 * 1024,
                min_reclaim_bytes: 128 * 1024 * 1024,
                min_reclaim_ratio: 0.25,
            },
            hour_local: 3,
        });
        let store = FsStore::load_with_opts(blobs_dir.join("blobs.db"), options)
            .await
            .map_err(|e| SyncError::Store(format!("open sync blob store: {e}")))?;
        Ok(Self { store, config })
    }

    pub fn api_store(&self) -> crate::endpoint::BlobsApiStore {
        self.store.clone().into()
    }

    pub fn store(&self) -> &FsStore {
        &self.store
    }

    pub async fn has_blob(&self, hash_hex: &str) -> Result<bool, SyncError> {
        let hash = parse_hash(hash_hex)?;
        let status = self
            .store
            .status(hash)
            .await
            .map_err(|e| SyncError::Store(format!("blob status: {e}")))?;
        Ok(matches!(status, BlobStatus::Complete { .. }))
    }

    pub async fn fetch_missing(
        &self,
        hash_refs: &[String],
        endpoint: &Endpoint,
        peer_id: iroh::PublicKey,
    ) -> Result<usize, SyncError> {
        let mut missing = Vec::new();
        for hash_ref in hash_refs {
            if !self.has_blob(hash_ref).await? {
                missing.push(hash_ref);
            }
        }

        if missing.is_empty() {
            return Ok(0);
        }

        let concurrency = self.config.parallel_streams as usize;
        let mut fetched = 0usize;

        for chunk in missing.chunks(concurrency) {
            let mut handles = Vec::new();
            for hash_ref in chunk {
                let hash = parse_hash(hash_ref)?;
                let ep = endpoint.clone();
                let store = self.store.clone();
                let peer = peer_id;
                handles.push(tokio::spawn(async move {
                    fetch_single_blob(&store, &ep, peer, hash).await
                }));
            }
            for handle in handles {
                match handle.await {
                    Ok(Ok(())) => fetched += 1,
                    Ok(Err(e)) => warn!("sync blob fetch failed: {e}"),
                    Err(e) => warn!("sync blob fetch task panic: {e}"),
                }
            }
        }

        debug!(
            fetched,
            total_missing = missing.len(),
            "sync blob fetch_missing done"
        );
        Ok(fetched)
    }

    pub fn config(&self) -> &SyncBlobFetchConfig {
        &self.config
    }
}

async fn fetch_single_blob(
    store: &FsStore,
    endpoint: &Endpoint,
    peer: iroh::PublicKey,
    hash: Hash,
) -> Result<(), SyncError> {
    let api_store: crate::endpoint::BlobsApiStore = store.clone().into();
    api_store
        .downloader(endpoint)
        .download(HashAndFormat::raw(hash), vec![peer])
        .await
        .map_err(|e| SyncError::Store(format!("store fetched blob: {e}")))?;

    debug!(hash = %hash.to_hex(), "sync blob fetched and stored");
    Ok(())
}

fn parse_hash(hex_str: &str) -> Result<Hash, SyncError> {
    let bytes: [u8; 32] = hex::decode(hex_str)
        .map_err(|e| SyncError::Store(format!("invalid blob hash hex: {e}")))?
        .try_into()
        .map_err(|_| SyncError::Store("blob hash must be 32 bytes".into()))?;
    Ok(Hash::from_bytes(bytes))
}

pub async fn build_sync_blob_store_from_dir(
    sync_dir: &Path,
) -> Option<std::sync::Arc<SyncBlobStore>> {
    let blobs_dir = resolve_sync_blob_store_dir(sync_dir);
    let cfg = read_blob_fetch_config_from_dir(sync_dir);
    match SyncBlobStore::open(&blobs_dir, cfg).await {
        Ok(store) => Some(std::sync::Arc::new(store)),
        Err(e) => {
            warn!(error = %e, "[sync] blobs: SyncBlobStore open failed");
            None
        }
    }
}

pub fn resolve_sync_blob_store_dir(sync_dir: &Path) -> std::path::PathBuf {
    let legacy_dir = sync_dir.join("iroh-blobs");
    if legacy_dir.join("blobs.db").exists() || legacy_dir.join("data").is_dir() {
        return legacy_dir;
    }
    sync_dir.join("blobs")
}

fn read_blob_fetch_config_from_dir(sync_dir: &Path) -> SyncBlobFetchConfig {
    let path = sync_dir.join("sync_blob_fetch_config.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(n) = v.get("parallel_streams").and_then(|n| n.as_u64()) {
                if let Ok(cfg) = SyncBlobFetchConfig::new(n as u8) {
                    return cfg;
                }
            }
        }
    }
    SyncBlobFetchConfig::default()
}
