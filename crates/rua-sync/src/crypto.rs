use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng as AeadOsRng},
    AeadCore, ChaCha20Poly1305,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use rand::RngCore;

use crate::SyncError;

#[derive(Clone)]
pub struct SyncKey([u8; 32]);

#[derive(Clone)]
pub struct DeviceKey([u8; 32]);

impl SyncKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl DeviceKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

pub fn symmetric_encrypt(plaintext: &[u8], key: &SyncKey) -> Result<Vec<u8>, SyncError> {
    let cipher = ChaCha20Poly1305::new(key.0.as_ref().into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut AeadOsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| SyncError::Crypto(format!("symmetric encrypt: {e}")))?;
    let mut out = Vec::with_capacity(nonce.len() + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn symmetric_encrypt_with_nonce(
    plaintext: &[u8],
    key: &SyncKey,
    nonce: &[u8; 12],
) -> Result<Vec<u8>, SyncError> {
    let cipher = ChaCha20Poly1305::new(key.0.as_ref().into());
    let ciphertext = cipher
        .encrypt(nonce.as_slice().into(), plaintext)
        .map_err(|e| SyncError::Crypto(format!("symmetric encrypt: {e}")))?;
    let mut out = Vec::with_capacity(nonce.len() + ciphertext.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn hmac_key(sync_key: &SyncKey, real_key: &str) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(sync_key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(real_key.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn symmetric_decrypt(ciphertext: &[u8], key: &SyncKey) -> Result<Vec<u8>, SyncError> {
    const NONCE_LEN: usize = 12;
    if ciphertext.len() < NONCE_LEN {
        return Err(SyncError::Crypto("ciphertext too short".into()));
    }
    let (nonce_bytes, ct) = ciphertext.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(key.0.as_ref().into());
    cipher
        .decrypt(nonce_bytes.into(), ct)
        .map_err(|e| SyncError::Crypto(format!("symmetric decrypt: {e}")))
}
