use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use rua_config::types::acp::{AcpAgentConfig, AcpConfig};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing;

use crate::client::{AcpClientAdapter, AcpConnectionContext, TurnCommand};
use crate::core::{AcpError, SessionInit, SessionKey};
use crate::permissions::{
    AcpPermissionRelay, AcpPermissionRelayError, AcpPermissionSnapshot, AcpPermissionTaskContext,
    PermissionEvaluator,
};

use agent_client_protocol::schema::v1::SessionId as AcpSessionId;
use tokio_util::sync::CancellationToken;

const DEFAULT_ACP_RUNNER_SESSION_OPEN_TIMEOUT: Duration = Duration::from_secs(60);

struct SessionHandle {
    cmd_tx: mpsc::UnboundedSender<TurnCommand>,
    _connection_task: JoinHandle<()>,
    turn_lock: Arc<Mutex<()>>,

    cancel_gate: Arc<std::sync::Mutex<CancellationToken>>,
    active_turn: Arc<AtomicBool>,
    connection_alive: Arc<AtomicBool>,
    permission_scope_id: String,
}

pub struct AcpService {
    sessions: RwLock<HashMap<SessionKey, SessionHandle>>,
    config: StdRwLock<Arc<AcpConfig>>,
    default_cwd: String,
    runtime_cache_dir: PathBuf,
    evaluator: Option<Arc<dyn PermissionEvaluator>>,
    permission_relay: Arc<AcpPermissionRelay>,
    permission_task: Option<AcpPermissionTaskContext>,
    relay_service_id: String,
    agent_id: String,
    chat_address_hints: StdRwLock<HashMap<String, rua_core::ChatAddress>>,
}

pub enum TurnMode {
    Start,
    Restart,
    Message,
}

pub struct OpenSessionResumeResult {
    pub session_id: String,
    pub history_preview: Option<String>,
}

impl AcpService {
    pub fn new(config: Arc<AcpConfig>, default_cwd: String) -> Self {
        Self::with_evaluator(config, default_cwd, None, String::new())
    }

    pub fn with_evaluator(
        config: Arc<AcpConfig>,
        default_cwd: String,
        evaluator: Option<Arc<dyn PermissionEvaluator>>,
        agent_id: String,
    ) -> Self {
        let runtime_cache_dir = rua_config::dirs::Paths::new(None).cache_dir;
        Self::with_runtime_cache_dir(config, default_cwd, runtime_cache_dir, evaluator, agent_id)
    }

    pub fn with_runtime_cache_dir(
        config: Arc<AcpConfig>,
        default_cwd: String,
        runtime_cache_dir: PathBuf,
        evaluator: Option<Arc<dyn PermissionEvaluator>>,
        agent_id: String,
    ) -> Self {
        Self::with_runtime_cache_dir_and_permission_relay(
            config,
            default_cwd,
            runtime_cache_dir,
            evaluator,
            Arc::new(AcpPermissionRelay::new()),
            None,
            agent_id,
        )
    }

