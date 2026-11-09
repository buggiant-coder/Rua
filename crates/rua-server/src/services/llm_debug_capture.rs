use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

const CAPTURE_STATE_FILE: &str = "llm_debug_capture.json";
const CAPTURE_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmDebugCaptureInfo {
    pub capture_id: String,
    pub path: PathBuf,
}

pub struct LlmDebugCaptureRegistry {
    root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedCaptureState {
    schema_version: u32,
    enabled: bool,
    chat_address: ChatAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_id: Option<String>,
    updated_at: String,
}

impl LlmDebugCaptureRegistry {
    pub fn new(data_dir: &Path) -> Self {
        let registry = Self {
            root: data_dir.join("debug").join("llm-captures"),
        };
        registry.restore_persisted_captures(data_dir);
        registry
    }

    pub fn get(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
    ) -> io::Result<Option<LlmDebugCaptureInfo>> {
        if let Some(capture) = llm_multiplex::debug_capture::active_capture(address) {
            return Ok(Some(Self::info(capture)));
        }
        let Some(state) = self.load_state(scope_root, address)? else {
            return Ok(None);
        };
        self.restore_state(state)
    }

    pub fn start(
        &self,
        scope_root: &Path,
        address: ChatAddress,
    ) -> io::Result<LlmDebugCaptureInfo> {
        let store = rua_session::WorkspaceSessionStore::new(scope_root);
        if !store.chat_header_path(address.chat_id().as_str()).is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Chat is not materialized: {address}"),
            ));
        }
        if let Some(existing) = self.get(scope_root, &address)? {
            self.write_enabled_state(scope_root, &address, &existing.capture_id)?;
            return Ok(existing);
        }

        let capture_id = uuid::Uuid::new_v4().to_string();
        let info = LlmDebugCaptureInfo {
            path: self.root.join(&capture_id),
            capture_id,
        };
        std::fs::create_dir_all(&info.path)?;
        let manifest = serde_json::json!({
            "schema_version": 1,
            "capture_id": info.capture_id,
            "chat_address": &address,
            "created_at": chrono::Utc::now().to_rfc3339(),
        });
        if let Err(error) =
            rua_config::atomic_write_json(&info.path.join("manifest.json"), &manifest)
        {
            let _ = std::fs::remove_dir_all(&info.path);
            return Err(error);
        }
        if let Err(error) = self.write_enabled_state(scope_root, &address, &info.capture_id) {
            let _ = fs::remove_dir_all(&info.path);
            return Err(error);
        }
        llm_multiplex::debug_capture::register_capture(
            address,
            llm_multiplex::debug_capture::ActiveCapture {
                capture_id: info.capture_id.clone(),
                capture_path: info.path.clone(),
            },
        );
        Ok(info)
    }

    pub fn stop(&self, scope_root: &Path, address: &ChatAddress) -> io::Result<()> {
        let store = rua_session::WorkspaceSessionStore::new(scope_root);
        let state_path = self.state_path(scope_root, address.chat_id().as_str());
        if store.chat_header_path(address.chat_id().as_str()).is_file() || state_path.is_file() {
            self.write_state(
                scope_root,
                PersistedCaptureState {
                    schema_version: CAPTURE_STATE_SCHEMA_VERSION,
                    enabled: false,
                    chat_address: address.clone(),
                    capture_id: None,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            )?;
        }
        llm_multiplex::debug_capture::unregister_capture(address);
        Ok(())
    }

    fn info(capture: llm_multiplex::debug_capture::ActiveCapture) -> LlmDebugCaptureInfo {
        LlmDebugCaptureInfo {
            capture_id: capture.capture_id,
            path: capture.capture_path,
        }
    }

    fn state_path(&self, scope_root: &Path, chat_id: &str) -> PathBuf {
        rua_session::WorkspaceSessionStore::new(scope_root)
            .chat_dir(chat_id)
            .join(CAPTURE_STATE_FILE)
    }

    fn load_state(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
    ) -> io::Result<Option<PersistedCaptureState>> {
        let path = self.state_path(scope_root, address.chat_id().as_str());
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let state: PersistedCaptureState =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if state.schema_version != CAPTURE_STATE_SCHEMA_VERSION
            || state.chat_address != *address
            || !state.enabled
        {
            return Ok(None);
        }
        Ok(Some(state))
    }

    fn write_state(&self, scope_root: &Path, state: PersistedCaptureState) -> io::Result<()> {
        let path = self.state_path(scope_root, state.chat_address.chat_id().as_str());
        let value = serde_json::to_value(state).map_err(io::Error::other)?;
        rua_config::atomic_write_json(&path, &value)
    }

    fn write_enabled_state(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
        capture_id: &str,
    ) -> io::Result<()> {
        self.write_state(
            scope_root,
            PersistedCaptureState {
                schema_version: CAPTURE_STATE_SCHEMA_VERSION,
                enabled: true,
                chat_address: address.clone(),
                capture_id: Some(capture_id.to_string()),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        )
    }

    fn restore_state(
        &self,
        state: PersistedCaptureState,
    ) -> io::Result<Option<LlmDebugCaptureInfo>> {
        if !state.enabled {
            return Ok(None);
        }
        let Some(capture_id) = state.capture_id.filter(|id| !id.trim().is_empty()) else {
            return Ok(None);
        };
        let info = LlmDebugCaptureInfo {
            path: self.root.join(&capture_id),
            capture_id,
        };
        fs::create_dir_all(&info.path)?;
        let manifest_path = info.path.join("manifest.json");
        if !manifest_path.is_file() {
            rua_config::atomic_write_json(
                &manifest_path,
                &serde_json::json!({
                    "schema_version": 1,
                    "capture_id": info.capture_id,
                    "chat_address": &state.chat_address,
                    "created_at": state.updated_at,
                }),
            )?;
        }
        llm_multiplex::debug_capture::register_capture(
            state.chat_address,
            llm_multiplex::debug_capture::ActiveCapture {
                capture_id: info.capture_id.clone(),
                capture_path: info.path.clone(),
            },
        );
        Ok(Some(info))
    }

    fn restore_persisted_captures(&self, data_dir: &Path) {
        for collection in [data_dir.join("workspaces"), data_dir.join("projects")] {
            let scopes = match fs::read_dir(&collection) {
                Ok(scopes) => scopes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(path = %collection.display(), %error, "failed to scan persisted LLM debug capture roots");
                    continue;
                }
            };
            for scope in scopes.flatten() {
                self.restore_session_states(&scope.path().join("sessions"));
            }
        }
    }

    fn restore_session_states(&self, sessions_dir: &Path) {
        let chats = match fs::read_dir(sessions_dir) {
            Ok(chats) => chats,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(path = %sessions_dir.display(), %error, "failed to scan persisted LLM debug capture sessions");
                return;
            }
        };
        for chat in chats.flatten() {
            let path = chat.path().join(CAPTURE_STATE_FILE);
            if !path.is_file() {
                continue;
            }
            let result = fs::read(&path)
                .and_then(|bytes| {
                    serde_json::from_slice::<PersistedCaptureState>(&bytes)
                        .map_err(io::Error::other)
                })
                .and_then(|state| self.restore_state(state).map(|_| ()));
            if let Err(error) = result {
                tracing::warn!(path = %path.display(), %error, "failed to restore persisted LLM debug capture");
            }
        }
    }
}
