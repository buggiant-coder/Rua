use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{IncomingContentPart, OutgoingContentPart};
use crate::core::ChannelId;
use rua_core::Result;

use super::protocol::{
    build_message_request, build_response, header, invite_info, parse_sip_message, request_method,
    rtp_payload, InviteInfo,
};
use super::realtime::VoiceRuntime;
use super::types::{SipConfig, SipOptions};

const SIP_READ_BUF: usize = 8192;
const RTP_READ_BUF: usize = 2048;
const RTP_PAYLOAD_BYTES: usize = 160;

#[derive(Debug, Clone)]
struct ActiveCall {
    call_id: String,
    from: String,
    to: String,
    peer: SocketAddr,
    remote_rtp: Option<SocketAddr>,
    audio_path: PathBuf,
    transcript: Option<String>,
    local_uri: String,
    remote_uri: String,
    cancel: CancellationToken,
    recorder: Arc<Mutex<Option<JoinHandle<std::io::Result<()>>>>>,
}

pub struct SipChannel {
    channel_id: ChannelId,
    config: SipConfig,
    options: Option<SipOptions>,
    voice: Mutex<Option<VoiceRuntime>>,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    stop_tx: watch::Sender<bool>,
    sip_handle: RwLock<Option<JoinHandle<()>>>,
    calls: Arc<Mutex<HashMap<String, ActiveCall>>>,
}

impl SipChannel {
    pub fn new(config: SipConfig) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self {
            channel_id: "sip".into(),
            config,
            options: None,
            voice: Mutex::new(None),
            process: RwLock::new(None),
            started: RwLock::new(false),
            stop_tx,
            sip_handle: RwLock::new(None),
            calls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_options(mut self, options: SipOptions) -> Self {
        self.options = Some(options);
        self
    }

    pub fn config(&self) -> &SipConfig {
        &self.config
    }

    pub async fn active_call_count(&self) -> usize {
        if let Some(voice) = self.voice.lock().await.as_ref() {
            return voice.calls.lock().await.len();
        }
        self.calls.lock().await.len()
    }
}

#[async_trait]
impl channel::Channel for SipChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            return Ok(());
        }
        if let Some(options) = self
            .options
            .as_ref()
            .filter(|options| options.sip_mode != "native")
        {
            *self.voice.lock().await = Some(VoiceRuntime::start(options.clone(), process).await?);
            *started = true;
            return Ok(());
        }
        let socket = Arc::new(
            UdpSocket::bind(&self.config.bind_addr)
                .await
                .map_err(|err| rua_core::RuaError::Io {
                    message: format!("bind SIP UDP {}: {err}", self.config.bind_addr),
                })?,
        );
        self.stop_tx.send_replace(false);
        *self.process.write().await = Some(process.clone());
        *started = true;

        let rtp_bind_addr = self.config.rtp_bind_addr.clone();
        let media_dir = self.config.media_dir.clone();
        let public_addr = self.config.public_addr.clone();
        let username = self.config.username.clone();
        let domain = self.config.domain.clone();
        let max_call_secs = self.config.max_call_secs;
        let calls = self.calls.clone();
        let mut stop_rx = self.stop_tx.subscribe();
        let handle = tokio::spawn(async move {
            if let Err(err) = sip_loop(
                socket,
                rtp_bind_addr,
                media_dir,
                public_addr,
                username,
                domain,
                max_call_secs,
                calls,
                process,
                &mut stop_rx,
            )
            .await
            {
                tracing::error!(error = %err, "sip loop stopped with error");
            }
        });
        *self.sip_handle.write().await = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(voice) = self.voice.lock().await.take() {
            voice.stop().await;
        }
        let _ = self.stop_tx.send(true);
        if let Some(handle) = self.sip_handle.write().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        let calls = std::mem::take(&mut *self.calls.lock().await);
        for call in calls.into_values() {
            call.cancel.cancel();
            if let Some(recorder) = call.recorder.lock().await.take() {
                let _ = recorder.await;
            }
        }
        *self.process.write().await = None;
        *self.started.write().await = false;
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        if let Some(voice) = self.voice.lock().await.as_ref() {
            let text = render_text_parts(&msg.content);
            if text.trim().is_empty() {
                return Ok(());
            }
            return voice.send(&to.conversation_id, text).await;
        }
        let call = self
            .calls
            .lock()
            .await
            .get(&to.conversation_id)
            .cloned()
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "sip_call".into(),
                id: to.conversation_id.clone(),
            })?;
        for part in &msg.content {
            if let OutgoingContentPart::Audio {
                path: Some(path), ..
            } = part
            {
                let Some(remote_rtp) = call.remote_rtp else {
                    return Err(rua_core::RuaError::Validation {
                        message: format!(
                            "sip call {} has no negotiated remote RTP address",
                            call.call_id
                        ),
                    });
                };
                send_audio_file_as_rtp(path.clone(), remote_rtp).await?;
            }
        }
        let text = render_text_parts(&msg.content);
        if text.trim().is_empty() {
            return Ok(());
        }
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("bind sip send socket: {err}"),
            })?;
        let message =
            build_message_request(&call.call_id, &call.local_uri, &call.remote_uri, 2, &text);
        socket
            .send_to(message.as_bytes(), call.peer)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("send SIP MESSAGE: {err}"),
            })?;
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        build_request_from_native_value(&native, &self.config)
    }
}

