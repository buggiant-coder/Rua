use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A user-selected Device and its optional working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionDirectorySelection {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ExecutionDirectorySelection {
    pub fn validate(&self) -> crate::Result<()> {
        if self.device_id.trim().is_empty() || self.device_id.trim() != self.device_id {
            return Err(crate::RuaError::Validation {
                message: "expected a Device ID".into(),
            });
        }
        if let Some(path) = &self.path {
            if !path.to_str().is_some_and(is_device_absolute_path) {
                return Err(crate::RuaError::Validation {
                    message: "expected an absolute working directory on the execution Device"
                        .into(),
                });
            }
        }
        Ok(())
    }
}

// A replica may run a different operating system from the validating Device.
pub fn is_device_absolute_path(path: &str) -> bool {
    if path.contains('\0') {
        return false;
    }
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
        || path.strip_prefix(r"\\").is_some_and(|unc| {
            let mut components = unc.split('\\');
            components.next().is_some_and(|server| !server.is_empty())
                && components.next().is_some_and(|share| !share.is_empty())
        })
}
