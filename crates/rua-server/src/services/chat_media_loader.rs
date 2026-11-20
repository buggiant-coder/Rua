use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub(crate) struct RuntimeChatMediaLoader {
    pub rua_root: PathBuf,
    pub device: Arc<RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    pub network: Arc<super::device_session_access::DeviceNetworkAccess>,
}

#[async_trait::async_trait]
impl rua_core::ChatMediaLoader for RuntimeChatMediaLoader {
    async fn open(
        &self,
        address: &rua_core::ChatAddress,
        id: uuid::Uuid,
    ) -> rua_core::Result<rua_core::ResolvedChatMedia> {
        let device =
            super::device_context::current_sync_device_id(&self.device).ok_or_else(|| {
                rua_core::RuaError::Runtime {
                    message: "Local media Device is unavailable".into(),
                }
            })?;
        let manager = self
            .network
            .lock()
            .await
            .as_ref()
            .map(|runtime| runtime.session_manager().clone());
        let media = super::chat_media::resolve_chat_media(
            &self.rua_root,
            address,
            &id.to_string(),
            &device,
            manager.as_ref(),
        )
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: error.to_string(),
        })?;
        Ok(rua_core::ResolvedChatMedia {
            path: media.path,
            mime_type: media.mime_type,
        })
    }
}
