//! Server-owned Twilio webhook and tunnel lifecycle.
use rua_core::{Result, RuaError};
use rua_tunnel::{BinaryManager, CloudflareTunnelDriver, TunnelEvent};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc,
};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

#[derive(Clone)]
pub(crate) struct VoiceApplicationContext {
    pub agent_cells: Arc<rua_agent::AgentCells>,
    pub voice: Arc<rua_voice::VoiceManager>,
    pub tunnel_driver: Arc<Mutex<Option<CloudflareTunnelDriver>>>,
    pub rua_root: std::path::PathBuf,
}

impl VoiceApplicationContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            agent_cells: state.agent_cells.clone(),
            voice: state.voice.clone(),
            tunnel_driver: state.tunnel_driver.clone(),
            rua_root: state.rua_root.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct VoiceRuntime {
    port: AtomicU16,
    inner: Mutex<RuntimeState>,
    pub_url: Arc<RwLock<Option<String>>>,
    error: Arc<RwLock<Option<String>>>,
}

#[derive(Default)]
struct RuntimeState {
    config: Option<Value>,
    task: Option<(CancellationToken, tokio::task::JoinHandle<()>)>,
    owned_tunnel: Option<broadcast::Receiver<TunnelEvent>>,
}

impl VoiceRuntime {
    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub async fn public_url(&self) -> Option<String> {
        self.pub_url.read().await.clone()
    }

    pub async fn error(&self) -> Option<String> {
        self.error.read().await.clone()
    }

    pub async fn reload(&self, state: &AppState) -> Result<()> {
        self.apply(&VoiceApplicationContext::from_state(state))
            .await
    }

