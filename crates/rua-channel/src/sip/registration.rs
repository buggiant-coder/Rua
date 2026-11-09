//! SIP REGISTER and QwenPaw's loopback development registrar/proxy.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use md5::{Digest, Md5};
use tokio::net::UdpSocket;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::protocol::{header, parse_sip_message, SipMessage, SipStartLine};
use super::realtime::io_error;
use super::types::SipOptions;
use rua_core::{Result, RuaError};

pub(super) async fn resolve_server(server: &str) -> Result<SocketAddr> {
    let server = server.trim_start_matches("sip:").trim_start_matches("//");
    let address = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:5060")
    };
    tokio::net::lookup_host(address)
        .await
        .map_err(|e| io_error("resolve SIP server", e))?
        .next()
        .ok_or_else(|| RuaError::Validation {
            message: "SIP server address is empty".into(),
        })
}

pub(super) struct Registration {
    server: SocketAddr,
    contact: String,
    username: String,
    password: String,
    id: String,
    seq: u32,
    challenges: u8,
}

impl Registration {
    pub fn new(options: &SipOptions, server: SocketAddr, contact: &str) -> Self {
        Self {
            server,
            contact: contact.into(),
            username: if options.sip_username.is_empty() {
                "agent".into()
            } else {
                options.sip_username.clone()
            },
            password: options.sip_password.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            seq: 0,
            challenges: 0,
        }
    }

    pub fn request(&mut self, expires: u32, authorization: Option<(&str, String)>) -> String {
        self.seq += 1;
        let auth = authorization
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .unwrap_or_default();
        format!(
            "REGISTER sip:{server} SIP/2.0\r\nVia: SIP/2.0/UDP {contact};branch=z9hG4bK-{id}-{seq};rport\r\nFrom: <sip:{user}@{server}>;tag={id}\r\nTo: <sip:{user}@{server}>\r\nCall-ID: {id}\r\nCSeq: {seq} REGISTER\r\nContact: <sip:{user}@{contact}>\r\nExpires: {expires}\r\n{auth}Content-Length: 0\r\n\r\n",
            server=self.server, contact=self.contact, id=self.id, seq=self.seq, user=self.username,
        )
    }

    pub fn accept(&mut self, message: &SipMessage) {
        if header(message, "call-id") == Some(&self.id) {
            self.challenges = 0;
        }
    }

    pub fn challenge(&mut self, message: &SipMessage) -> Result<Option<String>> {
        if header(message, "call-id") != Some(&self.id) {
            return Ok(None);
        }
        self.challenges += 1;
        if self.challenges > 2 {
            return Err(RuaError::Validation {
                message: "SIP registration authentication rejected".into(),
            });
        }
        let (field, raw) = if let Some(value) = header(message, "www-authenticate") {
            ("Authorization", value)
        } else if let Some(value) = header(message, "proxy-authenticate") {
            ("Proxy-Authorization", value)
        } else {
            return Ok(None);
        };
        let fields = digest_fields(raw);
        let realm = fields.get("realm").cloned().unwrap_or_default();
        let nonce = fields.get("nonce").cloned().unwrap_or_default();
        let uri = format!("sip:{}", self.server);
        let ha1 = md5(&format!("{}:{realm}:{}", self.username, self.password));
        let ha2 = md5(&format!("REGISTER:{uri}"));
        let cnonce = uuid::Uuid::new_v4().simple().to_string();
        let qop = fields
            .get("qop")
            .is_some_and(|q| q.split(',').any(|q| q.trim() == "auth"));
        let response = if qop {
            md5(&format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}"))
        } else {
            md5(&format!("{ha1}:{nonce}:{ha2}"))
        };
        let mut auth = format!(
            "Digest username=\"{}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5",
            self.username,
        );
        if qop {
            auth.push_str(&format!(", qop=auth, nc=00000001, cnonce=\"{cnonce}\""));
        }
        if let Some(opaque) = fields.get("opaque") {
            auth.push_str(&format!(", opaque=\"{opaque}\""));
        }
        Ok(Some(self.request(600, Some((field, auth)))))
    }
}

