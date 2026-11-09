use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rua_core::{Result, RuaError};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};

use crate::cuda::{detect_arch, detect_cuda, detect_os};
use crate::download_ipc::{DownloadStatus, IpcMessage};
use crate::model_source::ModelSource;
use crate::process::{wait_for_health, LlamaProcess};
use crate::server_binary::ServerBinaryDownloader;
use crate::source_custom::CustomUrlSource;
use crate::source_hf::HfSource;
use crate::source_modelscope::ModelScopeSource;
use crate::source_ollama::OllamaSource;

const DEFAULT_LLAMA_BINARY: &str = "llama-server";
const DEFAULT_LLAMA_PORT: u16 = 8080;
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: Option<u64>,
    pub format: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub found: Vec<LocalModelInfo>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ServerStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub model_id: String,
    pub model_name: Option<String>,
    pub status: String,
    pub progress: Option<f64>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_sec: u64,
    pub source: Option<String>,
    pub error: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ServerUpdateInfo {
    pub has_update: Option<bool>,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StartServerRequest {
    pub model_id: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone)]
pub enum ModelSourceConfig {
    HuggingFace {
        repo_id: String,
        filename: Option<String>,
    },
    Ollama {
        model_name: String,
        tag: Option<String>,
    },
    CustomUrl {
        url: String,
    },
    ModelScope {
        repo_id: String,
        filename: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct DownloadState {
    model_id: String,
    model_name: Option<String>,
    status: String,
    progress: Option<f64>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    speed_bytes_per_sec: u64,
    source: Option<String>,
    error: Option<String>,
    local_path: Option<String>,
}

pub struct LocalModelManager {
    working_dir: PathBuf,
    scan_enabled: bool,
    llama_binary: String,
    health_timeout: Duration,

    skip_health_check: bool,

    server_api_url: Option<String>,

    server_release_base_url: Option<String>,
    watchdog_interval: Duration,
    models: Arc<RwLock<HashMap<String, LocalModelInfo>>>,
    server: RwLock<ServerStatus>,
    process: RwLock<Option<LlamaProcess>>,
    server_download: Arc<RwLock<Option<DownloadState>>>,
    model_download: Arc<RwLock<Option<DownloadState>>>,
    download_streams: Arc<RwLock<HashMap<String, broadcast::Sender<IpcMessage>>>>,
    server_download_abort: RwLock<Option<tokio::task::AbortHandle>>,
    model_download_abort: RwLock<Option<tokio::task::AbortHandle>>,
}

static INSTANCE: OnceLock<Arc<LocalModelManager>> = OnceLock::new();

impl LocalModelManager {
    pub(crate) fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            scan_enabled: true,
            llama_binary: DEFAULT_LLAMA_BINARY.to_string(),
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            skip_health_check: false,
            server_api_url: None,
            server_release_base_url: None,
            watchdog_interval: DEFAULT_WATCHDOG_INTERVAL,
            models: Arc::new(RwLock::new(HashMap::new())),
            server: RwLock::new(ServerStatus::default()),
            process: RwLock::new(None),
            server_download: Arc::new(RwLock::new(None)),
            model_download: Arc::new(RwLock::new(None)),
            download_streams: Arc::new(RwLock::new(HashMap::new())),
            server_download_abort: RwLock::new(None),
            model_download_abort: RwLock::new(None),
        }
    }

    pub fn with_scan_enabled(mut self, enabled: bool) -> Self {
        self.scan_enabled = enabled;
        self
    }

    pub fn with_llama_binary(mut self, binary: impl Into<String>) -> Self {
        self.llama_binary = binary.into();
        self
    }

    pub fn with_health_timeout(mut self, timeout: Duration) -> Self {
        self.health_timeout = timeout;
        self
    }

    pub fn with_skip_health_check(mut self, skip: bool) -> Self {
        self.skip_health_check = skip;
        self
    }

    pub fn with_server_api_url(mut self, url: impl Into<String>) -> Self {
        self.server_api_url = Some(url.into());
        self
    }

    pub fn with_server_release_base_url(mut self, url: impl Into<String>) -> Self {
        self.server_release_base_url = Some(url.into());
        self
    }

    pub fn with_watchdog_interval(mut self, interval: Duration) -> Self {
        self.watchdog_interval = interval;
        self
    }

