use super::support::{recv, reply_process};
use axum::extract::{
    ws::{Message, WebSocket},
    State, WebSocketUpgrade,
};
use axum::{routing::get, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::sip::{SipChannel, SipConfig, SipOptions};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone, Default)]
pub(super) struct SpeechState {
    pub runs: Arc<Mutex<Vec<Value>>>,
    pub pcm: Arc<Mutex<Vec<Vec<u8>>>>,
    pub speech: Arc<Mutex<Vec<String>>>,
    pub asr: Arc<Mutex<Option<mpsc::UnboundedSender<Value>>>>,
}

pub(super) async fn speech_server() -> (SpeechState, String, tokio::task::JoinHandle<()>) {
    async fn upgrade(
        State(state): State<SpeechState>,
        headers: axum::http::HeaderMap,
        ws: WebSocketUpgrade,
    ) -> impl axum::response::IntoResponse {
        assert_eq!(headers["authorization"], "Bearer test-dashscope-key");
        ws.on_upgrade(move |socket| speech_connection(socket, state))
    }
    let state = SpeechState::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", get(upgrade))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, url, task)
}

async fn speech_connection(mut socket: WebSocket, state: SpeechState) {
    let Some(Ok(Message::Text(text))) = socket.recv().await else {
        return;
    };
    let run: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(run["header"]["action"], "run-task");
    let task = run["header"]["task_id"].as_str().unwrap().to_string();
    let asr = run["payload"]["task"] == "asr";
    state.runs.lock().await.push(run.clone());
    if socket
        .send(Message::Text(
            json!({"header":{"event":"task-started","task_id":task}})
                .to_string()
                .into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    if asr {
        let (tx, mut rx) = mpsc::unbounded_channel();
        *state.asr.lock().await = Some(tx);
        loop {
            tokio::select! {
                command=rx.recv() => {
                    let Some(sentence)=command else {break;};
                    let result=json!({"header":{"event":"result-generated","task_id":task},"payload":{"output":{"sentence":sentence}}});
                    if socket.send(Message::Text(result.to_string().into())).await.is_err() { break; }
                }
                frame=socket.recv() => {
                    match frame {
                        Some(Ok(Message::Binary(bytes))) => state.pcm.lock().await.push(bytes.to_vec()),
                        Some(Ok(Message::Ping(bytes))) => { let _=socket.send(Message::Pong(bytes)).await; }
                        None|Some(Ok(Message::Close(_)))|Some(Err(_)) => break,
                        _=>{}
                    }
                }
            }
        }
    } else {
        assert_eq!(run["payload"]["model"], "cosyvoice-v1");
        let Some(Ok(Message::Text(text))) = socket.recv().await else {
            return;
        };
        let text: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(text["header"]["action"], "continue-task");
        let text = text["payload"]["input"]["text"]
            .as_str()
            .unwrap()
            .to_string();
        state.speech.lock().await.push(text.clone());
        let Some(Ok(Message::Text(finish))) = socket.recv().await else {
            return;
        };
        assert_eq!(
            serde_json::from_str::<Value>(&finish).unwrap()["header"]["action"],
            "finish-task"
        );
        let sample_rate = run["payload"]["parameters"]["sample_rate"]
            .as_u64()
            .unwrap() as usize;
        let frames = if text == "long greeting" { 300 } else { 50 };
        let pcm = (0..sample_rate / 50)
            .flat_map(|_| 1000_i16.to_le_bytes())
            .collect::<Vec<_>>();
        for _ in 0..frames {
            if socket
                .send(Message::Binary(pcm.clone().into()))
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = socket
            .send(Message::Text(
                json!({"header":{"event":"task-finished","task_id":task}})
                    .to_string()
                    .into(),
            ))
            .await;
    }
}

async fn until(mut predicate: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

pub(super) async fn transcript(state: &SpeechState, text: &str, final_sentence: bool) {
    until(async || state.asr.lock().await.is_some()).await;
    state
        .asr
        .lock()
        .await
        .as_ref()
        .unwrap()
        .send(json!({"text":text,"sentence_end":final_sentence}))
        .unwrap();
}

async fn registrar() -> (String, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            let (len, peer) = socket.recv_from(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..len]);
            let headers = request
                .lines()
                .filter(|line| {
                    ["Via:", "From:", "To:", "Call-ID:", "CSeq:"]
                        .iter()
                        .any(|prefix| line.starts_with(prefix))
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            socket
                .send_to(
                    format!("SIP/2.0 200 OK\r\n{headers}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                    peer,
                )
                .await
                .unwrap();
        }
    });
    (address, task)
}

async fn reserve_udp() -> std::net::SocketAddr {
    UdpSocket::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
}

fn invite(id: &str, peer: std::net::SocketAddr, rtp: std::net::SocketAddr) -> String {
    let body=format!("v=0\r\no=alice 0 0 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",rtp.port());
    format!("INVITE sip:agent@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP {peer};branch=z9hG4bK-{id}\r\nFrom: <sip:alice@{peer}>;tag=alice\r\nTo: <sip:agent@127.0.0.1>\r\nCall-ID: {id}\r\nCSeq: 1 INVITE\r\nContact: <sip:alice@{peer}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",body.len())
}

async fn answer(peer: &UdpSocket) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = [0u8; 8192];
        loop {
            let (len, _) = peer.recv_from(&mut buf).await.unwrap();
            let message = String::from_utf8_lossy(&buf[..len]).to_string();
            if message.starts_with("SIP/2.0 200") || message.starts_with("SIP/2.0 486") {
                return message;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn live_rtp_asr_agent_tts_barge_in_and_restart() {
    let (speech, url, server) = speech_server().await;
    let (registrar_addr, registration) = registrar().await;
    let sip_addr = reserve_udp().await;
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        sip_host: "127.0.0.1".into(),
        sip_port: sip_addr.port(),
        sip_server: registrar_addr,
        dashscope_api_key: "test-dashscope-key".into(),
        rtp_port_low: 0,
        rtp_port_high: 0,
        welcome_greeting: "long greeting".into(),
        max_concurrent_calls: 1,
        extra: std::collections::BTreeMap::from([("dashscope_ws_url".into(), json!(url))]),
        ..Default::default()
    });
    let (process, mut requests) = reply_process("hello from the agent");
    channel.start(process.clone()).await.unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let invite = invite(
        "call-live",
        peer.local_addr().unwrap(),
        rtp.local_addr().unwrap(),
    );
    peer.send_to(invite.as_bytes(), sip_addr).await.unwrap();
    let response = answer(&peer).await;
    let port = response
        .lines()
        .find_map(|line| line.strip_prefix("m=audio "))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_eq!(channel.active_call_count().await, 1);
    // Retransmitted INVITE must reuse the call instead of starting another STT stream.
    peer.send_to(invite.as_bytes(), sip_addr).await.unwrap();
    assert!(answer(&peer).await.contains(&format!("m=audio {port}")));
    let mut packet = vec![0x80, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1];
    packet.extend([0xff; 160]);
    for _ in 0..3 {
        rtp.send_to(&packet, ("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    until(async || !speech.pcm.lock().await.is_empty()).await;
    let pcm = speech.pcm.lock().await;
    assert_eq!(
        pcm[0],
        vec![0; 640],
        "mu-law silence must become PCM16 at 16 kHz"
    );
    drop(pcm);
    let run = speech
        .runs
        .lock()
        .await
        .iter()
        .find(|r| r["payload"]["task"] == "asr")
        .unwrap()
        .clone();
    assert_eq!(run["payload"]["model"], "paraformer-realtime-v2");
    assert_eq!(run["payload"]["parameters"]["sample_rate"], 16000);

    // Speech start stops a greeting that otherwise plays for six seconds.
    let mut buf = [0u8; 2048];
    tokio::time::timeout(Duration::from_secs(5), rtp.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    transcript(&speech, "interrupt", false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    while rtp.try_recv_from(&mut buf).is_ok() {}
    assert!(
        tokio::time::timeout(Duration::from_millis(150), rtp.recv_from(&mut buf))
            .await
            .is_err()
    );
    transcript(&speech, "what time is it", true).await;
    let request = recv(&mut requests).await;
    assert_eq!(request.session_id, "sip_call:call-live");
    assert!(
        matches!(&request.input[0],IncomingContentPart::Text { text } if text=="what time is it")
    );
    until(async || {
        speech
            .speech
            .lock()
            .await
            .iter()
            .any(|t| t == "hello from the agent")
    })
    .await;
    let (len, source) = tokio::time::timeout(Duration::from_secs(5), rtp.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        source.port(),
        port,
        "outbound RTP must use the negotiated socket"
    );
    assert!(rua_channel::sip::protocol::rtp_payload(&buf[..len])
        .unwrap()
        .iter()
        .all(|b| *b == 0xce));
    let busy = invite.replace("call-live", "call-busy");
    peer.send_to(busy.as_bytes(), sip_addr).await.unwrap();
    assert!(answer(&peer).await.starts_with("SIP/2.0 486"));
    let bye=format!("BYE sip:agent@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP {}\r\nFrom: <sip:alice@127.0.0.1>;tag=alice\r\nTo: <sip:agent@127.0.0.1>;tag=rua\r\nCall-ID: call-live\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n",peer.local_addr().unwrap());
    peer.send_to(bye.as_bytes(), sip_addr).await.unwrap();
    answer(&peer).await;
    until(async || channel.active_call_count().await == 0).await;
    tokio::time::timeout(Duration::from_secs(3), channel.stop())
        .await
        .unwrap()
        .unwrap();
    channel.start(process).await.unwrap();
    channel.stop().await.unwrap();
    server.abort();
    registration.abort();
}

#[tokio::test]
async fn invalid_voice_options_and_bind_failures_are_reported_at_start() {
    let (process, _) = reply_process("");
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        tts_provider: "unsupported".into(),
        dashscope_api_key: "test".into(),
        ..Default::default()
    });
    assert!(channel.start(process.clone()).await.is_err());
    let bound = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        sip_host: "127.0.0.1".into(),
        sip_port: bound.local_addr().unwrap().port(),
        dashscope_api_key: "test".into(),
        ..Default::default()
    });
    assert!(channel.start(process).await.is_err());
}

#[tokio::test]
async fn idle_timeout_sends_bye_and_removes_live_session() {
    let (_, url, server) = speech_server().await;
    let (registrar_addr, registration) = registrar().await;
    let sip_addr = reserve_udp().await;
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        sip_host: "127.0.0.1".into(),
        sip_port: sip_addr.port(),
        sip_server: registrar_addr,
        dashscope_api_key: "test-dashscope-key".into(),
        rtp_port_low: 0,
        rtp_port_high: 0,
        welcome_greeting: String::new(),
        call_timeout: 0.3,
        extra: std::collections::BTreeMap::from([("dashscope_ws_url".into(), json!(url))]),
        ..Default::default()
    });
    let (process, _) = reply_process("");
    channel.start(process).await.unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.send_to(
        invite(
            "idle",
            peer.local_addr().unwrap(),
            rtp.local_addr().unwrap(),
        )
        .as_bytes(),
        sip_addr,
    )
    .await
    .unwrap();
    answer(&peer).await;
    let mut buf = [0; 4096];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..len]).starts_with("BYE sip:"));
    until(async || channel.active_call_count().await == 0).await;
    channel.stop().await.unwrap();
    server.abort();
    registration.abort();
}

