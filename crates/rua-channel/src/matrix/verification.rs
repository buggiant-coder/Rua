//! Headless SAS handshake, matching QwenPaw: print the challenge, confirm the
//! bot side, and require the peer to compare and confirm in its Matrix client.
use matrix_sdk::ruma::events::AnyToDeviceEvent;
use matrix_sdk::{event_handler::EventHandlerDropGuard, Client};

pub(super) fn install(client: &Client) -> EventHandlerDropGuard {
    let verification_client = client.clone();
    let handle = client.add_event_handler(move |event: AnyToDeviceEvent| {
        let client = verification_client.clone();
        async move {
            use matrix_sdk::ruma::events::AnyToDeviceEvent::*;
            let result = match event {
                KeyVerificationRequest(event) => {
                    if let Some(request) = client.encryption().get_verification_request(
                        &event.sender, &event.content.transaction_id,
                    ).await {
                        request.accept().await
                    } else { Ok(()) }
                }
                KeyVerificationStart(event) => {
                    if let Some(sas) = client.encryption().get_verification(
                        &event.sender, event.content.transaction_id.as_str(),
                    ).await.and_then(|v| v.sas()) {
                        sas.accept().await
                    } else { Ok(()) }
                }
                KeyVerificationKey(event) => {
                    if let Some(sas) = client.encryption().get_verification(
                        &event.sender, event.content.transaction_id.as_str(),
                    ).await.and_then(|v| v.sas()) {
                        tracing::info!(
                            sender = %event.sender, device = %sas.other_device().device_id(),
                            emojis = ?sas.emoji(), decimals = ?sas.decimals(),
                            "Matrix SAS challenge: compare in your Matrix client before confirming there"
                        );
                        sas.confirm().await
                    } else { Ok(()) }
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                tracing::warn!(%error, "Matrix verification handshake failed");
            }
        }
    });
    client.event_handler_drop_guard(handle)
}
