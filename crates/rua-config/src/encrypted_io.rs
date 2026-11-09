use std::path::Path;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;

use crate::migration::atomic::atomic_write;

const ENCRYPTED_HEADER: u8 = 0x01;
const ENVELOPE_VERSION: u8 = 1;
const FIELD_NOTICE: &str = "This Rua secret field is encrypted. Do not edit this field directly. Other JSON fields may be edited normally.";

pub struct EncryptedFileIO;

pub struct EncryptedFieldIO;

impl EncryptedFileIO {
    pub fn read_json<T: DeserializeOwned>(
        path: &Path,
        key: Option<&[u8; 32]>,
    ) -> std::io::Result<T> {
        let bytes = std::fs::read(path)?;
        if bytes.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "empty file",
            ));
        }

        let json_bytes = if bytes[0] == ENCRYPTED_HEADER {
            let key = key.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "file is encrypted but no key provided",
                )
            })?;
            decrypt_payload(&bytes[1..], key)?
        } else {
            bytes
        };

        serde_json::from_slice(&json_bytes).map_err(std::io::Error::other)
    }

    pub fn is_encrypted_file(path: &Path) -> std::io::Result<bool> {
        let bytes = std::fs::read(path)?;
        Ok(bytes.first().copied() == Some(ENCRYPTED_HEADER))
    }

    pub fn write_json<T: Serialize>(
        path: &Path,
        value: &T,
        key: Option<&[u8; 32]>,
    ) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let data = match key {
            Some(k) => {
                let json_bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
                let encrypted = encrypt_payload(&json_bytes, k)?;
                let mut out = Vec::with_capacity(1 + encrypted.len());
                out.push(ENCRYPTED_HEADER);
                out.extend_from_slice(&encrypted);
                out
            }
            None => serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?,
        };

        atomic_write(path, &data)
    }
}

impl EncryptedFieldIO {
    pub fn encrypt_string(
        plaintext: &str,
        key: &[u8; 32],
        field: &str,
        tool_hint: impl Into<String>,
    ) -> std::io::Result<serde_json::Value> {
        let (nonce, ciphertext) = encrypt_payload_parts(plaintext.as_bytes(), key)?;
        Ok(serde_json::json!({
            "rua_encrypted": true,
            "version": ENVELOPE_VERSION,
            "notice": FIELD_NOTICE,
            "field": field,
            "tool_hint": tool_hint.into(),
            "algorithm": "ChaCha20-Poly1305",
            "nonce": B64.encode(nonce),
            "ciphertext": B64.encode(ciphertext),
        }))
    }

    pub fn decrypt_string(value: &serde_json::Value, key: &[u8; 32]) -> std::io::Result<String> {
        let envelope: EncryptedFieldEnvelope =
            serde_json::from_value(value.clone()).map_err(std::io::Error::other)?;
        if envelope.version != ENVELOPE_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported encrypted field version {}", envelope.version),
            ));
        }
        let nonce = B64
            .decode(&envelope.nonce)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let ciphertext = B64
            .decode(&envelope.ciphertext)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if nonce.len() != 12 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "encrypted field nonce must be 12 bytes",
            ));
        }
        let cipher = ChaCha20Poly1305::new(key.into());
        let plaintext = cipher
            .decrypt(nonce.as_slice().into(), ciphertext.as_slice())
            .map_err(|e| std::io::Error::other(format!("decrypt: {e}")))?;
        String::from_utf8(plaintext).map_err(std::io::Error::other)
    }

    pub fn is_encrypted_value(value: &serde_json::Value) -> bool {
        value
            .get("rua_encrypted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && value.get("ciphertext").is_some()
    }
}

#[derive(Debug, Deserialize)]
struct EncryptedFieldEnvelope {
    version: u8,
    nonce: String,
    ciphertext: String,
}

fn encrypt_payload(plaintext: &[u8], key: &[u8; 32]) -> std::io::Result<Vec<u8>> {
    let (nonce, ciphertext) = encrypt_payload_parts(plaintext, key)?;
    let mut out = Vec::with_capacity(nonce.len() + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn encrypt_payload_parts(plaintext: &[u8], key: &[u8; 32]) -> std::io::Result<([u8; 12], Vec<u8>)> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| std::io::Error::other(format!("encrypt: {e}")))?;
    Ok((nonce.into(), ciphertext))
}

fn decrypt_payload(data: &[u8], key: &[u8; 32]) -> std::io::Result<Vec<u8>> {
    const NONCE_LEN: usize = 12;
    if data.len() < NONCE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "encrypted payload too short",
        ));
    }
    let (nonce_bytes, ct) = data.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(nonce_bytes.into(), ct)
        .map_err(|e| std::io::Error::other(format!("decrypt: {e}")))
}