#[tokio::test]
async fn registration_answers_digest_challenge_and_unregisters_on_stop() {
    use md5::{Digest, Md5};

    let registrar = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = registrar.local_addr().unwrap();
    let sip_addr = reserve_udp().await;
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        sip_host: "127.0.0.1".into(),
        sip_port: sip_addr.port(),
        sip_server: address.to_string(),
        sip_username: "agent".into(),
        sip_password: "sip-secret".into(),
        dashscope_api_key: "test-dashscope-key".into(),
        ..Default::default()
    });
    let (process, _) = reply_process("");
    channel.start(process).await.unwrap();
    let mut buf = [0; 8192];
    let (len, peer) = tokio::time::timeout(Duration::from_secs(3), registrar.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let initial = String::from_utf8_lossy(&buf[..len]);
    assert!(initial.starts_with("REGISTER sip:"));
    let headers = initial
        .lines()
        .filter(|line| {
            ["Via:", "From:", "To:", "Call-ID:", "CSeq:"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let challenge = format!("SIP/2.0 401 Unauthorized\r\n{headers}\r\nWWW-Authenticate: Digest realm=\"test-sip\", nonce=\"nonce-one\", qop=\"auth\", opaque=\"opaque-one\"\r\nContent-Length: 0\r\n\r\n");
    registrar.send_to(challenge.as_bytes(), peer).await.unwrap();
    let (len, _) = tokio::time::timeout(Duration::from_secs(3), registrar.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let authenticated = String::from_utf8_lossy(&buf[..len]);
    let auth = authenticated
        .lines()
        .find_map(|line| line.strip_prefix("Authorization: Digest "))
        .unwrap();
    let fields: std::collections::HashMap<_, _> = auth
        .split(", ")
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap();
            (key, value.trim_matches('"'))
        })
        .collect();
    let hash = |input: &str| format!("{:x}", Md5::digest(input.as_bytes()));
    let ha1 = hash("agent:test-sip:sip-secret");
    let ha2 = hash(&format!("REGISTER:sip:{address}"));
    let expected = hash(&format!(
        "{ha1}:nonce-one:00000001:{}:auth:{ha2}",
        fields["cnonce"]
    ));
    assert_eq!(fields["response"], expected);
    assert_eq!(fields["opaque"], "opaque-one");
    assert_eq!(fields["qop"], "auth");
    assert!(authenticated.contains("CSeq: 2 REGISTER"));

    channel.stop().await.unwrap();
    let (len, _) = tokio::time::timeout(Duration::from_secs(3), registrar.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..len]).contains("Expires: 0"));
}