    pub fn init_instance(working_dir: impl Into<PathBuf>, scan_enabled: bool) -> Arc<Self> {
        INSTANCE
            .get_or_init(|| Arc::new(Self::new(working_dir).with_scan_enabled(scan_enabled)))
            .clone()
    }

    pub fn instance() -> Arc<Self> {
        INSTANCE
            .get()
            .expect("LocalModelManager::init_instance() must be called before instance()")
            .clone()
    }

    pub fn new_for_test(working_dir: impl Into<PathBuf>) -> Self {
        Self::new(working_dir)
    }

    pub async fn list_models(&self) -> Vec<LocalModelInfo> {
        self.models.read().await.values().cloned().collect()
    }

    pub fn get_recommended_models(&self, memory_gb: u64) -> Vec<crate::RecommendedModel> {
        let mut models = crate::get_recommended_models(memory_gb);
        for m in &mut models {
            m.downloaded = self.is_downloaded(&m.id);
        }
        models
    }

    fn is_downloaded(&self, model_id: &str) -> bool {
        let safe_name = model_id.replace(['/', ':'], "_");
        let model_dir = self.working_dir.join("models");
        let gguf_path = model_dir.join(format!("{safe_name}.gguf"));
        gguf_path.exists()
    }

    pub async fn scan(&self, paths: &[String]) -> ScanResult {
        if !self.scan_enabled {
            return ScanResult::default();
        }

        let mut found = Vec::new();
        let mut errors = Vec::new();

        for path in paths {
            match std::fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let file_path = entry.path();
                        let metadata = entry.metadata().ok();
                        if !metadata.as_ref().is_some_and(|m| m.is_file()) {
                            continue;
                        }
                        let name = file_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();

                        let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
                        let format = match ext {
                            "gguf" => Some("gguf".to_string()),
                            "bin" => Some("bin".to_string()),
                            _ => continue,
                        };
                        if format.as_deref() == Some("gguf")
                            && name.to_ascii_lowercase().contains("mmproj")
                        {
                            continue;
                        }

                        let size = metadata.map(|m| m.len());
                        let id = format!("local-{}", name);
                        found.push(LocalModelInfo {
                            id,
                            name,
                            path: file_path.to_string_lossy().to_string(),
                            size,
                            format,
                            quantization: None,
                        });
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path, e));
                }
            }
        }

        let mut models = self.models.write().await;
        for m in &found {
            models.insert(m.id.clone(), m.clone());
        }

        ScanResult { found, errors }
    }

    pub async fn delete_model(&self, model_id: &str) -> Result<()> {
        let mut models = self.models.write().await;
        let model = models.remove(model_id).ok_or_else(|| RuaError::NotFound {
            resource: "local model".into(),
            id: model_id.to_string(),
        })?;
        drop(models);

        let path = PathBuf::from(&model.path);
        let managed_models_dir = self.working_dir.join("models");
        if path.starts_with(&managed_models_dir) && path.is_file() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| RuaError::Io {
                    message: format!("failed to delete local model file {}: {e}", path.display()),
                })?;
        }
        Ok(())
    }

    pub async fn server_status(&self) -> ServerStatus {
        let unhealthy = {
            let guard = self.process.read().await;
            guard.as_ref().is_some_and(|p| !p.is_healthy())
        };
        if unhealthy {
            let mut status = self.server.write().await;
            if status.running {
                status.running = false;
            }
            return status.clone();
        }
        self.server.read().await.clone()
    }

    pub fn server_binary_installed(&self) -> bool {
        self.installed_server_binary_path().is_file()
    }

    fn installed_server_binary_path(&self) -> PathBuf {
        let binary_name = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        self.working_dir.join("server").join(binary_name)
    }

    pub async fn check_server_health(&self) -> Result<()> {
        let guard = self.process.read().await;
        match &*guard {
            Some(proc) if proc.is_healthy() => Ok(()),
            Some(_) => Err(RuaError::Provider {
                provider: "local".into(),
                message: "local model exited unexpectedly".into(),
                retryable: false,
            }),
            None => Err(RuaError::Conflict {
                resource: "local model server".into(),
                detail: "server not running".into(),
            }),
        }
    }

    pub async fn start_server(&self, req: StartServerRequest) -> Result<ServerStatus> {
        let mut process_guard = self.process.write().await;
        if process_guard.is_some() {
            return Err(RuaError::Conflict {
                resource: "local model server".into(),
                detail: "server already running".into(),
            });
        }

        let model_path = if let Some(ref model_id) = req.model_id {
            let models = self.models.read().await;
            let model = models.get(model_id).ok_or_else(|| RuaError::NotFound {
                resource: "local model".into(),
                id: model_id.clone(),
            })?;
            let path = Path::new(&model.path);
            if !path.exists() {
                return Err(RuaError::Validation {
                    message: format!("model file not found: {}", model.path),
                });
            }
            Some(model.path.clone())
        } else {
            None
        };

        let port = req.port.unwrap_or(DEFAULT_LLAMA_PORT);
        if port != 0 && !is_port_available(port) {
            return Err(RuaError::Validation {
                message: format!("local model server port {port} is not available"),
            });
        }
        let proc = LlamaProcess::spawn(
            &self.llama_binary,
            port,
            model_path.as_deref(),
            req.model_id.clone(),
        )?;
        let pid = proc.pid;
        *process_guard = Some(proc);
        drop(process_guard);

        if !self.skip_health_check {
            if let Err(e) = wait_for_health(port, self.health_timeout).await {
                if let Some(p) = self.process.write().await.take() {
                    p.shutdown().await;
                }
                return Err(e);
            }
        }

        {
            let mut guard = self.process.write().await;
            if let Some(ref mut proc) = *guard {
                proc.start_watchdog(self.watchdog_interval);
            }
        }

        let status = ServerStatus {
            running: true,
            pid: Some(pid),
            port: Some(port),
            model: req.model_id,
        };
        *self.server.write().await = status.clone();
        Ok(status)
    }

    pub async fn stop_server(&self) -> Result<()> {
        let proc = self
            .process
            .write()
            .await
            .take()
            .ok_or_else(|| RuaError::Conflict {
                resource: "local model server".into(),
                detail: "server not running".into(),
            })?;
        proc.shutdown().await;
        *self.server.write().await = ServerStatus::default();
        Ok(())
    }

    pub async fn check_server_update(&self) -> ServerUpdateInfo {
        let current = self.read_installed_server_version().await;

        let latest = match self.fetch_latest_server_tag().await {
            Ok(tag) => tag,
            Err(e) => {
                tracing::warn!("failed to fetch latest server release: {e}");
                return ServerUpdateInfo {
                    has_update: None,
                    current_version: current,
                    latest_version: None,
                };
            }
        };

        let has_update = current.as_deref().map(|cur| version_is_newer(cur, &latest));

        ServerUpdateInfo {
            has_update,
            current_version: current,
            latest_version: Some(latest),
        }
    }

    async fn read_installed_server_version(&self) -> Option<String> {
        let path = self.working_dir.join("server").join("version.txt");
        tokio::fs::read_to_string(&path)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    async fn fetch_latest_server_tag(&self) -> Result<String> {
        let os = detect_os();
        let arch = detect_arch();
        let cuda = detect_cuda();
        let mut downloader = ServerBinaryDownloader::new(os, arch, cuda, &self.working_dir);
        if let Some(ref url) = self.server_api_url {
            downloader = downloader.with_api_url(url.clone());
        }
        let client = rua_http::default_client_or_default("rua-local-model");
        downloader.fetch_latest_tag(&client).await
    }

    pub async fn download_server(&self, model_id: String) -> DownloadProgress {
        if self.server_status().await.running {
            if let Err(err) = self.stop_server().await {
                tracing::warn!(error = %err, "failed to stop local model server before server download");
            }
        }

        if let Some(abort) = self.server_download_abort.write().await.take() {
            abort.abort();
        }
        if let Some(previous_model_id) = self
            .server_download
            .read()
            .await
            .as_ref()
            .map(|state| state.model_id.clone())
        {
            self.download_streams
                .write()
                .await
                .remove(&previous_model_id);
        }

        let state = DownloadState {
            model_id: model_id.clone(),
            model_name: Some(model_id.clone()),
            status: "downloading".into(),
            progress: Some(0.0),
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_sec: 0,
            source: Some("auto".into()),
            error: None,
            local_path: None,
        };
        *self.server_download.write().await = Some(state);

        let (broadcast_tx, _) = broadcast::channel(64);
        self.download_streams
            .write()
            .await
            .insert(model_id.clone(), broadcast_tx.clone());

        let (mpsc_tx, mut mpsc_rx) = mpsc::channel::<IpcMessage>(64);

        let os = detect_os();
        let arch = detect_arch();
        let cuda = detect_cuda();
        let mut downloader = ServerBinaryDownloader::new(os, arch, cuda, &self.working_dir);
        if let Some(ref url) = self.server_api_url {
            downloader = downloader.with_api_url(url.clone());
        }
        if let Some(ref url) = self.server_release_base_url {
            downloader = downloader.with_release_base_url(url.clone());
        }

        let dl_state = self.server_download.clone();
        let streams = self.download_streams.clone();
        let mid = model_id.clone();

        let handle = tokio::spawn(async move {
            let drain_state = dl_state.clone();
            let drain_handle = tokio::spawn(async move {
                while let Some(msg) = mpsc_rx.recv().await {
                    let _ = broadcast_tx.send(msg.clone());
                    update_download_state(&drain_state, &msg).await;
                }
            });

            let result = downloader.download(Some(mpsc_tx), None).await;
            let _ = drain_handle.await;

            match result {
                Ok(path) => {
                    let metadata = tokio::fs::metadata(&path).await.ok();
                    let mut dl = dl_state.write().await;
                    if let Some(ref mut s) = *dl {
                        s.status = "completed".into();
                        s.progress = Some(1.0);
                        s.local_path = Some(path.to_string_lossy().to_string());
                        if let Some(size) = metadata.map(|metadata| metadata.len()) {
                            s.downloaded_bytes = s.downloaded_bytes.max(size);
                            s.total_bytes = Some(s.total_bytes.unwrap_or(size).max(size));
                        }
                    }
                }
                Err(e) => {
                    let mut dl = dl_state.write().await;
                    if let Some(ref mut s) = *dl {
                        s.status = "failed".into();
                        s.error = Some(e.to_string());
                    }
                }
            }

            streams.write().await.remove(&mid);
        });

        *self.server_download_abort.write().await = Some(handle.abort_handle());

        DownloadProgress {
            model_id,
            model_name: Some("llama-server".into()),
            status: "downloading".into(),
            progress: Some(0.0),
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_sec: 0,
            source: Some("auto".into()),
            error: None,
            local_path: None,
        }
    }

    pub async fn server_download_progress(&self) -> Result<DownloadProgress> {
        match self.server_download.read().await.as_ref() {
            Some(s) => Ok(download_progress_from_state(s)),
            None => Err(RuaError::NotFound {
                resource: "server download".into(),
                id: String::new(),
            }),
        }
    }

    pub async fn cancel_server_download(&self) -> Result<()> {
        let mut dl = self.server_download.write().await;
        if dl.is_none() {
            return Err(RuaError::NotFound {
                resource: "server download".into(),
                id: String::new(),
            });
        }
        let mid = dl.as_ref().unwrap().model_id.clone();
        *dl = None;
        drop(dl);

        if let Some(abort) = self.server_download_abort.write().await.take() {
            abort.abort();
        }
        self.download_streams.write().await.remove(&mid);
        Ok(())
    }

    pub async fn download_model(&self, model_id: String) -> DownloadProgress {
        let source = infer_source_config(&model_id);
        self.download_model_with_source(model_id, source).await
    }

    pub async fn download_model_with_source(
        &self,
        model_id: String,
        source_config: ModelSourceConfig,
    ) -> DownloadProgress {
        let source: Box<dyn ModelSource> = match source_config {
            ModelSourceConfig::HuggingFace { repo_id, filename } => {
                Box::new(HfSource::new(repo_id, filename))
            }
            ModelSourceConfig::Ollama { model_name, tag } => {
                Box::new(OllamaSource::new(model_name, tag))
            }
            ModelSourceConfig::CustomUrl { url } => Box::new(CustomUrlSource::new(url, &model_id)),
            ModelSourceConfig::ModelScope { repo_id, filename } => {
                Box::new(ModelScopeSource::new(repo_id, filename))
            }
        };
        self.download_model_from_source(model_id, source).await
    }

    pub async fn download_model_from_source(
        &self,
        model_id: String,
        source: Box<dyn ModelSource>,
    ) -> DownloadProgress {
        if let Some(abort) = self.model_download_abort.write().await.take() {
            abort.abort();
        }
        if let Some(previous_model_id) = self
            .model_download
            .read()
            .await
            .as_ref()
            .map(|state| state.model_id.clone())
        {
            self.download_streams
                .write()
                .await
                .remove(&previous_model_id);
        }

        let source_name = normalize_download_source_name(source.name());
        let state = DownloadState {
            model_id: model_id.clone(),
            model_name: Some(model_id.clone()),
            status: "downloading".into(),
            progress: Some(0.0),
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_sec: 0,
            source: Some(source_name.clone()),
            error: None,
            local_path: None,
        };
        *self.model_download.write().await = Some(state);

        let (broadcast_tx, _) = broadcast::channel(64);
        self.download_streams
            .write()
            .await
            .insert(model_id.clone(), broadcast_tx.clone());

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<IpcMessage>();

        let dl_state = self.model_download.clone();
        let streams = self.download_streams.clone();
        let models = self.models.clone();
        let mid = model_id.clone();
        let safe_name = model_id.replace(['/', ':'], "_");
        let dest_path = self
            .working_dir
            .join("models")
            .join(format!("{safe_name}.gguf"));
        let temp_path = self
            .working_dir
            .join("models")
            .join(format!(".{safe_name}.gguf.downloading"));

        let handle = tokio::spawn(async move {
            if let Some(parent) = dest_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::remove_file(&temp_path).await;

            let drain_state = dl_state.clone();
            let drain_handle = tokio::spawn(async move {
                while let Some(msg) = progress_rx.recv().await {
                    let _ = broadcast_tx.send(msg.clone());
                    update_download_state(&drain_state, &msg).await;
                }
            });

            let result = match source.fetch_to(&temp_path, progress_tx).await {
                Ok(()) => tokio::fs::rename(&temp_path, &dest_path)
                    .await
                    .map_err(|e| RuaError::Io {
                        message: format!("failed to finalize model download: {e}"),
                    }),
                Err(e) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    Err(e)
                }
            };
            let _ = drain_handle.await;

            match result {
                Ok(()) => {
                    let metadata = tokio::fs::metadata(&dest_path).await.ok();
                    let size = metadata.as_ref().map(|metadata| metadata.len());
                    let format = dest_path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(str::to_string);
                    models.write().await.insert(
                        mid.clone(),
                        LocalModelInfo {
                            id: mid.clone(),
                            name: mid.clone(),
                            path: dest_path.to_string_lossy().to_string(),
                            size,
                            format,
                            quantization: None,
                        },
                    );
                    let mut dl = dl_state.write().await;
                    if let Some(ref mut s) = *dl {
                        s.status = "completed".into();
                        s.progress = Some(1.0);
                        s.local_path = Some(dest_path.to_string_lossy().to_string());
                        if let Some(size) = size {
                            s.downloaded_bytes = s.downloaded_bytes.max(size);
                            s.total_bytes = Some(s.total_bytes.unwrap_or(size).max(size));
                        }
                    }
                }
                Err(e) => {
                    let mut dl = dl_state.write().await;
                    if let Some(ref mut s) = *dl {
                        s.status = "failed".into();
                        s.error = Some(e.to_string());
                    }
                }
            }

            streams.write().await.remove(&mid);
        });

        *self.model_download_abort.write().await = Some(handle.abort_handle());

        DownloadProgress {
            model_name: Some(model_id.clone()),
            model_id,
            status: "downloading".into(),
            progress: Some(0.0),
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_sec: 0,
            source: Some(source_name),
            error: None,
            local_path: None,
        }
    }

    pub async fn model_download_progress(&self) -> Result<DownloadProgress> {
        match self.model_download.read().await.as_ref() {
            Some(s) => Ok(download_progress_from_state(s)),
            None => Err(RuaError::NotFound {
                resource: "model download".into(),
                id: String::new(),
            }),
        }
    }

    pub async fn cancel_model_download(&self) -> Result<()> {
        let mut dl = self.model_download.write().await;
        if dl.is_none() {
            return Err(RuaError::NotFound {
                resource: "model download".into(),
                id: String::new(),
            });
        }
        let mid = dl.as_ref().unwrap().model_id.clone();
        *dl = None;
        drop(dl);

        if let Some(abort) = self.model_download_abort.write().await.take() {
            abort.abort();
        }
        self.download_streams.write().await.remove(&mid);
        Ok(())
    }

    pub async fn register_download_sender(
        &self,
        task_id: String,
        sender: broadcast::Sender<IpcMessage>,
    ) {
        self.download_streams.write().await.insert(task_id, sender);
    }

    pub async fn subscribe_download(
        &self,
        task_id: &str,
    ) -> Result<broadcast::Receiver<IpcMessage>> {
        let streams = self.download_streams.read().await;
        match streams.get(task_id) {
            Some(sender) => Ok(sender.subscribe()),
            None => Err(RuaError::NotFound {
                resource: "download stream".into(),
                id: task_id.to_string(),
            }),
        }
    }
}

