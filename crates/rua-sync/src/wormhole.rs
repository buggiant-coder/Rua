use iroh::{Endpoint, EndpointAddr, SecretKey};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tracing::debug;

use crate::endpoint::n0_endpoint_builder;
use crate::SyncError;

const PAIRING_ALPN: &[u8] = b"rua-pairing/1";

pub fn derive_rendezvous_secret(pin: &str) -> SecretKey {
    let bytes = blake3::derive_key("rua-pairing-rendezvous-v1", pin.as_bytes());
    SecretKey::from_bytes(&bytes)
}

pub struct WormholeChannel {
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    _conn: iroh::endpoint::Connection,
    endpoint: Option<Endpoint>,
    session_key: Vec<u8>,
}

impl WormholeChannel {
    pub async fn connect_to_listener(pin: &str) -> Result<Self, SyncError> {
        let rendezvous_secret = derive_rendezvous_secret(pin);
        let rendezvous_node_id = rendezvous_secret.public();

        let endpoint = n0_endpoint_builder()
            .alpns(vec![PAIRING_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| SyncError::Pairing(format!("connector bind: {e}")))?;

        debug!(target_node = %rendezvous_node_id.fmt_short(), "connector connecting to listener");

        let setup = async {
            let listener_addr = connector_listener_addr(rendezvous_node_id);
            let conn = endpoint
                .connect(listener_addr, PAIRING_ALPN)
                .await
                .map_err(|e| SyncError::Pairing(format!("connector connect: {e}")))?;
            let (send, recv) = conn
                .accept_bi()
                .await
                .map_err(|e| SyncError::Pairing(format!("accept bi stream: {e}")))?;
            Ok::<_, SyncError>((send, recv, conn))
        }
        .await;

        match setup {
            Ok((send, recv, conn)) => Self::handshake(send, recv, conn, Some(endpoint), pin).await,
            Err(e) => {
                endpoint.close().await;
                Err(e)
            }
        }
    }

    pub async fn create_initiator_endpoint(pin: &str) -> Result<Endpoint, SyncError> {
        let rendezvous_secret = derive_rendezvous_secret(pin);
        n0_endpoint_builder()
            .secret_key(rendezvous_secret)
            .alpns(vec![PAIRING_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| SyncError::Pairing(format!("initiator bind: {e}")))
    }

    pub async fn accept_and_handshake(endpoint: &Endpoint, pin: &str) -> Result<Self, SyncError> {
        let conn = endpoint
            .accept()
            .await
            .ok_or_else(|| SyncError::Pairing("accept returned None".into()))?
            .await
            .map_err(|e| SyncError::Pairing(format!("accept connection: {e}")))?;
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| SyncError::Pairing(format!("open bi stream: {e}")))?;
        Self::handshake(send, recv, conn, None, pin).await
    }

    async fn handshake(
        mut send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
        conn: iroh::endpoint::Connection,
        mut endpoint: Option<Endpoint>,
        pin: &str,
    ) -> Result<Self, SyncError> {
        let identity = Identity::new(b"rua-pairing-symmetric");
        let (spake_state, outbound_msg) =
            Spake2::<Ed25519Group>::start_symmetric(&Password::new(pin.as_bytes()), &identity);

        if let Err(e) = send_framed(&mut send, &outbound_msg).await {
            if let Some(ep) = endpoint.take() {
                ep.close().await;
            }
            return Err(e);
        }

        let peer_bytes = match recv_framed(&mut recv).await {
            Ok(b) => b,
            Err(e) => {
                if let Some(ep) = endpoint.take() {
                    ep.close().await;
                }
                return Err(e);
            }
        };

        match spake_state.finish(&peer_bytes) {
            Ok(session_key) => Ok(Self {
                send,
                recv,
                _conn: conn,
                endpoint,
                session_key,
            }),
            Err(e) => {
                if let Some(ep) = endpoint.take() {
                    ep.close().await;
                }
                Err(SyncError::Pairing(format!("SPAKE2 finish failed: {e}")))
            }
        }
    }

    pub fn direct_addresses(&self) -> Vec<std::net::SocketAddr> {
        self.endpoint
            .as_ref()
            .map(|ep| ep.addr().ip_addrs().copied().collect())
            .unwrap_or_default()
    }

    pub async fn send_encrypted(&mut self, plaintext: &[u8]) -> Result<(), SyncError> {
        let ciphertext = encrypt_aead(&self.session_key, plaintext)?;
        match send_framed(&mut self.send, &ciphertext).await {
            Ok(()) => Ok(()),
            Err(first_err) => {
                tracing::warn!("[wormhole] send_encrypted failed, retrying in 1s: {first_err}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let ciphertext = encrypt_aead(&self.session_key, plaintext)?;
                send_framed(&mut self.send, &ciphertext).await
            }
        }
    }

    pub async fn recv_encrypted(&mut self) -> Result<Vec<u8>, SyncError> {
        let ciphertext = recv_framed(&mut self.recv).await?;
        decrypt_aead(&self.session_key, &ciphertext)
    }

    pub async fn close(mut self) -> Result<(), SyncError> {
        if let Err(e) = self.send.finish() {
            tracing::warn!("[wormhole] send finish: {e}");
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            drain_recv(&mut self.recv),
        )
        .await;
        drop(self.send);
        drop(self.recv);
        drop(self._conn);
        if let Some(ep) = self.endpoint {
            ep.close().await;
        }
        Ok(())
    }
}

fn connector_listener_addr(rendezvous_node_id: iroh::PublicKey) -> EndpointAddr {
    EndpointAddr::new(rendezvous_node_id)
}

#[cfg(feature = "test-util")]
pub fn connector_listener_addr_for_test(pin: &str) -> EndpointAddr {
    connector_listener_addr(derive_rendezvous_secret(pin).public())
}

async fn drain_recv(recv: &mut iroh::endpoint::RecvStream) {
    let mut buf = [0u8; 1024];
    loop {
        match recv.read(&mut buf).await {
            Ok(None) => break,
            Ok(Some(_)) => continue,
            Err(_) => break,
        }
    }
}

async fn send_framed(send: &mut iroh::endpoint::SendStream, data: &[u8]) -> Result<(), SyncError> {
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len)
        .await
        .map_err(|e| SyncError::Pairing(format!("send len: {e}")))?;
    send.write_all(data)
        .await
        .map_err(|e| SyncError::Pairing(format!("send data: {e}")))?;
    Ok(())
}

async fn recv_framed(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>, SyncError> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(|e| SyncError::Pairing(format!("recv len: {e}")))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(SyncError::Pairing("frame too large".into()));
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| SyncError::Pairing(format!("recv data: {e}")))?;
    Ok(buf)
}

