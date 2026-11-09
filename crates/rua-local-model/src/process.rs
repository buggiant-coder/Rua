use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use rua_core::{Result, RuaError};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub enum ProcessState {
    Running,
    Exited { code: Option<i32> },
}

pub struct LlamaProcess {
    child: Arc<StdMutex<Option<Child>>>,
    pub pid: u32,
    pub port: u16,
    pub model: Option<String>,
    state: Arc<StdMutex<ProcessState>>,
    watchdog_handle: Option<JoinHandle<()>>,
}

impl LlamaProcess {
    pub fn spawn(
        binary: &str,
        port: u16,
        model_path: Option<&str>,
        model_id: Option<String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(binary);
        cmd.arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string());
        if let Some(mp) = model_path {
            cmd.arg("--model").arg(mp);
        }
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| RuaError::Io {
            message: format!("failed to start llama-server: {e}"),
        })?;

        let pid = child.id().ok_or_else(|| RuaError::Runtime {
            message: "llama-server process exited immediately".into(),
        })?;

        Ok(Self {
            child: Arc::new(StdMutex::new(Some(child))),
            pid,
            port,
            model: model_id,
            state: Arc::new(StdMutex::new(ProcessState::Running)),
            watchdog_handle: None,
        })
    }

    pub fn start_watchdog(&mut self, interval: Duration) {
        let child = self.child.clone();
        let state = self.state.clone();
        let pid = self.pid;

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let exited = {
                    let mut guard = match child.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    match guard.as_mut() {
                        Some(c) => match c.try_wait() {
                            Ok(Some(status)) => {
                                if let Ok(mut s) = state.lock() {
                                    *s = ProcessState::Exited {
                                        code: status.code(),
                                    };
                                }
                                true
                            }
                            Ok(None) => false,
                            Err(_) => {
                                if let Ok(mut s) = state.lock() {
                                    *s = ProcessState::Exited { code: None };
                                }
                                true
                            }
                        },
                        None => return,
                    }
                };

                if exited {
                    tracing::warn!(pid, "llama-server process exited unexpectedly");
                    return;
                }
            }
        });

        self.watchdog_handle = Some(handle);
    }

    pub fn is_healthy(&self) -> bool {
        self.state
            .lock()
            .map(|s| matches!(*s, ProcessState::Running))
            .unwrap_or(false)
    }

    pub async fn shutdown(mut self) {
        if let Some(handle) = self.watchdog_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        let child = self.child.lock().ok().and_then(|mut g| g.take());
        if let Some(mut c) = child {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
    }
}

impl Drop for LlamaProcess {
    fn drop(&mut self) {
        if let Some(handle) = self.watchdog_handle.take() {
            handle.abort();
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(ref mut c) = *guard {
                let _ = c.start_kill();
            }
        }
    }
}

pub async fn wait_for_health(port: u16, timeout: Duration) -> Result<()> {
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = rua_http::build_client(reqwest::Client::builder().timeout(Duration::from_secs(2)))
        .map_err(|e| RuaError::Runtime {
            message: format!("failed to create HTTP client: {e}"),
        })?;

    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    Err(RuaError::Timeout {
        operation: format!("llama-server health check on port {port}"),
    })
}