#[allow(clippy::too_many_arguments)]
async fn sip_loop(
    socket: Arc<UdpSocket>,
    rtp_bind_addr: String,
    media_dir: String,
    public_addr: String,
    username: String,
    domain: String,
    max_call_secs: u64,
    calls: Arc<Mutex<HashMap<String, ActiveCall>>>,
    process: Arc<dyn Process>,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let local_addr = socket.local_addr().map_err(|err| rua_core::RuaError::Io {
        message: format!("read SIP local addr: {err}"),
    })?;
    let advertised_addr = if public_addr.trim().is_empty() {
        local_addr.to_string()
    } else {
        public_addr
    };
    let local_uri = format!(
        "sip:{}@{}",
        if username.is_empty() {
            "rua"
        } else {
            &username
        },
        if domain.is_empty() {
            advertised_addr.as_str()
        } else {
            domain.as_str()
        }
    );
    let mut buf = vec![0_u8; SIP_READ_BUF];
    loop {
        tokio::select! {
            _ = stop_rx.changed() => break,
            received = socket.recv_from(&mut buf) => {
                let (len, peer) = received.map_err(|err| rua_core::RuaError::Io {
                    message: format!("receive SIP packet: {err}"),
                })?;
                let raw = String::from_utf8_lossy(&buf[..len]).to_string();
                let Some(message) = parse_sip_message(&raw) else {
                    continue;
                };
                match request_method(&message) {
                    Some("INVITE") => {
                        if let Some(invite) = invite_info(&message) {
                            handle_invite(
                                socket.clone(),
                                peer,
                                invite,
                                &rtp_bind_addr,
                                &media_dir,
                                &local_uri,
                                max_call_secs,
                                &calls,
                            ).await?;
                        }
                    }
                    Some("BYE") => {
                        let call_id = header(&message, "call-id").unwrap_or("").to_string();
                        let response = minimal_response(200, "OK", &message);
                        let _ = socket.send_to(response.as_bytes(), peer).await;
                        let call = calls.lock().await.remove(&call_id);
                        if let Some(call) = call {
                            if call.peer != peer {
                                calls.lock().await.insert(call_id, call);
                                continue;
                            }
                            call.cancel.cancel();
                            if let Some(recorder) = call.recorder.lock().await.take() {
                                if let Err(error) = recorder.await {
                                    tracing::warn!(%error, "SIP recorder stopped");
                                }
                            }
                            dispatch_call(call, process.clone()).await;
                        }
                    }
                    Some("ACK") => {}
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_invite(
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    invite: InviteInfo,
    rtp_bind_addr: &str,
    media_dir: &str,
    local_uri: &str,
    max_call_secs: u64,
    calls: &Arc<Mutex<HashMap<String, ActiveCall>>>,
) -> Result<()> {
    let trying = build_response(100, "Trying", &invite, local_uri, None);
    let _ = socket.send_to(trying.as_bytes(), peer).await;
    let ringing = build_response(180, "Ringing", &invite, local_uri, None);
    let _ = socket.send_to(ringing.as_bytes(), peer).await;

    let rtp_socket =
        UdpSocket::bind(rtp_bind_addr)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("bind SIP RTP {rtp_bind_addr}: {err}"),
            })?;
    let rtp_port = rtp_socket
        .local_addr()
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("read RTP local addr: {err}"),
        })?
        .port();
    let dir = if media_dir.trim().is_empty() {
        std::env::temp_dir().join("rua-sip")
    } else {
        PathBuf::from(media_dir)
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create SIP media dir {}: {err}", dir.display()),
        })?;
    let audio_path = dir.join(format!("{}.pcmu", sanitize_id(&invite.call_id)));
    let audio_path_for_task = audio_path.clone();
    let max_call_secs = max_call_secs.max(1);
    let cancel = CancellationToken::new();
    let recorder_cancel = cancel.clone();
    let remote = invite.remote_rtp;
    let recorder = tokio::spawn(async move {
        receive_rtp_to_file(
            rtp_socket,
            audio_path_for_task,
            max_call_secs,
            remote,
            recorder_cancel,
        )
        .await
    });

    let ok = build_response(200, "OK", &invite, local_uri, Some(rtp_port));
    socket
        .send_to(ok.as_bytes(), peer)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("send SIP 200 OK: {err}"),
        })?;
    let remote_uri = invite
        .contact
        .clone()
        .unwrap_or_else(|| invite.from.clone());
    calls.lock().await.insert(
        invite.call_id.clone(),
        ActiveCall {
            call_id: invite.call_id,
            from: invite.from,
            to: invite.to,
            peer,
            remote_rtp: invite.remote_rtp,
            audio_path,
            transcript: invite.transcript,
            local_uri: local_uri.to_string(),
            remote_uri,
            cancel,
            recorder: Arc::new(Mutex::new(Some(recorder))),
        },
    );
    Ok(())
}

