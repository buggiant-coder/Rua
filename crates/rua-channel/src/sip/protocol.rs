use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SipStartLine {
    Request { method: String, uri: String },
    Response { code: u16, reason: String },
}

#[derive(Debug, Clone)]
pub struct SipMessage {
    pub start_line: SipStartLine,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct InviteInfo {
    pub call_id: String,
    pub from: String,
    pub to: String,
    pub cseq: String,
    pub via: String,
    pub contact: Option<String>,
    pub remote_rtp: Option<SocketAddr>,
    pub transcript: Option<String>,
}

pub fn parse_sip_message(raw: &str) -> Option<SipMessage> {
    let normalized = raw.replace("\r\n", "\n");
    let (head, body) = normalized
        .split_once("\n\n")
        .map(|(head, body)| (head, body.to_string()))
        .unwrap_or((normalized.as_str(), String::new()));
    let mut lines = head.lines();
    let first = lines.next()?.trim();
    let start_line = if first.starts_with("SIP/2.0") {
        let mut parts = first.splitn(3, ' ');
        let _ = parts.next();
        let code = parts.next()?.parse::<u16>().ok()?;
        let reason = parts.next().unwrap_or("").to_string();
        SipStartLine::Response { code, reason }
    } else {
        let mut parts = first.split_whitespace();
        let method = parts.next()?.to_string();
        let uri = parts.next()?.to_string();
        SipStartLine::Request { method, uri }
    };
    let mut headers = HashMap::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Some(SipMessage {
        start_line,
        headers,
        body,
    })
}

pub fn invite_info(message: &SipMessage) -> Option<InviteInfo> {
    let SipStartLine::Request { method, .. } = &message.start_line else {
        return None;
    };
    if method != "INVITE" {
        return None;
    }
    let call_id = header(message, "call-id")?.to_string();
    let from = header(message, "from").unwrap_or("unknown").to_string();
    let to = header(message, "to").unwrap_or("").to_string();
    let cseq = header(message, "cseq").unwrap_or("1 INVITE").to_string();
    let via = header(message, "via").unwrap_or("").to_string();
    let contact = header(message, "contact").map(str::to_string);
    let remote_rtp = parse_sdp_rtp_addr(&message.body);
    let transcript = header(message, "x-rua-transcript").map(str::to_string);
    Some(InviteInfo {
        call_id,
        from,
        to,
        cseq,
        via,
        contact,
        remote_rtp,
        transcript,
    })
}

pub fn header<'a>(message: &'a SipMessage, name: &str) -> Option<&'a str> {
    message
        .headers
        .get(&name.to_ascii_lowercase())
        .map(String::as_str)
}

pub fn request_method(message: &SipMessage) -> Option<&str> {
    match &message.start_line {
        SipStartLine::Request { method, .. } => Some(method.as_str()),
        _ => None,
    }
}

pub fn build_response(
    code: u16,
    reason: &str,
    invite: &InviteInfo,
    local_contact: &str,
    local_rtp_port: Option<u16>,
) -> String {
    build_response_with_host(
        code,
        reason,
        invite,
        local_contact,
        local_rtp_port,
        "127.0.0.1",
    )
}

pub(super) fn build_response_with_host(
    code: u16,
    reason: &str,
    invite: &InviteInfo,
    local_contact: &str,
    local_rtp_port: Option<u16>,
    host: &str,
) -> String {
    let body = local_rtp_port
        .map(|port| {
            format!(
                "v=0\r\no=rua 0 0 IN IP4 {host}\r\ns=Rua SIP\r\nc=IN IP4 {host}\r\nt=0 0\r\nm=audio {port} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
            )
        })
        .unwrap_or_default();
    let content_type = if body.is_empty() {
        String::new()
    } else {
        "Content-Type: application/sdp\r\n".into()
    };
    format!(
        "SIP/2.0 {code} {reason}\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=rua\r\nCall-ID: {call_id}\r\nCSeq: {cseq}\r\nContact: <{local_contact}>\r\n{content_type}Content-Length: {len}\r\n\r\n{body}",
        via = invite.via,
        from = invite.from,
        to = invite.to,
        call_id = invite.call_id,
        cseq = invite.cseq,
        len = body.len(),
    )
}

pub fn build_message_request(
    call_id: &str,
    from_uri: &str,
    to_uri: &str,
    cseq: u32,
    text: &str,
) -> String {
    format!(
        "MESSAGE {to_uri} SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK-rua-{cseq}\r\nFrom: <{from_uri}>;tag=rua\r\nTo: <{to_uri}>\r\nCall-ID: {call_id}\r\nCSeq: {cseq} MESSAGE\r\nContent-Type: text/plain\r\nContent-Length: {len}\r\n\r\n{text}",
        len = text.len()
    )
}

fn parse_sdp_rtp_addr(sdp: &str) -> Option<SocketAddr> {
    let mut host = "127.0.0.1".to_string();
    let mut port = None;
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("c=IN IP4 ") {
            host = rest.trim().to_string();
        }
        if let Some(rest) = line.strip_prefix("m=audio ") {
            port = rest
                .split_whitespace()
                .next()
                .and_then(|p| p.parse::<u16>().ok());
        }
    }
    format!("{}:{}", host, port?).parse().ok()
}

pub fn rtp_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let mut start = 12 + (packet[0] & 0x0f) as usize * 4;
    if packet[0] & 0x10 != 0 {
        let extension = packet.get(start..start + 4)?;
        start += 4 + u16::from_be_bytes([extension[2], extension[3]]) as usize * 4;
    }
    let padding = if packet[0] & 0x20 != 0 {
        *packet.last()? as usize
    } else {
        0
    };
    let end = packet.len().checked_sub(padding)?;
    if start >= end {
        return None;
    }
    packet.get(start..end)
}

pub(super) fn build_bye_request(call_id: &str, from: &str, to: &str, cseq: u32) -> String {
    let uri = to
        .split('<')
        .nth(1)
        .and_then(|s| s.split('>').next())
        .unwrap_or(to);
    let host = from.split('@').next_back().unwrap_or(from);
    format!("BYE {uri} SIP/2.0\r\nVia: SIP/2.0/UDP {host};branch=z9hG4bK-{}\r\nFrom: <{from}>;tag=rua\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: {cseq} BYE\r\nContent-Length: 0\r\n\r\n", uuid::Uuid::new_v4())
}
