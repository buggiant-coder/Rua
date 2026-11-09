use std::io::{Read, Write};

use age::secrecy::SecretString;

use crate::SyncError;

pub fn encrypt_for_export(key_bytes: &[u8; 32], passphrase: &str) -> Result<String, SyncError> {
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_owned()));

    let mut armored_output = Vec::new();
    let armor_writer =
        age::armor::ArmoredWriter::wrap_output(&mut armored_output, age::armor::Format::AsciiArmor)
            .map_err(|e| SyncError::Crypto(format!("armor init: {e}")))?;

    let mut writer = encryptor
        .wrap_output(armor_writer)
        .map_err(|e| SyncError::Crypto(format!("encrypt init: {e}")))?;

    writer
        .write_all(key_bytes)
        .map_err(|e| SyncError::Crypto(format!("encrypt write: {e}")))?;

    let armor_writer = writer
        .finish()
        .map_err(|e| SyncError::Crypto(format!("encrypt finish: {e}")))?;

    armor_writer
        .finish()
        .map_err(|e| SyncError::Crypto(format!("armor finish: {e}")))?;

    String::from_utf8(armored_output)
        .map_err(|e| SyncError::Crypto(format!("armored output not utf8: {e}")))
}

pub fn decrypt_export(armored: &str, passphrase: &str) -> Result<[u8; 32], SyncError> {
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));

    let armor_reader = age::armor::ArmoredReader::new(armored.as_bytes());

    let decryptor = age::Decryptor::new(armor_reader)
        .map_err(|e| SyncError::Crypto(format!("decrypt init: {e}")))?;

    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|e| SyncError::Crypto(format!("decrypt: {e}")))?;

    let mut plaintext = Vec::new();
    reader
        .read_to_end(&mut plaintext)
        .map_err(|e| SyncError::Crypto(format!("decrypt read: {e}")))?;

    let bytes: [u8; 32] = plaintext
        .try_into()
        .map_err(|_| SyncError::Crypto("decrypted key is not 32 bytes".into()))?;

    Ok(bytes)
}

pub fn render_qr_svg(text: &str) -> Result<String, SyncError> {
    use qrcode::QrCode;

    let code =
        QrCode::new(text.as_bytes()).map_err(|e| SyncError::Crypto(format!("qr encode: {e}")))?;

    let svg = code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();

    Ok(svg)
}