fn download_progress_from_state(state: &DownloadState) -> DownloadProgress {
    DownloadProgress {
        model_id: state.model_id.clone(),
        model_name: state.model_name.clone(),
        status: state.status.clone(),
        progress: state.progress,
        downloaded_bytes: state.downloaded_bytes,
        total_bytes: state.total_bytes,
        speed_bytes_per_sec: state.speed_bytes_per_sec,
        source: state.source.clone(),
        error: state.error.clone(),
        local_path: state.local_path.clone(),
    }
}

fn normalize_download_source_name(source: &str) -> String {
    match source {
        "huggingface" | "modelscope" => source.to_string(),
        _ => "auto".to_string(),
    }
}

async fn update_download_state(state: &Arc<RwLock<Option<DownloadState>>>, msg: &IpcMessage) {
    match msg {
        IpcMessage::Progress(p) => {
            let progress = if p.bytes_total > 0 {
                Some(p.bytes_downloaded as f64 / p.bytes_total as f64)
            } else {
                None
            };
            let mut dl = state.write().await;
            if let Some(ref mut s) = *dl {
                s.progress = progress;
                s.downloaded_bytes = p.bytes_downloaded;
                s.total_bytes = (p.bytes_total > 0).then_some(p.bytes_total);
            }
        }
        IpcMessage::Status(s) => {
            let mut dl = state.write().await;
            if let Some(ref mut st) = *dl {
                match s.status {
                    DownloadStatus::Completed => {
                        st.status = "completed".into();
                        st.progress = Some(1.0);
                    }
                    DownloadStatus::Cancelled => {
                        st.status = "cancelled".into();
                    }
                    DownloadStatus::Started => {}
                }
            }
        }
        IpcMessage::Error(e) => {
            let mut dl = state.write().await;
            if let Some(ref mut s) = *dl {
                s.status = "failed".into();
                s.error = Some(e.error.clone());
            }
        }
    }
}