fn encrypt_aead(session_key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, SyncError> {
    use rand::RngCore;

    let key = derive_aead_key(session_key);
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);

    let mut stream_key = [0u8; 32];
    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(&nonce);
    hasher.update(b"stream");
    let stream_hash = hasher.finalize();
    stream_key.copy_from_slice(stream_hash.as_bytes());

    let ciphertext: Vec<u8> = plaintext
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let ki = blake3::keyed_hash(&stream_key, &(i as u64).to_le_bytes());
            b ^ ki.as_bytes()[0]
        })
        .collect();

    let mut mac_hasher = blake3::Hasher::new_keyed(&key);
    mac_hasher.update(&nonce);
    mac_hasher.update(&ciphertext);
    let mac = mac_hasher.finalize();

    let mut out = Vec::with_capacity(24 + 32 + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(mac.as_bytes());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_aead(session_key: &[u8], data: &[u8]) -> Result<Vec<u8>, SyncError> {
    if data.len() < 24 + 32 {
        return Err(SyncError::Pairing("ciphertext too short".into()));
    }

    let key = derive_aead_key(session_key);
    let nonce = &data[..24];
    let mac_received = &data[24..56];
    let ciphertext = &data[56..];

    let mut mac_hasher = blake3::Hasher::new_keyed(&key);
    mac_hasher.update(nonce);
    mac_hasher.update(ciphertext);
    let mac_computed = mac_hasher.finalize();

    if mac_computed.as_bytes() != mac_received {
        return Err(SyncError::Pairing("AEAD authentication failed".into()));
    }

    let mut stream_key = [0u8; 32];
    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(nonce);
    hasher.update(b"stream");
    let stream_hash = hasher.finalize();
    stream_key.copy_from_slice(stream_hash.as_bytes());

    let plaintext: Vec<u8> = ciphertext
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let ki = blake3::keyed_hash(&stream_key, &(i as u64).to_le_bytes());
            b ^ ki.as_bytes()[0]
        })
        .collect();

    Ok(plaintext)
}

