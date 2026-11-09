use arc_swap::ArcSwap;
use futures::StreamExt;
use rua_config::loader::load_config;
use rua_config::types::AgentsConfig;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError};
use rua_workspace::registry::WorkspaceRegistry;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::twilio;

#[derive(Debug, Clone)]
pub struct VoiceSession {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub provider_type: String,
    pub provider_id: String,
    pub model: String,
    pub voice: String,
    pub speed: f64,
    pub language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub provider_type: String,
    pub provider_id: String,
    pub model: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VoiceWsToken {
    pub token: String,
    pub call_sid: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
struct ProviderCreds {
    base_url: String,
    api_key: String,
}

const WS_TOKEN_TTL_SECS: i64 = 300;

pub const MAX_PENDING_WS_TOKENS: usize = 128;

pub struct VoiceManager {
    enabled: AtomicBool,
    outbound: RwLock<HashMap<String, tokio::sync::mpsc::Sender<String>>>,
    sessions: RwLock<HashMap<String, VoiceSession>>,
    tokens: RwLock<HashMap<String, VoiceWsToken>>,
    token_order: RwLock<VecDeque<String>>,
    token_secret: String,
    audio_mode: Arc<ArcSwap<String>>,
    tts_config: Arc<ArcSwap<TtsConfig>>,
    asr_config: Arc<ArcSwap<AsrConfig>>,
    secret_dir: PathBuf,
    workspace_registry: Arc<dyn WorkspaceRegistry>,
    agent_id: String,
}

impl VoiceManager {
    pub fn new(
        token_secret: String,
        workspace_registry: Arc<dyn WorkspaceRegistry>,
        agent_id: String,
    ) -> Result<Self> {
        Self::with_dirs(
            token_secret,
            rua_config::dirs::rua_root(),
            rua_config::dirs::resolve_secret_dir(),
            workspace_registry,
            agent_id,
        )
    }

    pub fn with_dirs(
        token_secret: String,
        data_dir: PathBuf,
        secret_dir: PathBuf,
        workspace_registry: Arc<dyn WorkspaceRegistry>,
        agent_id: String,
    ) -> Result<Self> {
        let root = load_config(&data_dir.join("config.json"), &data_dir).map_err(|error| {
            RuaError::Config {
                message: error.to_string(),
            }
        })?;
        let audio_mode = extract_audio_mode(&root.agents);
        let tts = extract_tts_config(&root.agents);
        let asr = extract_asr_config(&root.agents);
        Ok(Self {
            enabled: AtomicBool::new(true),
            outbound: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
            token_order: RwLock::new(VecDeque::new()),
            token_secret,
            audio_mode: Arc::new(ArcSwap::from_pointee(audio_mode)),
            tts_config: Arc::new(ArcSwap::from_pointee(tts)),
            asr_config: Arc::new(ArcSwap::from_pointee(asr)),
            secret_dir,
            workspace_registry,
            agent_id,
        })
    }

    pub fn with_configs(
        token_secret: String,
        tts: TtsConfig,
        asr: AsrConfig,
        secret_dir: PathBuf,
        workspace_registry: Arc<dyn WorkspaceRegistry>,
        agent_id: String,
    ) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            outbound: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
            token_order: RwLock::new(VecDeque::new()),
            token_secret,
            audio_mode: Arc::new(ArcSwap::from_pointee("auto".to_string())),
            tts_config: Arc::new(ArcSwap::from_pointee(tts)),
            asr_config: Arc::new(ArcSwap::from_pointee(asr)),
            secret_dir,
            workspace_registry,
            agent_id,
        }
    }

    pub fn store_tts_config(&self, cfg: TtsConfig) {
        self.tts_config.store(Arc::new(cfg));
    }

    pub fn store_asr_config(&self, cfg: AsrConfig) {
        self.asr_config.store(Arc::new(cfg));
    }

    pub fn update_config(&self, root: &rua_config::types::RootConfig) {
        self.set_audio_mode(extract_audio_mode(&root.agents));
        self.store_tts_config(extract_tts_config(&root.agents));
        self.store_asr_config(extract_asr_config(&root.agents));
    }

