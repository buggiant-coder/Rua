//! Conversation control delivery: the single place that knows how a control
//! reaches its Chat authority.
//!
//! Callers express a complete [`ChatAddress`] plus the command intent; this module
//! owns local-vs-remote selection, the Project fixed-resource rule, redirect
//! verification, target/epoch rewriting, and the one-retry limit.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::ChatAddress;

use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::conversation_input_intake::ConversationInputIntake;
use crate::services::device_command_bus::{
    deliver_acp_chat_command_via_session, deliver_conversation_control_via_session,
    AcpChatDeviceCommand, AcpChatDeviceResponse, ConversationAuthorityRedirect,
    ConversationControlDeviceCommand, ConversationControlDeviceResponse,
    DeviceCommandDeliveryError,
};
use crate::services::device_session_access::current_session_manager;

const DEVICE_COMMAND_DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const PROJECT_AUTHORITY_FIXED_DETAIL: &str =
    "Project Chat authority is fixed to its resource Device";

/// Device-to-device delivery for controls whose authority lives elsewhere.
#[async_trait]
pub trait ConversationControlTransport: Send + Sync {
    async fn deliver_control(
        &self,
        command: &ConversationControlDeviceCommand,
    ) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError>;

    async fn deliver_acp(
        &self,
        command: &AcpChatDeviceCommand,
    ) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError>;
}

/// Production transport over the Device Session network.
///
/// The session manager is re-acquired on every delivery: the shared runtime cell
/// can be replaced between attempts, and a redirected retry must never reuse the
/// connection that produced the redirect.
#[derive(Clone)]
pub struct DeviceSessionControlTransport {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
}

impl DeviceSessionControlTransport {
    pub(crate) fn for_intake(intake: &ConversationInputIntake) -> Self {
        Self {
            device_network: Arc::clone(intake.device_network()),
        }
    }
}

#[async_trait]
impl ConversationControlTransport for DeviceSessionControlTransport {
    async fn deliver_control(
        &self,
        command: &ConversationControlDeviceCommand,
    ) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
        let session_manager = current_session_manager(&self.device_network)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_conversation_control_via_session(
            &session_manager,
            command,
            DEVICE_COMMAND_DELIVERY_TIMEOUT,
        )
        .await
    }

    async fn deliver_acp(
        &self,
        command: &AcpChatDeviceCommand,
    ) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
        let session_manager = current_session_manager(&self.device_network)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_acp_chat_command_via_session(
            &session_manager,
            command,
            DEVICE_COMMAND_DELIVERY_TIMEOUT,
        )
        .await
    }
}

pub async fn dispatch_conversation_control(
    intake: &ConversationInputIntake,
    command: ConversationControlDeviceCommand,
) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
    let transport = DeviceSessionControlTransport::for_intake(intake);
    dispatch_conversation_control_with(intake, &transport, command).await
}

pub async fn dispatch_acp_control(
    intake: &ConversationInputIntake,
    command: AcpChatDeviceCommand,
) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
    let transport = DeviceSessionControlTransport::for_intake(intake);
    dispatch_acp_control_with(intake, &transport, command).await
}

/// Remote-only conversation control delivery, including the verified single redirect.
pub async fn deliver_remote_conversation_control(
    intake: &ConversationInputIntake,
    command: ConversationControlDeviceCommand,
) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
    let transport = DeviceSessionControlTransport::for_intake(intake);
    deliver_remote_conversation_control_with(intake, &transport, command).await
}

/// Remote-only ACP control delivery, including the verified single redirect.
pub async fn deliver_remote_acp_control(
    intake: &ConversationInputIntake,
    command: AcpChatDeviceCommand,
) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
    let transport = DeviceSessionControlTransport::for_intake(intake);
    deliver_remote_acp_control_with(intake, &transport, command).await
}

pub async fn dispatch_conversation_control_with(
    intake: &ConversationInputIntake,
    transport: &dyn ConversationControlTransport,
    command: ConversationControlDeviceCommand,
) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
    if command.source_device_id == command.target_device_id {
        return Box::pin(intake.apply_conversation_control(command)).await;
    }
    deliver_remote_conversation_control_with(intake, transport, command).await
}

pub async fn dispatch_acp_control_with(
    intake: &ConversationInputIntake,
    transport: &dyn ConversationControlTransport,
    command: AcpChatDeviceCommand,
) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
    if command.source_device_id == command.target_device_id {
        return Box::pin(intake.apply_acp_control(command)).await;
    }
    deliver_remote_acp_control_with(intake, transport, command).await
}

pub async fn deliver_remote_conversation_control_with(
    intake: &ConversationInputIntake,
    transport: &dyn ConversationControlTransport,
    command: ConversationControlDeviceCommand,
) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
    deliver_with_redirect(intake, transport, command).await
}

pub async fn deliver_remote_acp_control_with(
    intake: &ConversationInputIntake,
    transport: &dyn ConversationControlTransport,
    command: AcpChatDeviceCommand,
) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
    deliver_with_redirect(intake, transport, command).await
}