fn infer_source_config(model_id: &str) -> ModelSourceConfig {
    if model_id.starts_with("http://") || model_id.starts_with("https://") {
        ModelSourceConfig::CustomUrl {
            url: model_id.to_string(),
        }
    } else if let Some(rest) = model_id
        .strip_prefix("ms:")
        .or_else(|| model_id.strip_prefix("modelscope:"))
    {
        ModelSourceConfig::ModelScope {
            repo_id: rest.to_string(),
            filename: None,
        }
    } else if model_id.contains(':') {
        let parts: Vec<&str> = model_id.splitn(2, ':').collect();
        ModelSourceConfig::Ollama {
            model_name: parts[0].to_string(),
            tag: Some(parts[1].to_string()),
        }
    } else {
        ModelSourceConfig::HuggingFace {
            repo_id: model_id.to_string(),
            filename: None,
        }
    }
}

fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn version_is_newer(current: &str, latest: &str) -> bool {
    if let (Ok(cur), Ok(lat)) = (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        return lat > cur;
    }

    if let (Some(cur_n), Some(lat_n)) = (parse_build_number(current), parse_build_number(latest)) {
        return lat_n > cur_n;
    }

    latest > current
}

fn parse_build_number(tag: &str) -> Option<u64> {
    tag.strip_prefix('b')
        .or(Some(tag))
        .and_then(|s| s.parse::<u64>().ok())
}