    async fn resolve_runtime(&self) -> Result<Arc<dyn AgentRuntime>> {
        let ws_rt = self
            .workspace_registry
            .get_or_create(&self.agent_id)
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("failed to get workspace runtime for voice: {e}"),
            })?;
        ws_rt
            .react_runtime()
            .await
            .ok_or_else(|| RuaError::Runtime {
                message: "react_runtime not available for voice agent".into(),
            })
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub async fn attach_outbound(&self, call_sid: &str) -> tokio::sync::mpsc::Receiver<String> {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        self.outbound.write().await.insert(call_sid.into(), tx);
        rx
    }

    pub async fn send_to_call(&self, call_sid: &str, text: String) -> Result<()> {
        let sender = self
            .outbound
            .read()
            .await
            .get(call_sid)
            .cloned()
            .ok_or_else(|| RuaError::NotFound {
                resource: "voice_session".into(),
                id: call_sid.into(),
            })?;
        sender.send(text).await.map_err(|_| RuaError::Runtime {
            message: "voice call closed".into(),
        })
    }

    pub async fn stop_calls(&self) {
        self.set_enabled(false);
        self.outbound.write().await.clear();
        self.sessions.write().await.clear();
        self.tokens.write().await.clear();
        self.token_order.write().await.clear();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn generate_token(&self, call_sid: &str) -> String {
        let nonce = uuid::Uuid::new_v4();
        let mut hasher = Sha256::new();
        hasher.update(self.token_secret.as_bytes());
        hasher.update(call_sid.as_bytes());
        hasher.update(nonce.as_bytes());
        let hash = hasher.finalize();
        format!("{:x}", hash)
    }

    pub async fn handle_incoming(&self, call_sid: &str, ws_url: &str) -> Result<String> {
        let token = self.generate_token(call_sid);
        let expires_at = chrono::Utc::now().timestamp() + WS_TOKEN_TTL_SECS;
        self.insert_pending_token(token.clone(), call_sid.to_string(), expires_at)
            .await;
        tracing::info!(call_sid = %call_sid, "generated ws token for incoming call");
        let ws_url_with_token = twilio::append_token(ws_url, &token);
        let tts_cfg = self.tts_config.load();
        let asr_cfg = self.asr_config.load();
        Ok(twilio::render_connect_twiml(
            &ws_url_with_token,
            tts_cfg.language.as_deref(),
            asr_cfg.language.as_deref(),
        ))
    }

    pub async fn handle_incoming_with_config(
        &self,
        call_sid: &str,
        config: &serde_json::Value,
    ) -> Result<String> {
        let public_url = config["public_url"].as_str().unwrap_or("");
        let ws_url = twilio::public_ws_url(public_url)?;
        let token = self.generate_token(call_sid);
        self.insert_pending_token(
            token.clone(),
            call_sid.to_string(),
            chrono::Utc::now().timestamp() + WS_TOKEN_TTL_SECS,
        )
        .await;
        Ok(twilio::render_channel_twiml(
            &twilio::append_token(&ws_url, &token),
            config,
        ))
    }

    pub async fn validate_ws_token(&self, token: &str) -> Result<String> {
        let mut tokens = self.tokens.write().await;
        match tokens.remove(token) {
            Some(ws_token) => {
                self.token_order
                    .write()
                    .await
                    .retain(|queued| queued != token);
                let now = chrono::Utc::now().timestamp();
                if now > ws_token.expires_at {
                    return Err(RuaError::Security {
                        message: "voice ws token expired".into(),
                    });
                }
                Ok(ws_token.call_sid)
            }
            None => Err(RuaError::Security {
                message: "invalid voice ws token".into(),
            }),
        }
    }

    async fn insert_pending_token(&self, token: String, call_sid: String, expires_at: i64) {
        let mut tokens = self.tokens.write().await;
        let mut token_order = self.token_order.write().await;
        tokens.insert(
            token.clone(),
            VoiceWsToken {
                token: token.clone(),
                call_sid,
                expires_at,
            },
        );
        token_order.push_back(token);
        while tokens.len() > MAX_PENDING_WS_TOKENS {
            match token_order.pop_front() {
                Some(oldest) => {
                    tokens.remove(&oldest);
                }
                None => break,
            }
        }
        while let Some(oldest) = token_order.front() {
            if tokens.contains_key(oldest) {
                break;
            }
            token_order.pop_front();
        }
    }

    pub async fn create_session(&self, call_sid: &str, from: &str, to: &str) -> Result<()> {
        self.sessions.write().await.insert(
            call_sid.to_string(),
            VoiceSession {
                from: from.to_string(),
                to: to.to_string(),
            },
        );
        tracing::info!(call_sid = %call_sid, from = %from, to = %to, "voice session created");
        Ok(())
    }

    pub async fn process_prompt(&self, call_sid: &str, prompt: &str) -> Result<String> {
        self.process_prompt_with_cancel(call_sid, prompt, None)
            .await
    }

    pub async fn process_prompt_with_cancel(
        &self,
        call_sid: &str,
        prompt: &str,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<String> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(call_sid) {
            return Err(RuaError::NotFound {
                resource: "voice_session".into(),
                id: call_sid.into(),
            });
        }
        let user_id = sessions
            .get(call_sid)
            .map(|session| session.from.clone())
            .unwrap_or_else(|| "voice".into());
        drop(sessions);
        tracing::debug!(call_sid = %call_sid, "processing voice prompt");

        let req = AgentRequest {
            session_id: call_sid.into(),
            user_id,
            agent_id: self.agent_id.clone().into(),
            messages: vec![AgentInputMessage {
                role: "user".into(),
                content: vec![ContentPart::Text {
                    text: prompt.to_string(),
                }],
            }],
            model_override: None,
            tool_mode: "auto".into(),
            attachments: vec![],
            display_preferences: None,
            disabled_tools: vec![],
            metadata: Default::default(),
            cancel: cancel.clone(),
            run_control: None,
            workspace_dir: None,
            conversation_store_dir: None,
            working_dir: None,
        };

        let agent_runtime = match self.resolve_runtime().await {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!(call_sid = %call_sid, error = %e, "failed to resolve agent runtime");
                return Ok("抱歉，处理出错了".to_string());
            }
        };

        let mut stream = match agent_runtime.stream(req).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(call_sid = %call_sid, error = %e, "agent stream failed");
                return Ok("抱歉，处理出错了".to_string());
            }
        };

        let mut reply = String::new();
        while let Some(event) = stream.next().await {
            if let Some(ref rx) = cancel {
                if *rx.borrow() {
                    tracing::info!(call_sid = %call_sid, "voice prompt cancelled");
                    break;
                }
            }
            match event {
                Ok(RuntimeEvent::TextDelta { delta, .. }) => {
                    reply.push_str(&delta);
                }
                Ok(RuntimeEvent::Error { error }) => {
                    tracing::warn!(call_sid = %call_sid, error = %error, "agent error event");
                    if reply.is_empty() {
                        return Ok("抱歉，处理出错了".to_string());
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(call_sid = %call_sid, error = %e, "agent stream item error");
                    if reply.is_empty() {
                        return Ok("抱歉，处理出错了".to_string());
                    }
                }
            }
        }

        if reply.is_empty() {
            reply = "抱歉，处理出错了".to_string();
        }

        Ok(reply)
    }

    pub async fn handle_interrupt(&self, call_sid: &str) {
        tracing::info!(call_sid = %call_sid, "voice interrupt: cancelling LLM stream");
        let session_id = SessionId::from(call_sid);
        match self.resolve_runtime().await {
            Ok(rt) => {
                if let Err(e) = rt.cancel(&session_id).await {
                    tracing::warn!(call_sid = %call_sid, error = %e, "agent cancel failed (may already be done)");
                }
            }
            Err(e) => {
                tracing::warn!(call_sid = %call_sid, error = %e, "failed to resolve runtime for interrupt");
            }
        }
    }

    pub async fn handle_dtmf(&self, call_sid: &str, digit: &str) -> Result<String> {
        let prompt = format!("[DTMF] {digit}");
        tracing::info!(call_sid = %call_sid, digit = %digit, "voice dtmf: forwarding to agent");
        self.process_prompt(call_sid, &prompt).await
    }

    pub async fn handle_status_callback(&self, call_sid: &str, call_status: &str) -> Result<()> {
        if twilio::is_terminal_status(call_status) {
            self.outbound.write().await.remove(call_sid);
            let removed = self.sessions.write().await.remove(call_sid);
            if removed.is_some() {
                tracing::info!(
                    call_sid = %call_sid,
                    status = %call_status,
                    "voice session cleaned up"
                );
            }
        } else {
            tracing::debug!(
                call_sid = %call_sid,
                status = %call_status,
                "voice status callback (non-terminal)"
            );
        }
        Ok(())
    }

    pub fn get_audio_mode(&self) -> String {
        (**self.audio_mode.load()).clone()
    }

    pub fn set_audio_mode(&self, mode: String) {
        self.audio_mode.store(Arc::new(mode));
    }

    pub fn get_tts_config(&self) -> TtsConfig {
        (**self.tts_config.load()).clone()
    }

    pub fn get_asr_config(&self) -> AsrConfig {
        (**self.asr_config.load()).clone()
    }

    pub async fn synthesize_tts(&self, text: &str) -> Result<Vec<u8>> {
        self.synthesize_speech(text, None, None, None).await
    }

    pub async fn synthesize_speech(
        &self,
        text: &str,
        model: Option<&str>,
        voice: Option<&str>,
        speed: Option<f64>,
    ) -> Result<Vec<u8>> {
        let tts_cfg = self.tts_config.load();
        let creds = load_provider_creds(&self.secret_dir, &tts_cfg.provider_id)?;
        let model = model.unwrap_or(&tts_cfg.model);
        let voice = voice.unwrap_or(&tts_cfg.voice);
        let speed = speed.unwrap_or(tts_cfg.speed);
        send_tts_request(&creds, model, voice, speed, text).await
    }

    pub async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        filename: &str,
        model: Option<&str>,
    ) -> Result<String> {
        let asr_cfg = self.asr_config.load();
        if asr_cfg.provider_type == "disabled" {
            return Err(RuaError::NotImplemented {
                feature: "audio transcription is disabled".into(),
            });
        }
        if asr_cfg.provider_type == "local_whisper" {
            return Err(RuaError::NotImplemented {
                feature: "local whisper transcription is not available".into(),
            });
        }
        let provider_id = if asr_cfg.provider_id.is_empty() {
            self.tts_config.load().provider_id.clone()
        } else {
            asr_cfg.provider_id.clone()
        };
        let creds = load_provider_creds(&self.secret_dir, &provider_id)?;
        let model = model.unwrap_or(&asr_cfg.model);
        send_asr_request(&creds, model, &audio_bytes, filename).await
    }

    pub async fn token_count(&self) -> usize {
        self.tokens.read().await.len()
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    #[doc(hidden)]
    pub async fn inject_token_for_test(&self, token: &str, expires_at: i64) {
        self.insert_pending_token(token.to_string(), token.to_string(), expires_at)
            .await;
    }
}