async fn send_audio_file_as_rtp(path: PathBuf, remote: SocketAddr) -> Result<()> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("read SIP RTP audio {}: {err}", path.display()),
        })?;
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("bind SIP RTP send socket: {err}"),
        })?;
    let ssrc = 0x434f_504c_u32;
    let mut sequence = 1_u16;
    let mut timestamp = 0_u32;
    for chunk in bytes.chunks(RTP_PAYLOAD_BYTES) {
        let mut packet = Vec::with_capacity(12 + chunk.len());
        packet.push(0x80);
        packet.push(0x00);
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&timestamp.to_be_bytes());
        packet.extend_from_slice(&ssrc.to_be_bytes());
        packet.extend_from_slice(chunk);
        socket
            .send_to(&packet, remote)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("send SIP RTP audio: {err}"),
            })?;
        sequence = sequence.wrapping_add(1);
        timestamp = timestamp.wrapping_add(chunk.len() as u32);
    }
    Ok(())
}

fn render_text_parts(parts: &[OutgoingContentPart]) -> String {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            OutgoingContentPart::Text { text }
            | OutgoingContentPart::Markdown { text }
            | OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
            OutgoingContentPart::Tool { name, status, .. } => {
                segments.push(format!("[Tool: {name} - {status:?}]"));
            }
            OutgoingContentPart::Raw { value } => segments.push(value.to_string()),
            OutgoingContentPart::Thinking { .. }
            | OutgoingContentPart::Image { .. }
            | OutgoingContentPart::File { .. }
            | OutgoingContentPart::Audio { .. }
            | OutgoingContentPart::Video { .. } => {}
        }
    }
    segments.join("\n")
}