fn derive_aead_key(session_key: &[u8]) -> [u8; 32] {
    // BLAKE3's keyed hash requires exactly 32 bytes, including padding.
    let hash = blake3::keyed_hash(b"rua-pairing-aead-key-derive-v1\0\0", session_key);
    *hash.as_bytes()
}

#[cfg(feature = "test-util")]
pub async fn establish_pair_local(
    pin: &str,
) -> Result<(WormholeChannel, WormholeChannel), SyncError> {
    use iroh::endpoint::presets;

    let rendezvous_secret = derive_rendezvous_secret(pin);

    let init_ep = Endpoint::builder(presets::Minimal)
        .secret_key(rendezvous_secret)
        .alpns(vec![PAIRING_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| SyncError::Pairing(format!("initiator bind: {e}")))?;

    let init_addrs = init_ep.bound_sockets();
    let rendezvous_node_id = init_ep.id();

    let join_ep = Endpoint::builder(presets::Minimal)
        .alpns(vec![PAIRING_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| SyncError::Pairing(format!("joiner bind: {e}")))?;

    let mut node_addr = iroh::EndpointAddr::new(rendezvous_node_id);
    for sock in init_addrs {
        node_addr.addrs.insert(iroh::TransportAddr::Ip(sock));
    }

    let pin_i = pin.to_string();
    let pin_j = pin.to_string();

    let (init_ch, join_ch) = tokio::try_join!(
        async {
            let conn = init_ep
                .accept()
                .await
                .ok_or_else(|| SyncError::Pairing("accept returned None".into()))?
                .await
                .map_err(|e| SyncError::Pairing(format!("accept: {e}")))?;
            let (send, recv) = conn
                .open_bi()
                .await
                .map_err(|e| SyncError::Pairing(format!("open bi: {e}")))?;
            WormholeChannel::handshake(send, recv, conn, Some(init_ep), &pin_i).await
        },
        async {
            let conn = join_ep
                .connect(node_addr, PAIRING_ALPN)
                .await
                .map_err(|e| SyncError::Pairing(format!("connect: {e}")))?;
            let (send, recv) = conn
                .accept_bi()
                .await
                .map_err(|e| SyncError::Pairing(format!("accept bi: {e}")))?;
            WormholeChannel::handshake(send, recv, conn, Some(join_ep), &pin_j).await
        }
    )?;

    Ok((init_ch, join_ch))
}

#[cfg(feature = "test-util")]
pub mod test_relay {
    use tokio::sync::mpsc;

    use crate::SyncError;

    pub struct LocalChannel {
        tx: mpsc::Sender<Vec<u8>>,
        rx: mpsc::Receiver<Vec<u8>>,
        session_key: Vec<u8>,
    }

    pub fn local_pair(session_key: Vec<u8>) -> (LocalChannel, LocalChannel) {
        let (tx1, rx1) = mpsc::channel(16);
        let (tx2, rx2) = mpsc::channel(16);
        (
            LocalChannel {
                tx: tx1,
                rx: rx2,
                session_key: session_key.clone(),
            },
            LocalChannel {
                tx: tx2,
                rx: rx1,
                session_key,
            },
        )
    }

    impl LocalChannel {
        pub async fn send_encrypted(&self, plaintext: &[u8]) -> Result<(), SyncError> {
            let ciphertext = super::encrypt_aead(&self.session_key, plaintext)?;
            self.tx
                .send(ciphertext)
                .await
                .map_err(|_| SyncError::Pairing("local channel closed".into()))?;
            Ok(())
        }

        pub async fn recv_encrypted(&mut self) -> Result<Vec<u8>, SyncError> {
            let ciphertext = self
                .rx
                .recv()
                .await
                .ok_or_else(|| SyncError::Pairing("local channel closed".into()))?;
            super::decrypt_aead(&self.session_key, &ciphertext)
        }
    }
}
