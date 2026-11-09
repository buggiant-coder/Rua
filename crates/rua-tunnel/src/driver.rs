use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch, Mutex};
use tracing::{debug, info, warn};

use crate::binary_manager::{self, BinaryManager};
use crate::tunnel_info::TunnelInfo;

const URL_WAIT_TIMEOUT_SECS: u64 = 30;
const STOP_TIMEOUT_SECS: u64 = 5;

const DEFAULT_RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("binary manager error: {0}")]
    BinaryManager(#[source] binary_manager::Error),
    #[error("failed to spawn cloudflared: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("cloudflared did not produce a tunnel URL within {0} seconds")]
    UrlTimeout(u64),
    #[error("cloudflared stderr not available")]
    NoStderr,
}

#[derive(Debug, Clone)]
pub enum TunnelEvent {
    Started { url: String },
    Restarted { url: String },
    AllRetriesFailed,
}

pub struct CloudflareTunnelDriver {
    binary_mgr: BinaryManager,
    inner: Arc<Mutex<DriverInner>>,
    event_tx: broadcast::Sender<TunnelEvent>,
    retry_backoffs: Vec<Duration>,
}

struct DriverInner {
    info: Option<TunnelInfo>,
    running: Option<RunningState>,
}

struct RunningState {
    shutdown_tx: watch::Sender<bool>,
    monitor_handle: tokio::task::JoinHandle<()>,
}

