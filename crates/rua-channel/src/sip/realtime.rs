use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::protocol::{self, SipStartLine};
use super::types::SipOptions;
use super::voice::{self, AudioOutput, CallControl, VoiceCall};
use crate::core::channel::Process;
use rua_core::{Result, RuaError};

pub(super) type Calls = Arc<Mutex<HashMap<String, CallControl>>>;

pub(super) struct VoiceRuntime {
    pub calls: Calls,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl VoiceRuntime {
    pub async fn start(options: SipOptions, process: Arc<dyn Process>) -> Result<Self> {
        options.validate()?;
        let options = Arc::new(options);
        let cancel = CancellationToken::new();
        let calls: Calls = Arc::default();
        let task_cancel = cancel.clone();
        let task_calls = calls.clone();
        let handle = if options.sip_mode == "livekit" {
            #[cfg(not(feature = "sip-livekit"))]
            return Err(RuaError::Validation {
                message: "LiveKit SIP requires a build with --features sip-livekit".into(),
            });
            #[cfg(feature = "sip-livekit")]
            {
                tokio::spawn(async move {
                    super::livekit::run(options, process, task_calls, task_cancel).await;
                })
            }
        } else {
            let socket = Arc::new(
                UdpSocket::bind((options.sip_host.as_str(), options.sip_port))
                    .await
                    .map_err(|e| io_error("bind SIP", e))?,
            );
            let registrar = if options.sip_server.is_empty() {
                Some(
                    UdpSocket::bind("127.0.0.1:5060")
                        .await
                        .map_err(|e| io_error("bind SIP registrar", e))?,
                )
            } else {
                None
            };
            let registrar_addr = match &registrar {
                Some(socket) => socket
                    .local_addr()
                    .map_err(|e| io_error("SIP registrar address", e))?,
                None => super::registration::resolve_server(&options.sip_server).await?,
            };
            let local_addr = socket
                .local_addr()
                .map_err(|e| io_error("SIP address", e))?;
            tokio::spawn(async move {
                let mut auxiliary = JoinSet::new();
                if let Some(registrar) = registrar {
                    let token = task_cancel.child_token();
                    let username = if options.sip_username.is_empty() {
                        "agent".into()
                    } else {
                        options.sip_username.clone()
                    };
                    let bot_addr =
                        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), local_addr.port());
                    auxiliary.spawn(super::registration::run_registrar(
                        registrar, username, bot_addr, token,
                    ));
                }
                if let Err(error) = udp_loop(
                    socket,
                    registrar_addr,
                    options,
                    process,
                    task_calls,
                    task_cancel.clone(),
                )
                .await
                {
                    tracing::error!(%error, "SIP voice runtime failed");
                }
                task_cancel.cancel();
                while auxiliary.join_next().await.is_some() {}
            })
        };
        Ok(Self {
            calls,
            cancel,
            handle,
        })
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }

    pub async fn send(&self, id: &str, text: String) -> Result<()> {
        let call = self
            .calls
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| RuaError::NotFound {
                resource: "sip_call".into(),
                id: id.into(),
            })?;
        tokio::select! {
            _ = call.cancel.cancelled() => Err(RuaError::Runtime { message: "SIP call ended".into() }),
            result = call.outbound.send(text) => result.map_err(|_| RuaError::Runtime { message: "SIP call ended".into() }),
        }
    }
}

pub(super) fn io_error(context: &str, error: impl std::fmt::Display) -> RuaError {
    RuaError::Io {
        message: format!("{context}: {error}"),
    }
}