fn md5(text: &str) -> String {
    format!("{:x}", Md5::digest(text.as_bytes()))
}

fn digest_fields(raw: &str) -> HashMap<String, String> {
    let regex = regex::Regex::new(r#"([a-zA-Z_]+)\s*=\s*(?:"([^"]*)"|([^,\s]+))"#)
        .expect("constant digest regex");
    regex
        .captures_iter(raw)
        .map(|capture| {
            (
                capture[1].to_lowercase(),
                capture
                    .get(2)
                    .or_else(|| capture.get(3))
                    .map(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

fn uri_user(uri: &str) -> &str {
    uri.split_once("sip:")
        .map(|(_, v)| v)
        .unwrap_or(uri)
        .split('@')
        .next()
        .unwrap_or_default()
}

pub(super) async fn run_registrar(
    socket: UdpSocket,
    bot: String,
    bot_addr: SocketAddr,
    cancel: CancellationToken,
) {
    let mut users: HashMap<String, (SocketAddr, Instant)> = HashMap::new();
    let mut dialogs: HashMap<String, (SocketAddr, SocketAddr, Instant)> = HashMap::new();
    users.insert(bot, (bot_addr, Instant::now() + Duration::from_secs(86400)));
    let mut buf = [0u8; 8192];
    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => break,
            received = socket.recv_from(&mut buf) => received,
        };
        let Ok((len, peer)) = received else {
            break;
        };
        let raw = String::from_utf8_lossy(&buf[..len]);
        let Some(message) = parse_sip_message(&raw) else {
            continue;
        };
        let now = Instant::now();
        users.retain(|_, (_, end)| *end > now);
        dialogs.retain(|_, (_, _, end)| *end > now);
        match &message.start_line {
            SipStartLine::Request { method, uri } if method == "REGISTER" => {
                let user = uri_user(header(&message, "to").unwrap_or_default()).to_string();
                if user.is_empty() {
                    continue;
                }
                let expires = header(&message, "expires")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(3600)
                    .min(86400);
                if expires == 0 {
                    if users.get(&user).is_some_and(|(addr, _)| *addr == peer) {
                        users.remove(&user);
                    }
                } else if users.len() < 1024 || users.contains_key(&user) {
                    users.insert(user, (peer, now + Duration::from_secs(expires)));
                }
                let response = super::channel::minimal_response(200, "OK", &message);
                let _ = socket.send_to(response.as_bytes(), peer).await;
                let _ = uri;
            }
            SipStartLine::Request { method, uri } => {
                let id = header(&message, "call-id").unwrap_or_default().to_string();
                let dest = dialogs
                    .get(&id)
                    .and_then(|(a, b, _)| {
                        if *a == peer {
                            Some(*b)
                        } else if *b == peer {
                            Some(*a)
                        } else {
                            None
                        }
                    })
                    .or_else(|| users.get(uri_user(uri)).map(|(addr, _)| *addr));
                if let Some(dest) = dest {
                    if !dialogs.contains_key(&id) && dialogs.len() < 1024 {
                        dialogs.insert(id, (peer, dest, now + Duration::from_secs(300)));
                    }
                    let _ = socket.send_to(&buf[..len], dest).await;
                } else if method == "INVITE" {
                    let response = super::channel::minimal_response(404, "Not Found", &message);
                    let _ = socket.send_to(response.as_bytes(), peer).await;
                }
            }
            SipStartLine::Response { .. } => {
                if let Some((a, b, _)) =
                    dialogs.get(header(&message, "call-id").unwrap_or_default())
                {
                    let dest = if *a == peer {
                        Some(*b)
                    } else if *b == peer {
                        Some(*a)
                    } else {
                        None
                    };
                    if let Some(dest) = dest {
                        let _ = socket.send_to(&buf[..len], dest).await;
                    }
                }
            }
        }
    }
}