    pub async fn apply(&self, state: &VoiceApplicationContext) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let saved = state
            .agent_cells
            .section::<Value>(state.voice.agent_id(), "channels")
            .read()
            .await
            .map_err(|error| RuaError::Config {
                message: format!("voice channel config: {error}"),
            })?;
        let config = rua_config::channel_catalog::resolve_channel_config(
            "voice",
            &saved
                .as_ref()
                .and_then(|v| v.get("voice"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        );
        let config = config.map_err(|message| RuaError::Config { message })?;
        if config["enabled"] != true {
            state.voice.stop_calls().await;
        }
        if inner.config.as_ref() == Some(&config) && self.error.read().await.is_none() {
            state.voice.set_enabled(config["enabled"] == true);
            return Ok(());
        }
        self.stop_inner(state, &mut inner).await;
        *self.error.write().await = None;
        let result = self.start_inner(state, &mut inner, config.clone()).await;
        if let Err(error) = &result {
            self.stop_inner(state, &mut inner).await;
            *self.error.write().await = Some(error.to_string());
        } else {
            inner.config = Some(config);
        }
        result
    }

    pub async fn stop(&self, state: &AppState) {
        let mut inner = self.inner.lock().await;
        state.voice.stop_calls().await;
        self.stop_inner(&VoiceApplicationContext::from_state(state), &mut inner)
            .await;
    }

    async fn stop_inner(&self, state: &VoiceApplicationContext, inner: &mut RuntimeState) {
        inner.config = None;
        state.voice.set_enabled(false);
        if let Some((cancel, mut task)) = inner.task.take() {
            cancel.cancel();
            if tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        if let Some(owned) = inner.owned_tunnel.take() {
            let mut driver = state.tunnel_driver.lock().await;
            if let Some(current) = driver.as_ref() {
                // Do not stop a tunnel the user has replaced through the tunnel API.
                if owned.same_channel(&current.subscribe_events()) {
                    current.stop().await;
                    *driver = None;
                }
            }
        }
        *self.pub_url.write().await = None;
    }

    async fn start_inner(
        &self,
        state: &VoiceApplicationContext,
        inner: &mut RuntimeState,
        mut config: Value,
    ) -> Result<()> {
        if config["enabled"] != true {
            return Ok(());
        }
        let automatic = config["public_url"]
            .as_str()
            .unwrap_or("")
            .trim()
            .is_empty();
        let provision = !config["phone_number_sid"]
            .as_str()
            .unwrap_or("")
            .trim()
            .is_empty();
        let mut events = None;
        if automatic {
            for key in [
                "phone_number_sid",
                "twilio_account_sid",
                "twilio_auth_token",
            ] {
                if config[key].as_str().unwrap_or("").trim().is_empty() {
                    return Err(RuaError::Validation {
                        message: format!("voice {key} is required for automatic webhook setup"),
                    });
                }
            }
            let port = self.port.load(Ordering::Relaxed);
            let mut guard = state.tunnel_driver.lock().await;
            if guard.is_none() || !guard.as_ref().expect("checked driver").is_running().await {
                if port == 0 {
                    return Err(RuaError::Runtime {
                        message: "voice is waiting for the HTTP listener".into(),
                    });
                }
                let driver =
                    CloudflareTunnelDriver::new(BinaryManager::new(state.rua_root.join("tunnel")));
                let receiver = driver.subscribe_events();
                driver
                    .start(port)
                    .await
                    .map_err(|error| RuaError::Runtime {
                        message: format!("voice tunnel: {error}"),
                    })?;
                inner.owned_tunnel = Some(receiver.resubscribe());
                events = Some(receiver);
                *guard = Some(driver);
            }
            let driver = guard.as_ref().expect("started tunnel");
            let info = driver.get_info().await.ok_or_else(|| RuaError::Runtime {
                message: "voice tunnel is unavailable".into(),
            })?;
            if port != 0 && info.local_port != port {
                return Err(RuaError::Validation {
                    message: "running tunnel does not point to this server's HTTP port".into(),
                });
            }
            config["public_url"] = serde_json::json!(info.public_url);
            events.get_or_insert_with(|| driver.subscribe_events());
        }
        rua_voice::twilio::public_ws_url(config["public_url"].as_str().unwrap_or(""))?;
        if provision {
            rua_voice::twilio::configure_voice_webhook(&config).await?;
        }
        *self.pub_url.write().await = config["public_url"].as_str().map(str::to_owned);
        state.voice.set_enabled(true);
        if let Some(mut events) = events {
            let cancel = CancellationToken::new();
            let child = cancel.child_token();
            let pub_url = self.pub_url.clone();
            let error = self.error.clone();
            let manager = state.voice.clone();
            inner.task = Some((
                cancel,
                tokio::spawn(async move {
                    loop {
                        let event = tokio::select! {
                            _ = child.cancelled() => break,
                            event = events.recv() => event,
                        };
                        match event {
                            Ok(TunnelEvent::Started { url } | TunnelEvent::Restarted { url }) => {
                                if config["public_url"].as_str() == Some(&url) {
                                    continue;
                                }
                                config["public_url"] = serde_json::json!(url);
                                *pub_url.write().await = Some(url);
                                let update = tokio::select! {
                                    _ = child.cancelled() => break,
                                    update = rua_voice::twilio::configure_voice_webhook(&config) => update,
                                };
                                match update {
                                    Ok(()) => {
                                        *error.write().await = None;
                                        manager.set_enabled(true);
                                    }
                                    Err(failure) => {
                                        manager.set_enabled(false);
                                        *error.write().await = Some(failure.to_string());
                                        tracing::warn!(%failure,"voice webhook refresh failed");
                                    }
                                }
                            }
                            Ok(TunnelEvent::AllRetriesFailed) => {
                                manager.stop_calls().await;
                                *error.write().await =
                                    Some("voice tunnel exhausted its reconnect attempts".into());
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => {
                                manager.stop_calls().await;
                                *error.write().await = Some("voice tunnel stopped".into());
                                break;
                            }
                        }
                    }
                }),
            ));
        }
        Ok(())
    }
}
