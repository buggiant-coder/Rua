use std::sync::Mutex;

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::services::conversation_control_dispatcher::{
    deliver_remote_acp_control_with, deliver_remote_conversation_control_with,
    ConversationControlTransport,
};
use rua_server::services::device_command_bus::{
    AcpChatDeviceAction, AcpChatDeviceCommand, AcpChatDeviceResponse,
    ConversationAuthorityRedirect, ConversationControlDeviceAction,
    ConversationControlDeviceCommand, ConversationControlDeviceResponse,
    DeviceCommandDeliveryError,
};
use rua_server::{AppState, ServerProfile};

struct Transport {
    redirect: Option<ConversationAuthorityRedirect>,
    controls: Mutex<Vec<ConversationControlDeviceCommand>>,
    acp: Mutex<Vec<AcpChatDeviceCommand>>,
}

#[async_trait]
impl ConversationControlTransport for Transport {
    async fn deliver_control(
        &self,
        command: &ConversationControlDeviceCommand,
    ) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
        self.controls.lock().unwrap().push(command.clone());
        match &self.redirect {
            Some(redirect) => Err(DeviceCommandDeliveryError::AuthorityRedirect(Box::new(
                redirect.clone(),
            ))),
            None => Ok(
                ConversationControlDeviceResponse::BackgroundResultDelivered {
                    observation_id: "observed".into(),
                },
            ),
        }
    }
    async fn deliver_acp(
        &self,
        command: &AcpChatDeviceCommand,
    ) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
        self.acp.lock().unwrap().push(command.clone());
        match &self.redirect {
            Some(redirect) => Err(DeviceCommandDeliveryError::AuthorityRedirect(Box::new(
                redirect.clone(),
            ))),
            None => Ok(AcpChatDeviceResponse::Runners { runners: vec![] }),
        }
    }
}

fn epoch(value: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(value, 0, [0; 8])
}

#[test]
fn discovery_can_omit_agent_hint() {
    let chat = "00000000-0000-0000-0000-000000001001";
    let (mut command, _) = commands(ChatAddress::new(chat));
    command.agent_id.clear();
    command.action = ConversationControlDeviceAction::Discover;
    assert!(command.validate_for_target("target").is_ok());
    command.action = ConversationControlDeviceAction::View;
    assert!(command.validate_for_target("target").is_err());
}

fn commands(address: ChatAddress) -> (ConversationControlDeviceCommand, AcpChatDeviceCommand) {
    (
        ConversationControlDeviceCommand {
            command_id: "original-control".into(),
            agent_id: "default".into(),
            address: address.clone(),
            source_device_id: "source".into(),
            target_device_id: "target".into(),
            authority_epoch: epoch(1),
            action: ConversationControlDeviceAction::View,
        },
        AcpChatDeviceCommand {
            command_id: "original-acp".into(),
            agent_id: "default".into(),
            address,
            source_device_id: "source".into(),
            target_device_id: "target".into(),
            authority_epoch: epoch(1),
            action: AcpChatDeviceAction::ListRunners,
        },
    )
}

fn transport(redirect_address: Option<ChatAddress>) -> Transport {
    Transport {
        redirect: redirect_address.map(|address| ConversationAuthorityRedirect {
            address,
            placement: ConversationAuthorityPlacement {
                device_id: "replacement".into(),
                authority_epoch: epoch(2),
                base_manifest_hash: format!("sha256:{}", "1".repeat(64)),
            },
        }),
        controls: Mutex::new(vec![]),
        acp: Mutex::new(vec![]),
    }
}

#[tokio::test]
async fn direct_delivery_preserves_both_typed_commands() {
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let (control, acp) = commands(ChatAddress::new("00000000-0000-0000-0000-000000001001"));
    let transport = transport(None);
    assert!(matches!(
        deliver_remote_conversation_control_with(
            &state.conversation_input_intake,
            &transport,
            control.clone()
        )
        .await
        .unwrap(),
        ConversationControlDeviceResponse::BackgroundResultDelivered { .. }
    ));
    assert!(matches!(
        deliver_remote_acp_control_with(&state.conversation_input_intake, &transport, acp.clone())
            .await
            .unwrap(),
        AcpChatDeviceResponse::Runners { .. }
    ));
    assert_eq!(*transport.controls.lock().unwrap(), vec![control]);
    assert_eq!(*transport.acp.lock().unwrap(), vec![acp]);
}

async fn rejects_redirect(
    address: ChatAddress,
    redirected: ChatAddress,
    project: bool,
    reason: &str,
) {
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    if project {
        let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
            chat_id: address.chat_id().to_string(),
            agent_id: "default".into(),
            title: "Project Chat".into(),
            created_at: "2026-08-20T00:00:00Z".into(),
            last_activity_at: "2026-08-20T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        });
        header.project_id = Some("project".into());
        rua_chat::apply_chat_defaults(&mut header);
        rua_session::WorkspaceSessionStore::new(state.rua_root.join("chats"))
            .save_chat_header(&header)
            .await
            .unwrap();
    }
    let (control, acp) = commands(address);
    let transport = transport(Some(redirected));
    for error in [
        deliver_remote_conversation_control_with(
            &state.conversation_input_intake,
            &transport,
            control,
        )
        .await
        .unwrap_err(),
        deliver_remote_acp_control_with(&state.conversation_input_intake, &transport, acp)
            .await
            .unwrap_err(),
    ] {
        assert!(
            matches!(error, DeviceCommandDeliveryError::Rejected { reason: ref actual, .. } if actual == reason),
            "{error:?}"
        );
    }
    assert_eq!(transport.controls.lock().unwrap().len(), 1);
    assert_eq!(transport.acp.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn project_redirect_is_rejected_without_retry() {
    let address = ChatAddress::new("00000000-0000-0000-0000-000000001001");
    rejects_redirect(address.clone(), address, true, "project_authority_fixed").await;
}

#[tokio::test]
async fn different_chat_id_is_not_a_valid_redirect() {
    let chat = "00000000-0000-0000-0000-000000001001";
    rejects_redirect(
        ChatAddress::new(chat),
        ChatAddress::new("00000000-0000-0000-0000-000000001002"),
        false,
        "invalid_authority_redirect",
    )
    .await;
}

#[tokio::test]
async fn stale_redirect_is_rejected_without_retry() {
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let address = ChatAddress::new("00000000-0000-0000-0000-000000001001");
    let (control, acp) = commands(address.clone());
    let mut transport = transport(Some(address));
    transport
        .redirect
        .as_mut()
        .unwrap()
        .placement
        .authority_epoch = epoch(1);
    for error in [
        deliver_remote_conversation_control_with(
            &state.conversation_input_intake,
            &transport,
            control,
        )
        .await
        .unwrap_err(),
        deliver_remote_acp_control_with(&state.conversation_input_intake, &transport, acp)
            .await
            .unwrap_err(),
    ] {
        assert!(
            matches!(error, DeviceCommandDeliveryError::Rejected { reason: ref actual, .. } if actual == "authority_redirect_verification_failed"),
            "{error:?}"
        );
    }
    assert_eq!(transport.controls.lock().unwrap().len(), 1);
    assert_eq!(transport.acp.lock().unwrap().len(), 1);
}