impl CloudflareTunnelDriver {
    pub fn new(binary_manager: BinaryManager) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            binary_mgr: binary_manager,
            inner: Arc::new(Mutex::new(DriverInner {
                info: None,
                running: None,
            })),
            event_tx,
            retry_backoffs: DEFAULT_RETRY_BACKOFFS.to_vec(),
        }
    }

    pub fn with_retry_backoffs(mut self, backoffs: Vec<Duration>) -> Self {
        self.retry_backoffs = backoffs;
        self
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<TunnelEvent> {
        self.event_tx.subscribe()
    }

    pub async fn start(&self, local_port: u16) -> Result<TunnelInfo, Error> {
        self.stop().await;
        let binary = self
            .binary_mgr
            .ensure_binary()
            .await
            .map_err(Error::BinaryManager)?;
        self.start_tunnel(&binary, local_port).await
    }

    pub async fn stop(&self) {
        let running = {
            let mut inner = self.inner.lock().await;
            inner.info = None;
            inner.running.take()
        };
        if let Some(running) = running {
            let _ = running.shutdown_tx.send(true);
            let mut handle = running.monitor_handle;
            if tokio::time::timeout(Duration::from_secs(STOP_TIMEOUT_SECS), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }

    pub async fn get_info(&self) -> Option<TunnelInfo> {
        self.inner.lock().await.info.clone()
    }

    pub async fn is_running(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.info.is_some() && inner.running.is_some()
    }

    pub async fn start_tunnel(&self, binary: &Path, local_port: u16) -> Result<TunnelInfo, Error> {
        info!("Starting cloudflared tunnel -> http://localhost:{local_port}");

        let mut child = spawn_cloudflared(binary, local_port).map_err(Error::Spawn)?;

        let pid = child.id();
        let stderr = child.stderr.take().ok_or(Error::NoStderr)?;
        let reader = BufReader::new(stderr);

        let url = wait_for_url(reader, URL_WAIT_TIMEOUT_SECS).await;

        let (url, reader) = match url {
            Some((u, r)) => (u, r),
            None => {
                let _ = child.start_kill();
                return Err(Error::UrlTimeout(URL_WAIT_TIMEOUT_SECS));
            }
        };

        info!("Tunnel ready: {url} (pid={pid:?})");

        let info = TunnelInfo::new(url, local_port);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let inner_clone = Arc::clone(&self.inner);
        let config = MonitorConfig {
            binary: binary.to_path_buf(),
            local_port,
            event_tx: self.event_tx.clone(),
            retry_backoffs: self.retry_backoffs.clone(),
        };
        let monitor_handle = tokio::spawn(monitor_task(
            child,
            reader,
            shutdown_rx,
            inner_clone,
            config,
        ));

        let mut inner = self.inner.lock().await;
        inner.info = Some(info.clone());
        inner.running = Some(RunningState {
            shutdown_tx,
            monitor_handle,
        });

        let _ = self.event_tx.send(TunnelEvent::Started {
            url: info.public_url.clone(),
        });

        Ok(info)
    }
}

fn spawn_cloudflared(binary: &Path, local_port: u16) -> Result<Child, std::io::Error> {
    Command::new(binary)
        .args(["tunnel", "--url", &format!("http://localhost:{local_port}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

async fn wait_for_url(
    mut reader: BufReader<tokio::process::ChildStderr>,
    timeout_secs: u64,
) -> Option<(String, BufReader<tokio::process::ChildStderr>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        let result = tokio::time::timeout_at(deadline, reader.read_line(&mut line_buf)).await;

        match result {
            Ok(Ok(0)) => return None,
            Ok(Ok(_)) => {
                debug!("cloudflared: {}", line_buf.trim());
                if let Some(url) = parse_tunnel_url(&line_buf) {
                    return Some((url, reader));
                }
            }
            Ok(Err(_)) => return None,
            Err(_) => return None,
        }
    }
}

struct MonitorConfig {
    binary: PathBuf,
    local_port: u16,
    event_tx: broadcast::Sender<TunnelEvent>,
    retry_backoffs: Vec<Duration>,
}

enum RestartResult {
    Success(Box<Child>, BufReader<tokio::process::ChildStderr>, String),
    Exhausted,
    ShutdownRequested,
}

async fn restart_with_retry(
    binary: &Path,
    local_port: u16,
    shutdown_rx: &mut watch::Receiver<bool>,
    backoffs: &[Duration],
) -> RestartResult {
    for (attempt, backoff) in backoffs.iter().enumerate() {
        info!(
            "Tunnel restart attempt {}/{} in {:?}",
            attempt + 1,
            backoffs.len(),
            backoff,
        );

        tokio::select! {
            _ = tokio::time::sleep(*backoff) => {}
            _ = shutdown_rx.changed() => {
                info!("Shutdown requested during restart backoff");
                return RestartResult::ShutdownRequested;
            }
        }

        match spawn_cloudflared(binary, local_port) {
            Ok(mut child) => {
                let stderr = match child.stderr.take() {
                    Some(s) => s,
                    None => {
                        warn!("Restart attempt {}: no stderr available", attempt + 1);
                        continue;
                    }
                };
                let reader = BufReader::new(stderr);
                match wait_for_url(reader, URL_WAIT_TIMEOUT_SECS).await {
                    Some((url, reader)) => {
                        return RestartResult::Success(Box::new(child), reader, url);
                    }
                    None => {
                        warn!("Restart attempt {}: no URL produced", attempt + 1);
                        let _ = child.start_kill();
                    }
                }
            }
            Err(e) => {
                warn!("Restart attempt {}: spawn failed: {e}", attempt + 1);
            }
        }
    }
    RestartResult::Exhausted
}

async fn monitor_task(
    mut child: Child,
    reader: BufReader<tokio::process::ChildStderr>,
    mut shutdown_rx: watch::Receiver<bool>,
    inner: Arc<Mutex<DriverInner>>,
    config: MonitorConfig,
) {
    let mut current_reader = reader;

    loop {
        let was_shutdown = drain_stderr(current_reader, &mut shutdown_rx).await;

        if was_shutdown {
            let _ = child.start_kill();
            break;
        }

        let status = child.wait().await;
        let rc = status.ok().and_then(|s| s.code());
        warn!("cloudflared exited with code {rc:?}, attempting restart with backoff");

        {
            let mut guard = inner.lock().await;
            guard.info = None;
        }

        match restart_with_retry(
            &config.binary,
            config.local_port,
            &mut shutdown_rx,
            &config.retry_backoffs,
        )
        .await
        {
            RestartResult::Success(new_child, new_reader, url) => {
                info!("Tunnel restarted successfully: {url}");
                let info = TunnelInfo::new(url.clone(), config.local_port);
                {
                    let mut guard = inner.lock().await;
                    guard.info = Some(info);
                }
                let _ = config.event_tx.send(TunnelEvent::Restarted { url });
                child = *new_child;
                current_reader = new_reader;
            }
            RestartResult::Exhausted => {
                warn!("All tunnel restart attempts exhausted");
                let _ = config.event_tx.send(TunnelEvent::AllRetriesFailed);
                break;
            }
            RestartResult::ShutdownRequested => {
                break;
            }
        }
    }

    let mut guard = inner.lock().await;
    guard.info = None;
    guard.running = None;
}

async fn drain_stderr(
    reader: BufReader<tokio::process::ChildStderr>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let mut lines = reader.lines();
    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        debug!("cloudflared: {line}");
                    }
                    Ok(None) | Err(_) => return false,
                }
            }
            _ = shutdown_rx.changed() => return true,
        }
    }
}

#[doc(hidden)]
pub fn parse_tunnel_url(line: &str) -> Option<String> {
    let marker = ".trycloudflare.com";
    let marker_pos = line.find(marker)?;
    let before = &line[..marker_pos];
    let https_pos = before.rfind("https://")?;

    let subdomain_start = https_pos + "https://".len();
    let subdomain = &line[subdomain_start..marker_pos];
    if subdomain.is_empty()
        || !subdomain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }

    let url_end = marker_pos + marker.len();
    Some(line[https_pos..url_end].to_string())
}