pub(crate) fn extract_audio_mode(agents: &AgentsConfig) -> String {
    agents
        .extra
        .get("audio_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string()
}

pub(crate) fn extract_tts_config(agents: &AgentsConfig) -> TtsConfig {
    TtsConfig {
        provider_type: agents
            .extra
            .get("tts_provider_type")
            .and_then(|v| v.as_str())
            .unwrap_or("disabled")
            .to_string(),
        provider_id: agents
            .extra
            .get("tts_provider_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        model: agents
            .extra
            .get("tts_model")
            .and_then(|v| v.as_str())
            .unwrap_or("tts-1")
            .to_string(),
        voice: agents
            .extra
            .get("tts_voice")
            .and_then(|v| v.as_str())
            .unwrap_or("alloy")
            .to_string(),
        speed: agents
            .extra
            .get("tts_speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0),
        language: agents
            .extra
            .get("tts_language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

pub(crate) fn extract_asr_config(agents: &AgentsConfig) -> AsrConfig {
    AsrConfig {
        provider_type: agents
            .extra
            .get("transcription_provider_type")
            .and_then(|v| v.as_str())
            .unwrap_or("disabled")
            .to_string(),
        provider_id: agents
            .extra
            .get("transcription_provider_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        model: agents
            .extra
            .get("transcription_model")
            .and_then(|v| v.as_str())
            .unwrap_or("whisper-1")
            .to_string(),
        language: agents
            .extra
            .get("speech_language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

fn load_provider_creds(secret_dir: &Path, provider_id: &str) -> Result<ProviderCreds> {
    if provider_id.is_empty() {
        return Err(RuaError::Config {
            message: "no provider_id configured".into(),
        });
    }
    let path = secret_dir
        .join("providers")
        .join("providers")
        .join(format!("{provider_id}.json"));
    let content = std::fs::read_to_string(&path).map_err(|e| RuaError::Config {
        message: format!("failed to read provider config {}: {e}", path.display()),
    })?;
    let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| RuaError::Config {
        message: format!("failed to parse provider config: {e}"),
    })?;
    let base_url = val
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let api_key = val
        .get("api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if base_url.is_empty() {
        return Err(RuaError::Config {
            message: format!("provider '{provider_id}' has no base_url configured"),
        });
    }
    Ok(ProviderCreds { base_url, api_key })
}

async fn send_tts_request(
    creds: &ProviderCreds,
    model: &str,
    voice: &str,
    speed: f64,
    text: &str,
) -> Result<Vec<u8>> {
    let base_url = creds.base_url.trim_end_matches('/');
    let url = format!("{base_url}/audio/speech");

    let payload = serde_json::json!({
        "model": model,
        "input": text,
        "voice": voice,
        "speed": speed,
        "response_format": "mp3",
    });

    let client = rua_http::default_client_or_default("rua-voice");
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload);
    if !creds.api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", creds.api_key));
    }

    let resp = req.send().await.map_err(|e| RuaError::Provider {
        provider: "tts".into(),
        retryable: true,
        message: format!("TTS request failed: {e}"),
    })?;

    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.text().await.unwrap_or_default();
        return Err(RuaError::Provider {
            provider: "tts".into(),
            retryable: status >= 500,
            message: format!(
                "TTS API returned {status}: {}",
                &body[..body.len().min(500)]
            ),
        });
    }

    let bytes = resp.bytes().await.map_err(|e| RuaError::Provider {
        provider: "tts".into(),
        retryable: false,
        message: format!("failed to read TTS response body: {e}"),
    })?;

    Ok(bytes.to_vec())
}

async fn send_asr_request(
    creds: &ProviderCreds,
    model: &str,
    audio_bytes: &[u8],
    filename: &str,
) -> Result<String> {
    let base_url = creds.base_url.trim_end_matches('/');
    let url = format!("{base_url}/audio/transcriptions");

    let (boundary, body) = twilio::build_multipart_body(audio_bytes, filename, model);

    let client = rua_http::default_client_or_default("rua-voice");
    let mut req = client
        .post(&url)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body);
    if !creds.api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", creds.api_key));
    }

    let resp = req.send().await.map_err(|e| RuaError::Provider {
        provider: "asr".into(),
        retryable: true,
        message: format!("ASR request failed: {e}"),
    })?;

    let status = resp.status().as_u16();
    if status != 200 {
        let resp_body = resp.text().await.unwrap_or_default();
        return Err(RuaError::Provider {
            provider: "asr".into(),
            retryable: status >= 500,
            message: format!(
                "ASR API returned {status}: {}",
                &resp_body[..resp_body.len().min(500)]
            ),
        });
    }

    let resp_json: serde_json::Value = resp.json().await.map_err(|e| RuaError::Provider {
        provider: "asr".into(),
        retryable: false,
        message: format!("failed to parse ASR response: {e}"),
    })?;

    let text = resp_json
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(text)
}
