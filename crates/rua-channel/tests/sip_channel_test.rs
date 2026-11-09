use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

use rua_channel::core::channel::{AgentRequest, Channel, OutMessage, Process};
use rua_channel::core::content::{ChannelTarget, IncomingContentPart, OutgoingContentPart};
use rua_channel::sip::protocol::rtp_payload;
use rua_channel::sip::{SipChannel, SipConfig};
use rua_core::RuntimeEvent;

#[derive(Default)]
struct CapturingProcess {
    requests: Mutex<Vec<AgentRequest>>,
}

#[async_trait]
impl Process for CapturingProcess {
    async fn handle(&self, req: AgentRequest) -> futures::stream::BoxStream<'static, RuntimeEvent> {
        self.requests.lock().await.push(req);
        Box::pin(stream::empty())
    }
}

#[tokio::test]
async fn sip_invite_rtp_bye_routes_audio_to_agent_session() {
    let temp = tempfile::tempdir().unwrap();
    let sip_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sip_addr = sip_socket.local_addr().unwrap();
    drop(sip_socket);
    let channel = Arc::new(SipChannel::new(SipConfig {
        bind_addr: sip_addr.to_string(),
        rtp_bind_addr: "127.0.0.1:0".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
        username: "rua".into(),
        domain: "127.0.0.1".into(),
        ..Default::default()
    }));
    let process = Arc::new(CapturingProcess::default());
    channel.start(process.clone()).await.unwrap();

    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_peer_port = rtp_peer.local_addr().unwrap().port();
    let invite = format!(
        "INVITE sip:rua@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:9999;branch=z9hG4bK-test\r\nFrom: <sip:alice@example.com>;tag=a\r\nTo: <sip:rua@127.0.0.1>\r\nCall-ID: call-123\r\nCSeq: 1 INVITE\r\nContact: <sip:alice@127.0.0.1:{}>\r\nX-Rua-Transcript: hello from call\r\nContent-Type: application/sdp\r\nContent-Length: 117\r\n\r\nv=0\r\no=alice 0 0 IN IP4 127.0.0.1\r\ns=test\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
        peer.local_addr().unwrap().port(),
        rtp_peer_port
    );
    peer.send_to(invite.as_bytes(), sip_addr).await.unwrap();

    let mut buf = [0_u8; 4096];
    let mut ok = String::new();
    for _ in 0..3 {
        let (len, _) = peer.recv_from(&mut buf).await.unwrap();
        let msg = String::from_utf8_lossy(&buf[..len]).to_string();
        if msg.starts_with("SIP/2.0 200") {
            ok = msg;
            break;
        }
    }
    assert!(ok.contains("m=audio "), "expected 200 OK SDP, got {ok}");
    let rtp_port = ok
        .lines()
        .find_map(|line| line.strip_prefix("m=audio "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mut rtp = vec![0x80, 0x00, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
    rtp.extend_from_slice(b"audio-payload");
    rtp_peer
        .send_to(&rtp, format!("127.0.0.1:{rtp_port}"))
        .await
        .unwrap();

    let bye = "BYE sip:rua@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:9999;branch=z9hG4bK-bye\r\nFrom: <sip:alice@example.com>;tag=a\r\nTo: <sip:rua@127.0.0.1>;tag=rua\r\nCall-ID: call-123\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n";
    peer.send_to(bye.as_bytes(), sip_addr).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let requests = process.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.channel_id, "sip");
    assert_eq!(req.session_id, "sip_call:call-123");
    assert!(req.input.iter().any(|part| matches!(
        part,
        IncomingContentPart::Text { text } if text == "hello from call"
    )));
    let audio_path = req
        .input
        .iter()
        .find_map(|part| match part {
            IncomingContentPart::Audio { path, .. } => Some(path.clone()),
            _ => None,
        })
        .expect("audio part");
    assert_eq!(std::fs::read(audio_path).unwrap(), b"audio-payload");

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn sip_send_uses_in_dialog_message_request() {
    let temp = tempfile::tempdir().unwrap();
    let sip_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sip_addr = sip_socket.local_addr().unwrap();
    drop(sip_socket);
    let channel = Arc::new(SipChannel::new(SipConfig {
        bind_addr: sip_addr.to_string(),
        rtp_bind_addr: "127.0.0.1:0".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
        username: "rua".into(),
        domain: "127.0.0.1".into(),
        ..Default::default()
    }));
    channel
        .start(Arc::new(CapturingProcess::default()))
        .await
        .unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let invite = format!(
        "INVITE sip:rua@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:9999;branch=z9hG4bK-test\r\nFrom: <sip:alice@example.com>;tag=a\r\nTo: <sip:rua@127.0.0.1>\r\nCall-ID: call-send\r\nCSeq: 1 INVITE\r\nContact: <sip:alice@{}>\r\nContent-Length: 0\r\n\r\n",
        peer.local_addr().unwrap()
    );
    peer.send_to(invite.as_bytes(), sip_addr).await.unwrap();
    let mut buf = [0_u8; 4096];
    for _ in 0..3 {
        let (len, _) = peer.recv_from(&mut buf).await.unwrap();
        if String::from_utf8_lossy(&buf[..len]).starts_with("SIP/2.0 200") {
            break;
        }
    }

    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "call-send".into(),
        sender_id: Some("alice".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    let msg = OutMessage {
        channel_id: "sip".into(),
        conversation_id: "call-send".into(),
        receiver_id: Some("alice".into()),
        content: vec![OutgoingContentPart::Text {
            text: "hello over sip".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    channel.send(target, msg).await.unwrap();
    let (len, _) = peer.recv_from(&mut buf).await.unwrap();
    let packet = String::from_utf8_lossy(&buf[..len]);
    assert!(packet.starts_with("MESSAGE "));
    assert!(packet.contains("Call-ID: call-send"));
    assert!(packet.ends_with("hello over sip"));
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn sip_send_audio_part_streams_rtp_to_negotiated_peer() {
    let temp = tempfile::tempdir().unwrap();
    let sip_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sip_addr = sip_socket.local_addr().unwrap();
    drop(sip_socket);
    let channel = Arc::new(SipChannel::new(SipConfig {
        bind_addr: sip_addr.to_string(),
        rtp_bind_addr: "127.0.0.1:0".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
        username: "rua".into(),
        domain: "127.0.0.1".into(),
        ..Default::default()
    }));
    channel
        .start(Arc::new(CapturingProcess::default()))
        .await
        .unwrap();

    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_peer_port = rtp_peer.local_addr().unwrap().port();
    let invite = format!(
        "INVITE sip:rua@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:9999;branch=z9hG4bK-test\r\nFrom: <sip:alice@example.com>;tag=a\r\nTo: <sip:rua@127.0.0.1>\r\nCall-ID: call-audio-send\r\nCSeq: 1 INVITE\r\nContact: <sip:alice@{}>\r\nContent-Type: application/sdp\r\nContent-Length: 117\r\n\r\nv=0\r\no=alice 0 0 IN IP4 127.0.0.1\r\ns=test\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
        peer.local_addr().unwrap(),
        rtp_peer_port
    );
    peer.send_to(invite.as_bytes(), sip_addr).await.unwrap();
    let mut sip_buf = [0_u8; 4096];
    for _ in 0..3 {
        let (len, _) = peer.recv_from(&mut sip_buf).await.unwrap();
        if String::from_utf8_lossy(&sip_buf[..len]).starts_with("SIP/2.0 200") {
            break;
        }
    }

    let audio_path = temp.path().join("agent-response.pcmu");
    std::fs::write(&audio_path, b"agent-audio-payload").unwrap();
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "call-audio-send".into(),
        sender_id: Some("alice".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    let msg = OutMessage {
        channel_id: "sip".into(),
        conversation_id: "call-audio-send".into(),
        receiver_id: Some("alice".into()),
        content: vec![OutgoingContentPart::Audio {
            url: None,
            path: Some(audio_path),
            name: Some("agent-response.pcmu".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    channel.send(target, msg).await.unwrap();

    let mut rtp_buf = [0_u8; 2048];
    let (len, _) = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        rtp_peer.recv_from(&mut rtp_buf),
    )
    .await
    .expect("rtp packet")
    .unwrap();
    assert_eq!(
        rtp_payload(&rtp_buf[..len]).unwrap(),
        b"agent-audio-payload"
    );
    channel.stop().await.unwrap();
}
