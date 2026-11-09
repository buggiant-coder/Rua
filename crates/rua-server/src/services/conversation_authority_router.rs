use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rua_channel::core::console::ConsoleChatRequest;
use rua_core::ChatAddress;

use crate::services::acp_chat_registry::AcpChatRegistry;
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::conversation_authority_target::{
    load_chat_authority_placement_target, resolve_submit_target, route_for_local_device,
    RemotePlacementRoute,
};
use crate::services::device_command_bus::{
    deliver_conversation_control_via_session, ConversationAuthorityRedirect,
    ConversationControlDeviceAction, ConversationControlDeviceCommand,
    ConversationControlDeviceResponse,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::device_session_access::current_session_manager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationSubmitRoute {
    LocalAuthority {
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
        base_manifest_hash: String,
        materialize_conversation: bool,
    },
    RemoteAuthority {
        source_device_id: String,
        target_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
        base_manifest_hash: String,
        materialize_conversation: bool,
    },
    Rejected {
        detail: String,
        target_device_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationControlRoute {
    LocalAuthority {
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    RemoteAuthority {
        source_device_id: String,
        target_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    Rejected {
        detail: String,
        target_device_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationLiveActivityRoute {
    LocalAuthority {
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    RemoteAuthority {
        source_device_id: String,
        target_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    Rejected {
        detail: String,
        target_device_id: Option<String>,
    },
}

pub struct ConversationSubmitRouteRequest<'a> {
    pub workspace_dir: Option<&'a Path>,
    pub address: &'a ChatAddress,
    pub request: &'a ConsoleChatRequest,
    pub execution_agent_id: &'a str,
    pub local_device_id: Option<&'a str>,
    pub acp_chat_registry: Option<&'a Arc<AcpChatRegistry>>,
}

pub struct ConversationControlRouteRequest<'a> {
    pub workspace_dir: Option<&'a Path>,
    pub address: &'a ChatAddress,
    pub local_device_id: Option<&'a str>,
}

pub struct ConversationLiveActivityRouteRequest<'a> {
    pub workspace_dir: Option<&'a Path>,
    pub address: &'a ChatAddress,
    pub local_device_id: Option<&'a str>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ConversationAuthorityRouter;

impl ConversationAuthorityRouter {
    pub fn route_fixed_placement(
        &self,
        placement: &ConversationAuthorityPlacement,
        local_device_id: Option<&str>,
    ) -> ConversationSubmitRoute {
        route_to_device(
            &placement.device_id,
            placement.authority_epoch,
            placement.base_manifest_hash.clone(),
            false,
            local_device_id,
        )
    }

    pub async fn resolve_submit_route(
        &self,
        request: ConversationSubmitRouteRequest<'_>,
    ) -> ConversationSubmitRoute {
        match resolve_submit_target(
            request.workspace_dir,
            request.address.chat_id().as_str(),
            request.request,
            request.execution_agent_id,
            request.local_device_id,
            request.acp_chat_registry,
        )
        .await
        {
            Ok(Some(target)) => {
                let materialize_conversation = target.source
                    == crate::services::conversation_authority_target::PlacementTargetSource::DraftTarget;
                match route_for_local_device(target.clone(), request.local_device_id) {
                    RemotePlacementRoute::Local {
                        authority_device_id,
                        authority_epoch,
                    } => ConversationSubmitRoute::LocalAuthority {
                        authority_device_id,
                        authority_epoch,
                        base_manifest_hash: target.base_manifest_hash,
                        materialize_conversation,
                    },
                    RemotePlacementRoute::Remote {
                        target_device_id,
                        source_device_id,
                        authority_epoch,
                        ..
                    } => ConversationSubmitRoute::RemoteAuthority {
                        source_device_id,
                        target_device_id,
                        authority_epoch,
                        base_manifest_hash: target.base_manifest_hash,
                        materialize_conversation,
                    },
                    RemotePlacementRoute::MissingSourceDevice {
                        target_device_id, ..
                    } => ConversationSubmitRoute::Rejected {
                        detail: "source device identity is unavailable".to_string(),
                        target_device_id: Some(target_device_id),
                    },
                }
            }
            Ok(None) => ConversationSubmitRoute::Rejected {
                detail: "conversation authority placement is unavailable".to_string(),
                target_device_id: None,
            },
            Err(error) => ConversationSubmitRoute::Rejected {
                detail: error,
                target_device_id: None,
            },
        }
    }

    pub async fn resolve_control_route(
        &self,
        request: ConversationControlRouteRequest<'_>,
    ) -> ConversationControlRoute {
        let Some(workspace_dir) = request.workspace_dir else {
            return ConversationControlRoute::Rejected {
                detail: "conversation workspace is unavailable".to_string(),
                target_device_id: None,
            };
        };
        match load_chat_authority_placement_target(
            workspace_dir,
            request.address.chat_id().as_str(),
        )
        .await
        {
            Ok(target) => match route_to_device(
                &target.device_id,
                target.authority_epoch,
                target.base_manifest_hash,
                false,
                request.local_device_id,
            ) {
                ConversationSubmitRoute::LocalAuthority {
                    authority_device_id,
                    authority_epoch,
                    ..
                } => ConversationControlRoute::LocalAuthority {
                    authority_device_id,
                    authority_epoch,
                },
                ConversationSubmitRoute::RemoteAuthority {
                    source_device_id,
                    target_device_id,
                    authority_epoch,
                    ..
                } => ConversationControlRoute::RemoteAuthority {
                    source_device_id,
                    target_device_id,
                    authority_epoch,
                },
                ConversationSubmitRoute::Rejected {
                    detail,
                    target_device_id,
                } => ConversationControlRoute::Rejected {
                    detail,
                    target_device_id,
                },
            },
            Err(error) => ConversationControlRoute::Rejected {
                detail: error,
                target_device_id: None,
            },
        }
    }

    pub async fn resolve_live_activity_route(
        &self,
        request: ConversationLiveActivityRouteRequest<'_>,
    ) -> ConversationLiveActivityRoute {
        let Some(workspace_dir) = request.workspace_dir else {
            return ConversationLiveActivityRoute::Rejected {
                detail: "conversation workspace is unavailable".to_string(),
                target_device_id: None,
            };
        };
        match load_chat_authority_placement_target(
            workspace_dir,
            request.address.chat_id().as_str(),
        )
        .await
        {
            Ok(target) => live_route_to_device(
                &target.device_id,
                target.authority_epoch,
                request.local_device_id,
            ),
            Err(error) => ConversationLiveActivityRoute::Rejected {
                detail: error,
                target_device_id: None,
            },
        }
    }
}

pub async fn verify_and_observe_authority_redirect(
    registry: &Arc<dyn rua_workspace::WorkspaceRegistry>,
    authority: &Arc<crate::services::conversation_authority::ConversationAuthority>,
    device_network: &Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    sync_device: &Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    requested_epoch: ConversationAuthorityEpoch,
    redirect: &ConversationAuthorityRedirect,
) -> Result<ConversationAuthorityPlacement, String> {
    redirect
        .placement
        .validate()
        .map_err(|error| error.to_string())?;
    if redirect.placement.authority_epoch <= requested_epoch {
        return Err("authority Redirect does not advance the requested Epoch".to_string());
    }
    let local_device_id = current_sync_device_id(sync_device)
        .ok_or_else(|| "local Device identity is unavailable".to_string())?;
    let runtime = registry
        .get_or_create(redirect.agent_id())
        .await
        .map_err(|error| error.to_string())?;
    let workspace_dir = runtime.workspace_dir();
    if redirect.placement.device_id == local_device_id {
        let current =
            load_chat_authority_placement_target(&workspace_dir, redirect.chat_id()).await?;
        if current.device_id != redirect.placement.device_id
            || current.authority_epoch != redirect.placement.authority_epoch
            || current.base_manifest_hash != redirect.placement.base_manifest_hash
        {
            return Err("local redirected authority does not match its Chat Index".to_string());
        }
    } else {
        let session_manager = current_session_manager(device_network).await?;
        let response = deliver_conversation_control_via_session(
            &session_manager,
            &ConversationControlDeviceCommand {
                command_id: format!("authority-redirect-verify-{}", uuid::Uuid::new_v4()),
                agent_id: redirect.agent_id().to_string(),
                address: redirect.address.clone(),
                source_device_id: local_device_id,
                target_device_id: redirect.placement.device_id.clone(),
                authority_epoch: redirect.placement.authority_epoch,
                action: ConversationControlDeviceAction::View,
            },
            Duration::from_secs(10),
        )
        .await
        .map_err(|error| error.to_string())?;
        let ConversationControlDeviceResponse::View { control, .. } = response else {
            return Err(
                "redirect verification returned an unexpected Control response".to_string(),
            );
        };
        if control.placement.authority_device_id.as_deref()
            != Some(redirect.placement.device_id.as_str())
            || control.placement.authority_epoch != Some(redirect.placement.authority_epoch)
        {
            return Err(
                "redirected authority Control View does not match the Redirect".to_string(),
            );
        }
    }
    if let Some(current) =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            &workspace_dir,
            redirect.chat_id(),
        )
        .await
        .map_err(|error| error.to_string())?
    {
        if current.authority_epoch > redirect.placement.authority_epoch {
            return Err("local Chat Session already has a newer authority Epoch".to_string());
        }
        if current.authority_epoch == redirect.placement.authority_epoch {
            if current == redirect.placement {
                return Ok(current);
            }
            return Err("same authority Epoch names a different placement".to_string());
        }
    }
    authority
        .apply_confirmed_authority_placement(
            &workspace_dir,
            &redirect.address,
            redirect.placement.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(redirect.placement.clone())
}

fn route_to_device(
    device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
    base_manifest_hash: String,
    materialize_conversation: bool,
    local_device_id: Option<&str>,
) -> ConversationSubmitRoute {
    if local_device_id == Some(device_id) {
        return ConversationSubmitRoute::LocalAuthority {
            authority_device_id: device_id.to_string(),
            authority_epoch,
            base_manifest_hash,
            materialize_conversation,
        };
    }
    let Some(source_device_id) = local_device_id else {
        return ConversationSubmitRoute::Rejected {
            detail: "source device identity is unavailable".to_string(),
            target_device_id: Some(device_id.to_string()),
        };
    };
    ConversationSubmitRoute::RemoteAuthority {
        source_device_id: source_device_id.to_string(),
        target_device_id: device_id.to_string(),
        authority_epoch,
        base_manifest_hash,
        materialize_conversation,
    }
}

fn live_route_to_device(
    device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
    local_device_id: Option<&str>,
) -> ConversationLiveActivityRoute {
    match route_to_device(
        device_id,
        authority_epoch,
        String::new(),
        false,
        local_device_id,
    ) {
        ConversationSubmitRoute::LocalAuthority {
            authority_device_id,
            authority_epoch,
            ..
        } => ConversationLiveActivityRoute::LocalAuthority {
            authority_device_id,
            authority_epoch,
        },
        ConversationSubmitRoute::RemoteAuthority {
            source_device_id,
            target_device_id,
            authority_epoch,
            ..
        } => ConversationLiveActivityRoute::RemoteAuthority {
            source_device_id,
            target_device_id,
            authority_epoch,
        },
        ConversationSubmitRoute::Rejected {
            detail,
            target_device_id,
        } => ConversationLiveActivityRoute::Rejected {
            detail,
            target_device_id,
        },
    }
}