async fn udp_loop(
    socket: Arc<UdpSocket>,
    registrar: SocketAddr,
    options: Arc<SipOptions>,
    process: Arc<dyn Process>,
    calls: Calls,
    cancel: CancellationToken,
) -> Result<()> {
    let local = socket
        .local_addr()
        .map_err(|e| io_error("SIP local address", e))?;
    // Match QwenPaw's loopback development registrar, advertise the bound interface otherwise.
    let advertised = if local.ip().is_unspecified() {
        if options.sip_server.is_empty() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            let probe = UdpSocket::bind("0.0.0.0:0")
                .await
                .map_err(|e| io_error("SIP route", e))?;
            probe
                .connect(registrar)
                .await
                .map_err(|e| io_error("SIP route", e))?;
            probe
                .local_addr()
                .map_err(|e| io_error("SIP route", e))?
                .ip()
        }
    } else {
        local.ip()
    };
    let user = if options.sip_username.is_empty() {
        "agent"
    } else {
        &options.sip_username
    };
    let local_uri = format!("sip:{user}@{advertised}:{}", local.port());
    let mut registration = super::registration::Registration::new(
        &options,
        registrar,
        &format!("{advertised}:{}", local.port()),
    );
    let mut refresh = tokio::time::interval(Duration::from_secs(300));
    let mut tasks: JoinSet<String> = JoinSet::new();
    let mut responses: HashMap<String, (SocketAddr, String)> = HashMap::new();
    let mut buf = [0u8; 8192];
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = refresh.tick() => {
                let request = registration.request(600, None);
                socket.send_to(request.as_bytes(), registrar).await.map_err(|e| io_error("SIP REGISTER", e))?;
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Ok(id)) = result {
                    calls.lock().await.remove(&id);
                    responses.remove(&id);
                }
            }
            received = socket.recv_from(&mut buf) => {
                let (len, peer) = received.map_err(|e| io_error("SIP receive", e))?;
                let Some(message) = protocol::parse_sip_message(&String::from_utf8_lossy(&buf[..len])) else { continue; };
                if let SipStartLine::Response { code, .. } = message.start_line {
                    if peer == registrar && code == 200 {
                        registration.accept(&message);
                    }
                    if peer == registrar && (code == 401 || code == 407) {
                        if let Some(request) = registration.challenge(&message)? {
                            socket.send_to(request.as_bytes(), registrar).await.map_err(|e| io_error("SIP auth", e))?;
                        }
                    }
                    continue;
                }
                let id = protocol::header(&message, "call-id").unwrap_or_default().to_string();
                match protocol::request_method(&message) {
                    Some("INVITE") => {
                        if let Some((caller, response)) = responses.get(&id) {
                            if *caller == peer {
                                let _ = socket.send_to(response.as_bytes(), peer).await;
                            }
                            continue;
                        }
                        let Some(invite) = protocol::invite_info(&message) else { continue; };
                        if calls.lock().await.len() >= options.max_concurrent_calls {
                            let response = protocol::build_response(486, "Busy Here", &invite, &local_uri, None);
                            let _ = socket.send_to(response.as_bytes(), peer).await;
                            continue;
                        }
                        let Some(remote) = invite.remote_rtp else {
                            let response = protocol::build_response(488, "Not Acceptable Here", &invite, &local_uri, None);
                            let _ = socket.send_to(response.as_bytes(), peer).await;
                            continue;
                        };
                        if !message.body.lines().any(|line| line.starts_with("m=audio ") && line.split_whitespace().skip(3).any(|p| p == "0")) {
                            let response = protocol::build_response(488, "Not Acceptable Here", &invite, &local_uri, None);
                            let _ = socket.send_to(response.as_bytes(), peer).await;
                            continue;
                        }
                        let rtp = Arc::new(bind_rtp(&options).await?);
                        let port = rtp.local_addr().map_err(|e| io_error("RTP local address", e))?.port();
                        for (code, reason, port) in [(100,"Trying",None),(180,"Ringing",None),(200,"OK",Some(port))] {
                            let response = protocol::build_response_with_host(code, reason, &invite, &local_uri, port, &advertised.to_string());
                            socket.send_to(response.as_bytes(), peer).await.map_err(|e| io_error("SIP response", e))?;
                            if code == 200 { responses.insert(id.clone(), (peer, response)); }
                        }
                        let call_cancel = cancel.child_token();
                        let (out_tx, out_rx) = mpsc::channel(16);
                        calls.lock().await.insert(id.clone(), CallControl { cancel: call_cancel.clone(), outbound: out_tx });
                        let call_options = options.clone();
                        let call_process = process.clone();
                        let sip_socket = socket.clone();
                        let call_uri = local_uri.clone();
                        tasks.spawn(async move {
                            let _cancel_on_drop = call_cancel.clone().drop_guard();
                            let (input_tx, input_rx) = mpsc::channel(150);
                            let output = Arc::new(RtpOutput {
                                socket: rtp.clone(), peer: remote,
                                state: Mutex::new((rand::random(), rand::random())),
                                ssrc: rand::random(),
                            });
                            let voice = voice::run_call(VoiceCall {
                                id: id.clone(), caller: invite.from.clone(), callee: invite.to.clone(),
                                input: input_rx, output, output_rate: 8000, outbound: out_rx,
                                cancel: call_cancel.clone(),
                            }, call_options, call_process);
                            let reader_cancel = call_cancel.clone();
                            let reader = tokio::spawn(async move {
                                receive_audio(rtp, remote, input_tx, reader_cancel.clone()).await;
                                reader_cancel.cancel();
                            });
                            if let Err(error) = voice.await {
                                tracing::warn!(%error, call_id = %id, "SIP call failed");
                            }
                            // Timeout / ASR failure terminates the remote dialog too.
                            if !call_cancel.is_cancelled() {
                                let bye = protocol::build_bye_request(&id, &call_uri, &invite.from, 2);
                                let _ = sip_socket.send_to(bye.as_bytes(), peer).await;
                            }
                            call_cancel.cancel();
                            let _ = reader.await;
                            id
                        });
                    }
                    Some("BYE" | "CANCEL") => {
                        if responses.get(&id).is_some_and(|(caller,_)| *caller == peer) {
                            if let Some(call) = calls.lock().await.get(&id) { call.cancel.cancel(); }
                        }
                        let response = super::channel::minimal_response(200, "OK", &message);
                        let _ = socket.send_to(response.as_bytes(), peer).await;
                    }
                    Some("OPTIONS") => {
                        let response = super::channel::minimal_response(200, "OK", &message);
                        let _ = socket.send_to(response.as_bytes(), peer).await;
                    }
                    _ => {}
                }
            }
        }
    }
    for call in calls.lock().await.values() {
        call.cancel.cancel();
    }
    while tasks.join_next().await.is_some() {}
    calls.lock().await.clear();
    let _ = socket
        .send_to(registration.request(0, None).as_bytes(), registrar)
        .await;
    Ok(())
}