#[async_trait]
trait RoutedControl: Send + Sync {
    type Response: Send;
    const KIND: &'static str;
    fn address(&self) -> &ChatAddress;
    fn source_device_id(&self) -> &str;
    fn epoch(&self) -> ConversationAuthorityEpoch;
    fn redirect_to(&mut self, placement: ConversationAuthorityPlacement);
    async fn deliver(
        &self,
        transport: &dyn ConversationControlTransport,
    ) -> Result<Self::Response, DeviceCommandDeliveryError>;
}

#[async_trait]
impl RoutedControl for ConversationControlDeviceCommand {
    type Response = ConversationControlDeviceResponse;
    const KIND: &'static str = "conversation control";
    fn address(&self) -> &ChatAddress {
        &self.address
    }
    fn source_device_id(&self) -> &str {
        &self.source_device_id
    }
    fn epoch(&self) -> ConversationAuthorityEpoch {
        self.authority_epoch
    }
    fn redirect_to(&mut self, placement: ConversationAuthorityPlacement) {
        self.target_device_id = placement.device_id;
        self.authority_epoch = placement.authority_epoch;
    }
    async fn deliver(
        &self,
        transport: &dyn ConversationControlTransport,
    ) -> Result<Self::Response, DeviceCommandDeliveryError> {
        transport.deliver_control(self).await
    }
}

#[async_trait]
impl RoutedControl for AcpChatDeviceCommand {
    type Response = AcpChatDeviceResponse;
    const KIND: &'static str = "ACP control";
    fn address(&self) -> &ChatAddress {
        &self.address
    }
    fn source_device_id(&self) -> &str {
        &self.source_device_id
    }
    fn epoch(&self) -> ConversationAuthorityEpoch {
        self.authority_epoch
    }
    fn redirect_to(&mut self, placement: ConversationAuthorityPlacement) {
        self.target_device_id = placement.device_id;
        self.authority_epoch = placement.authority_epoch;
    }
    async fn deliver(
        &self,
        transport: &dyn ConversationControlTransport,
    ) -> Result<Self::Response, DeviceCommandDeliveryError> {
        transport.deliver_acp(self).await
    }
}

async fn deliver_with_redirect<C: RoutedControl>(
    intake: &ConversationInputIntake,
    transport: &dyn ConversationControlTransport,
    mut command: C,
) -> Result<C::Response, DeviceCommandDeliveryError> {
    let redirect = match command.deliver(transport).await {
        Ok(response) => return Ok(response),
        Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => redirect,
        Err(error) => return Err(error),
    };
    let replacement = verify_control_redirect(
        intake,
        command.address(),
        command.source_device_id(),
        command.epoch(),
        &redirect,
    )
    .await?;
    command.redirect_to(replacement);
    match command.deliver(transport).await {
        Ok(response) => Ok(response),
        Err(DeviceCommandDeliveryError::AuthorityRedirect(second)) => {
            Err(redirect_limit(C::KIND, &second))
        }
        Err(error) => Err(error),
    }
}

async fn verify_control_redirect(
    intake: &ConversationInputIntake,
    address: &ChatAddress,
    source_device_id: &str,
    requested_epoch: ConversationAuthorityEpoch,
    redirect: &ConversationAuthorityRedirect,
) -> Result<ConversationAuthorityPlacement, DeviceCommandDeliveryError> {
    if address.project_id().is_some() {
        return Err(DeviceCommandDeliveryError::Rejected {
            reason: "project_authority_fixed".to_string(),
            detail: PROJECT_AUTHORITY_FIXED_DETAIL.to_string(),
        });
    }
    if &redirect.address != address {
        return Err(DeviceCommandDeliveryError::Rejected {
            reason: "invalid_authority_redirect".to_string(),
            detail: "authority Redirect targets a different Chat".to_string(),
        });
    }
    let replacement = intake
        .observe_authority_redirect(requested_epoch, redirect)
        .await
        .map_err(|detail| DeviceCommandDeliveryError::Rejected {
            reason: "authority_redirect_verification_failed".to_string(),
            detail,
        })?;
    if replacement.device_id == source_device_id {
        return Err(DeviceCommandDeliveryError::Rejected {
            reason: "authority_redirect_route_invalid".to_string(),
            detail: "verified authority Redirect unexpectedly targets the source Device"
                .to_string(),
        });
    }
    Ok(replacement)
}

fn redirect_limit(
    kind: &str,
    redirect: &ConversationAuthorityRedirect,
) -> DeviceCommandDeliveryError {
    DeviceCommandDeliveryError::Rejected {
        reason: "authority_redirect_limit".to_string(),
        detail: format!(
            "{kind} redirected more than once; latest target is Device '{}' at Epoch {}",
            redirect.placement.device_id, redirect.placement.authority_epoch
        ),
    }
}