async fn receive_rtp_to_file(
    socket: UdpSocket,
    path: PathBuf,
    max_call_secs: u64,
    peer: Option<SocketAddr>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let parent = path.parent().expect("media directory");
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&temporary).await?;
    let mut buf = vec![0_u8; RTP_READ_BUF];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(max_call_secs);
    loop {
        // Drain datagrams already queued before a BYE so the final audio is complete.
        let received = match socket.try_recv_from(&mut buf) {
            Ok(received) => received,
            Err(_) => tokio::select! {
                _ = cancel.cancelled() => break,
                result = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)) => {
                    let Ok(Ok(received)) = result else { break; };
                    received
                }
            },
        };
        let (len, sender) = received;
        if peer.is_some_and(|peer| peer != sender) {
            continue;
        }
        if let Some(payload) = rtp_payload(&buf[..len]) {
            file.write_all(payload).await?;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, &path).await?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

async fn dispatch_call(call: ActiveCall, process: Arc<dyn Process>) {
    let mut input = Vec::new();
    if let Some(transcript) = call.transcript.filter(|value| !value.trim().is_empty()) {
        input.push(IncomingContentPart::Text { text: transcript });
    }
    input.push(IncomingContentPart::Audio {
        file_id: call.call_id.clone(),
        path: call.audio_path.clone(),
        mime: Some("audio/pcmu".into()),
        name: Some(format!("{}.pcmu", call.call_id)),
    });
    let req = AgentRequest {
        agent_id: String::new(),
        user_id: call.from.clone(),
        session_id: format!("sip_call:{}", call.call_id),
        input,
        channel_id: "sip".into(),
        conversation_id: call.call_id.clone(),
        message_id: Some(call.call_id.clone()),
        metadata: serde_json::json!({
            "call_id": call.call_id,
            "caller": call.from,
            "callee": call.to,
            "remote_rtp": call.remote_rtp.map(|addr| addr.to_string()),
        }),
    };
    let mut stream = process.handle(req).await;
    while futures::StreamExt::next(&mut stream).await.is_some() {}
}

fn build_request_from_native_value(
    native: &serde_json::Value,
    config: &SipConfig,
) -> Result<AgentRequest> {
    let call_id = native
        .get("call_id")
        .or_else(|| native.get("callId"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if call_id.trim().is_empty() {
        return Err(rua_core::RuaError::Validation {
            message: "sip call_id is required".into(),
        });
    }
    let caller = native
        .get("from")
        .or_else(|| native.get("caller"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let mut input = Vec::new();
    if let Some(transcript) = native
        .get("transcript")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        input.push(IncomingContentPart::Text {
            text: transcript.to_string(),
        });
    }
    if let Some(path) = native
        .get("audio_path")
        .or_else(|| native.get("audioPath"))
        .and_then(|value| value.as_str())
    {
        input.push(IncomingContentPart::Audio {
            file_id: path.to_string(),
            path: PathBuf::from(path),
            mime: Some("audio/pcmu".into()),
            name: None,
        });
    }
    if input.is_empty() {
        return Err(rua_core::RuaError::Validation {
            message: "sip native payload has no transcript or audio".into(),
        });
    }
    Ok(AgentRequest {
        agent_id: String::new(),
        user_id: caller.to_string(),
        session_id: format!("sip_call:{call_id}"),
        input,
        channel_id: "sip".into(),
        conversation_id: call_id.to_string(),
        message_id: Some(call_id.to_string()),
        metadata: serde_json::json!({
            "call_id": call_id,
            "caller": caller,
            "sip_endpoint": config.bind_addr,
        }),
    })
}

pub(super) fn minimal_response(
    code: u16,
    reason: &str,
    message: &super::protocol::SipMessage,
) -> String {
    format!(
        "SIP/2.0 {code} {reason}\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=rua\r\nCall-ID: {call_id}\r\nCSeq: {cseq}\r\nContent-Length: 0\r\n\r\n",
        via = header(message, "via").unwrap_or(""),
        from = header(message, "from").unwrap_or(""),
        to = header(message, "to").unwrap_or(""),
        call_id = header(message, "call-id").unwrap_or(""),
        cseq = header(message, "cseq").unwrap_or(""),
    )
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}
