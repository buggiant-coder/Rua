use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    (sk, vk)
}

pub fn sign_bytes(key: &SigningKey, data: &[u8]) -> Signature {
    key.sign(data)
}

pub fn verify_bytes(pubkey: &VerifyingKey, data: &[u8], sig: &Signature) -> bool {
    pubkey.verify(data, sig).is_ok()
}

pub fn load_signing_key_raw(bytes: &[u8]) -> Result<SigningKey, String> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("key must be 32 bytes, got {}", bytes.len()))?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn encode_pubkey(vk: &VerifyingKey) -> String {
    B64.encode(vk.as_bytes())
}

pub fn encode_privkey(sk: &SigningKey) -> String {
    B64.encode(sk.to_bytes())
}

pub fn decode_pubkey(b64: &str) -> Result<VerifyingKey, String> {
    let bytes = B64.decode(b64).map_err(|e| format!("bad base64: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "pubkey must decode to 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("invalid pubkey: {e}"))
}
