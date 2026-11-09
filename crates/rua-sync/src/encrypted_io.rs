use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::crypto::DeviceKey;

pub struct EncryptedFileIO;

impl EncryptedFileIO {
    pub fn read_json<T: DeserializeOwned>(
        path: &Path,
        key: Option<&DeviceKey>,
    ) -> std::io::Result<T> {
        rua_config::encrypted_io::EncryptedFileIO::read_json(path, key.map(|k| k.as_bytes()))
    }

    pub fn write_json<T: Serialize>(
        path: &Path,
        value: &T,
        key: Option<&DeviceKey>,
    ) -> std::io::Result<()> {
        rua_config::encrypted_io::EncryptedFileIO::write_json(
            path,
            value,
            key.map(|k| k.as_bytes()),
        )
    }
}