async fn bind_rtp(options: &SipOptions) -> Result<UdpSocket> {
    for port in options.rtp_port_low..=options.rtp_port_high {
        match UdpSocket::bind((options.sip_host.as_str(), port)).await {
            Ok(socket) => return Ok(socket),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(io_error("bind RTP", error)),
        }
    }
    Err(RuaError::Runtime {
        message: "SIP RTP port range exhausted".into(),
    })
}

async fn receive_audio(
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    input: mpsc::Sender<Vec<i16>>,
    cancel: CancellationToken,
) {
    let mut buf = [0u8; 4096];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            packet = socket.recv_from(&mut buf) => {
                let Ok((len, sender)) = packet else { break; };
                if sender != peer || len < 12 || buf[1] & 0x7f != 0 { continue; }
                if let Some(payload) = protocol::rtp_payload(&buf[..len]) {
                    // G.711 mu-law, 8 kHz -> mono PCM16, 16 kHz for Paraformer.
                    let pcm = payload.iter().flat_map(|byte| {
                        let sample = decode_mulaw(*byte);
                        [sample, sample]
                    }).collect();
                    let _ = input.try_send(pcm);
                }
            }
        }
    }
}

struct RtpOutput {
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    state: Mutex<(u16, u32)>,
    ssrc: u32,
}

#[async_trait]
impl AudioOutput for RtpOutput {
    async fn write(&self, pcm: &[i16], sample_rate: u32) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut packet = vec![0x80, 0];
        packet.extend_from_slice(&state.0.to_be_bytes());
        packet.extend_from_slice(&state.1.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());
        packet.extend(pcm.iter().map(|sample| encode_mulaw(*sample)));
        self.socket
            .send_to(&packet, self.peer)
            .await
            .map_err(|e| io_error("RTP output", e))?;
        state.0 = state.0.wrapping_add(1);
        state.1 = state.1.wrapping_add(pcm.len() as u32);
        tokio::time::sleep(Duration::from_secs_f64(
            pcm.len() as f64 / sample_rate as f64,
        ))
        .await;
        Ok(())
    }

    async fn clear(&self) {}
}

fn decode_mulaw(byte: u8) -> i16 {
    let value = !byte;
    let magnitude = (((value & 0x0f) as i32) << 3) + 0x84;
    let sample = (magnitude << ((value & 0x70) >> 4)) - 0x84;
    if value & 0x80 != 0 {
        -sample as i16
    } else {
        sample as i16
    }
}

fn encode_mulaw(sample: i16) -> u8 {
    let sample = i32::from(sample);
    let sign = if sample < 0 { 0x80 } else { 0 };
    let value = sample.abs().min(32635) + 0x84;
    let exponent = (31 - (value as u32).leading_zeros())
        .saturating_sub(7)
        .min(7);
    let mantissa = ((value >> (exponent + 3)) & 0x0f) as u8;
    !(sign | (exponent as u8) << 4 | mantissa)
}