    pub fn with_runtime_cache_dir_and_permission_relay(
        config: Arc<AcpConfig>,
        default_cwd: String,
        runtime_cache_dir: PathBuf,
        evaluator: Option<Arc<dyn PermissionEvaluator>>,
        permission_relay: Arc<AcpPermissionRelay>,
        permission_task: Option<AcpPermissionTaskContext>,
        agent_id: String,
    ) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config: StdRwLock::new(config),
            default_cwd,
            runtime_cache_dir,
            evaluator,
            permission_relay,
            permission_task,
            relay_service_id: uuid::Uuid::new_v4().to_string(),
            agent_id,
            chat_address_hints: StdRwLock::new(HashMap::new()),
        }
    }

    pub fn bind_chat_address(&self, chat_id: &str, address: rua_core::ChatAddress) {
        self.chat_address_hints
            .write()
            .expect("ACP chat address hints lock poisoned")
            .insert(chat_id.to_string(), address);
    }

    pub async fn run_turn(
        &self,
        chat_id: SessionId,
        agent_name: &str,
        prompt: &str,
        mode: TurnMode,
        cwd: Option<String>,
    ) -> Result<mpsc::UnboundedReceiver<RuntimeEvent>, AcpError> {
        let key = SessionKey {
            chat_id: chat_id.clone(),
            agent_name: agent_name.to_string(),
        };

        let init_for_new = || SessionInit::New {
            cwd: cwd.clone().unwrap_or_else(|| self.default_cwd.clone()),
        };

        match mode {
            TurnMode::Start => {
                let exists = self.session_exists(&key).await;
                if exists {
                    return Err(AcpError::SessionAlreadyExists(key));
                }
                self.ensure_session(&key, agent_name, init_for_new())
                    .await?;
            }
            TurnMode::Restart => {
                self.close_session_inner(&key).await;
                self.ensure_session(&key, agent_name, init_for_new())
                    .await?;
            }
            TurnMode::Message => {
                let exists = self.session_exists(&key).await;
                if !exists {
                    return Err(AcpError::SessionNotFound(key));
                }
            }
        }

        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(&key)
            .ok_or_else(|| AcpError::SessionNotFound(key.clone()))?;

        let turn_guard = handle
            .turn_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| AcpError::TurnInProgress(key.clone()))?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let (started_tx, started_rx) = oneshot::channel();
        let send_result = handle.cmd_tx.send(TurnCommand::SendPrompt {
            text: prompt.to_string(),
            event_tx,
            started_tx,
        });
        drop(sessions);

        if send_result.is_err() {
            tracing::debug!(%key, "removing ACP session with closed command channel");
            self.close_session_inner(&key).await;
            return Err(AcpError::SessionClosed(key));
        }
        if started_rx.await.is_err() {
            tracing::debug!(%key, "ACP session closed before prompt start");
            self.close_session_inner(&key).await;
            return Err(AcpError::SessionClosed(key));
        }

        tokio::spawn(async move {
            drop(turn_guard);
        });

        Ok(event_rx)
    }

    pub async fn open_session(
        &self,
        chat_id: SessionId,
        agent_name: &str,
        cwd: Option<String>,
    ) -> Result<String, AcpError> {
        let key = SessionKey {
            chat_id,
            agent_name: agent_name.to_string(),
        };
        if self.session_exists(&key).await {
            return Err(AcpError::SessionAlreadyExists(key));
        }
        let init = SessionInit::New {
            cwd: cwd.unwrap_or_else(|| self.default_cwd.clone()),
        };
        self.ensure_session(&key, agent_name, init).await
    }

    pub async fn open_session_resume(
        &self,
        chat_id: SessionId,
        agent_name: &str,
        acp_session_id: AcpSessionId,
        cwd: Option<String>,
    ) -> Result<OpenSessionResumeResult, AcpError> {
        let key = SessionKey {
            chat_id,
            agent_name: agent_name.to_string(),
        };
        if self.session_exists(&key).await {
            return Err(AcpError::SessionAlreadyExists(key));
        }
        let (history_tx, history_rx) = oneshot::channel();
        let session_id = acp_session_id.to_string();
        let init = SessionInit::Load {
            session_id: acp_session_id,
            cwd: cwd.unwrap_or_else(|| self.default_cwd.clone()),
            history_tx: Some(history_tx),
        };
        self.ensure_session(&key, agent_name, init).await?;
        let timeout = DEFAULT_ACP_RUNNER_SESSION_OPEN_TIMEOUT;
        let history = match tokio::time::timeout(timeout, history_rx).await {
            Ok(Ok(preview)) => preview,
            Ok(Err(_)) => {
                self.close_session_inner(&key).await;
                return Err(AcpError::Connection(
                    "ACP session closed before load history completed".to_string(),
                ));
            }
            Err(_) => {
                tracing::warn!(%key, ?timeout, "ACP resume timed out while loading history");
                self.close_session_inner(&key).await;
                return Err(AcpError::Timeout(timeout));
            }
        };
        Ok(OpenSessionResumeResult {
            session_id,
            history_preview: Some(history.trim().to_string()).filter(|preview| !preview.is_empty()),
        })
    }

    pub fn agent_config(&self, agent_name: &str) -> Option<AcpAgentConfig> {
        self.config
            .read()
            .expect("ACP config read lock poisoned")
            .agents
            .get(agent_name)
            .cloned()
    }

    pub fn agent_configs(&self) -> Vec<(String, AcpAgentConfig)> {
        let config = self.config.read().expect("ACP config read lock poisoned");
        let mut configs: Vec<_> = config
            .agents
            .iter()
            .map(|(name, config)| (name.clone(), config.clone()))
            .collect();
        configs.sort_by(|(left, _), (right, _)| left.cmp(right));
        configs
    }

    pub fn enabled_agent_names(&self) -> Vec<String> {
        let config = self.config.read().expect("ACP config read lock poisoned");
        let mut names: Vec<String> = config
            .agents
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    pub fn replace_config(&self, config: Arc<AcpConfig>) {
        *self.config.write().expect("ACP config write lock poisoned") = config;
    }

    pub async fn permission_snapshot(&self, chat_id: &SessionId) -> AcpPermissionSnapshot {
        self.permission_relay
            .snapshot(&self.agent_id, chat_id.as_str())
            .await
    }

    pub async fn decide_permission(
        &self,
        chat_id: &SessionId,
        request_id: &str,
        option_id: &str,
    ) -> Result<crate::permissions::AcpPermissionRequest, AcpPermissionRelayError> {
        self.permission_relay
            .decide(&self.agent_id, chat_id.as_str(), request_id, option_id)
            .await
    }

    pub async fn close_chat_session(
        &self,
        chat_id: &SessionId,
        agent_name: &str,
    ) -> Result<(), AcpError> {
        let key = SessionKey {
            chat_id: chat_id.clone(),
            agent_name: agent_name.to_string(),
        };

        self.close_session_inner(&key).await;
        Ok(())
    }

    pub async fn close_all(&self) {
        let keys: Vec<SessionKey> = self.sessions.read().await.keys().cloned().collect();
        for key in keys {
            self.close_session_inner(&key).await;
        }
    }

    pub async fn cancel_turn(&self, chat_id: &SessionId, agent_name: &str) -> Result<(), AcpError> {
        let key = SessionKey {
            chat_id: chat_id.clone(),
            agent_name: agent_name.to_string(),
        };

        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(&key)
            .ok_or_else(|| AcpError::SessionNotFound(key.clone()))?;

        tracing::info!(%key, "cancel_turn requested (in-turn cancel, keep subprocess)");
        handle.cancel_gate.lock().unwrap().clone().cancel();

        Ok(())
    }

    pub async fn turn_in_progress(&self, chat_id: &SessionId, agent_name: &str) -> bool {
        let key = SessionKey {
            chat_id: chat_id.clone(),
            agent_name: agent_name.to_string(),
        };
        self.sessions
            .read()
            .await
            .get(&key)
            .is_some_and(|handle| handle.active_turn.load(Ordering::Acquire))
    }

    pub async fn chat_session_exists(&self, chat_id: &SessionId, agent_name: &str) -> bool {
        self.sessions
            .read()
            .await
            .get(&SessionKey {
                chat_id: chat_id.clone(),
                agent_name: agent_name.to_string(),
            })
            .is_some_and(|handle| handle.connection_alive.load(Ordering::Acquire))
    }

    pub async fn run_turn_with_timeout(
        &self,
        chat_id: SessionId,
        agent_name: &str,
        prompt: &str,
        mode: TurnMode,
        cwd: Option<String>,
        timeout: Duration,
    ) -> Result<mpsc::UnboundedReceiver<RuntimeEvent>, AcpError> {
        let result = tokio::time::timeout(
            timeout,
            self.run_turn(chat_id.clone(), agent_name, prompt, mode, cwd),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => {
                let key = SessionKey {
                    chat_id,
                    agent_name: agent_name.to_string(),
                };
                tracing::warn!(%key, ?timeout, "turn timed out, closing session");
                self.close_session_inner(&key).await;
                Err(AcpError::Timeout(timeout))
            }
        }
    }

    async fn ensure_session(
        &self,
        key: &SessionKey,
        agent_name: &str,
        init: SessionInit,
    ) -> Result<String, AcpError> {
        let agent_config = self
            .agent_config(agent_name)
            .ok_or_else(|| AcpError::AgentNotFound(agent_name.to_string()))?;

        if !agent_config.enabled {
            return Err(AcpError::AgentNotEnabled(agent_name.to_string()));
        }
        let runner_command = agent_config.command.clone();

        let session_cwd = session_init_cwd(&init);
        let chat_address = self
            .chat_address_hints
            .read()
            .expect("ACP chat address hints lock poisoned")
            .get(key.chat_id.as_str())
            .cloned()
            .unwrap_or_else(|| rua_core::ChatAddress::agent(&self.agent_id, key.chat_id.as_str()));
        let acp_agent = AcpClientAdapter::build_acp_agent_for_runtime(
            &agent_config,
            std::path::Path::new(&session_cwd),
            &self.runtime_cache_dir,
        )?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let key_display = key.to_string();
        let evaluator = self.evaluator.clone();
        let permission_relay = Arc::clone(&self.permission_relay);
        let agent_id = self.agent_id.clone();
        let chat_id = key.chat_id.to_string();
        let runner_name = agent_name.to_string();
        let trusted = agent_config.trusted;
        let permission_task = self.permission_task.clone();
        let permission_scope_id =
            format!("{}\0{}", self.relay_service_id, chat_address.identity_key());
        let connection_permission_scope_id = permission_scope_id.clone();
        let (startup_tx, startup_rx) = oneshot::channel();
        let cancel_gate = Arc::new(std::sync::Mutex::new(CancellationToken::new()));
        let cancel_gate_conn = Arc::clone(&cancel_gate);
        let active_turn = Arc::new(AtomicBool::new(false));
        let active_turn_conn = Arc::clone(&active_turn);
        let connection_alive = Arc::new(AtomicBool::new(true));
        let connection_alive_task = Arc::clone(&connection_alive);

        let connection_task = tokio::spawn(async move {
            let result = AcpClientAdapter::run_connection(
                acp_agent,
                init,
                cmd_rx,
                startup_tx,
                AcpConnectionContext {
                    evaluator,
                    permission_relay,
                    permission_context: crate::permissions::AcpPermissionRequestContext {
                        agent_id,
                        chat_id,
                        chat_address: Some(chat_address),
                        runner: runner_name,
                        task: permission_task,
                    },
                    trusted,
                    permission_scope_id: connection_permission_scope_id,
                    cancel_gate: cancel_gate_conn,
                    active_turn: active_turn_conn,
                },
            )
            .await;
            if let Err(e) = result {
                tracing::error!(session = %key_display, error = %e, "ACP connection ended with error");
            }
            connection_alive_task.store(false, Ordering::Release);
        });

        let handle = SessionHandle {
            cmd_tx,
            _connection_task: connection_task,
            turn_lock: Arc::new(Mutex::new(())),
            cancel_gate,
            active_turn,
            connection_alive,
            permission_scope_id,
        };

        self.sessions.write().await.insert(key.clone(), handle);
        let timeout = DEFAULT_ACP_RUNNER_SESSION_OPEN_TIMEOUT;
        match tokio::time::timeout(timeout, startup_rx).await {
            Ok(Ok(Ok(session_id))) => Ok(session_id),
            Ok(Ok(Err(error))) => {
                self.close_session_inner(key).await;
                Err(AcpError::Connection(format!(
                    "ACP runner '{agent_name}' command '{runner_command}' failed to initialize: {error}"
                )))
            }
            Ok(Err(_)) => {
                self.close_session_inner(key).await;
                Err(AcpError::Connection(
                    format!(
                        "ACP runner '{agent_name}' command '{runner_command}' closed before initialization completed"
                    ),
                ))
            }
            Err(_) => {
                tracing::warn!(%key, ?timeout, "ACP session timed out while initializing");
                self.close_session_inner(key).await;
                Err(AcpError::Timeout(timeout))
            }
        }
    }

    async fn session_exists(&self, key: &SessionKey) -> bool {
        let stale = {
            let sessions = self.sessions.read().await;
            sessions
                .get(key)
                .map(|handle| handle._connection_task.is_finished())
                .unwrap_or(false)
        };

        if stale {
            tracing::debug!(%key, "removing stale ACP session handle");
            self.close_session_inner(key).await;
            return false;
        }

        self.sessions.read().await.contains_key(key)
    }

    async fn close_session_inner(&self, key: &SessionKey) {
        let handle = self.sessions.write().await.remove(key);
        if let Some(handle) = handle {
            self.permission_relay
                .cancel_scope(&handle.permission_scope_id)
                .await;
            let _ = handle.cmd_tx.send(TurnCommand::Close);
            handle._connection_task.abort();
            // Await abort so ManagedChildGuard has terminated the process group before returning.
            let _ = handle._connection_task.await;
        }
    }
}

fn session_init_cwd(init: &SessionInit) -> String {
    match init {
        SessionInit::New { cwd } | SessionInit::Load { cwd, .. } => cwd.clone(),
    }
}

impl Drop for AcpService {
    fn drop(&mut self) {
        let permission_relay = Arc::clone(&self.permission_relay);
        let sessions = self.sessions.get_mut();
        for (key, handle) in sessions.drain() {
            let _ = handle.cmd_tx.send(TurnCommand::Close);
            handle._connection_task.abort();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let permission_relay = Arc::clone(&permission_relay);
                let scope_id = handle.permission_scope_id;
                runtime.spawn(async move {
                    permission_relay.cancel_scope(&scope_id).await;
                });
            }
            tracing::debug!(%key, "ACP session aborted on service drop");
        }
    }
}
