use rua_core::{Result, RuaError};

pub(crate) fn read_json_file<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(|e| RuaError::Config {
                message: format!("Failed to parse {}: {e}", path.display()),
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RuaError::Io {
            message: format!("Failed to read {}: {e}", path.display()),
        }),
    }
}
