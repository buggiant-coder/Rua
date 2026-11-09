use std::collections::HashMap;

use rua_agent::model_call_context::ModelCallIdentityContext;
use rua_core::ChatAddress;
use serde_json::json;

#[test]
fn model_call_identity_is_a_chat_address_snapshot() {
    let address = ChatAddress::project("project-1", "chat-1");
    let mut metadata = HashMap::from([(
        rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.to_string(),
        serde_json::to_value(&address).unwrap(),
    )]);
    let snapshot = ModelCallIdentityContext::from_metadata(&metadata).expect("identity snapshot");

    metadata.clear();
    let first = snapshot.model_call();
    let second = snapshot.model_call();
    let mut first_metadata = serde_json::Map::new();
    let mut second_metadata = serde_json::Map::new();
    first.apply_to_metadata(&mut first_metadata);
    second.apply_to_metadata(&mut second_metadata);

    assert_eq!(
        first_metadata[rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY],
        json!(address)
    );
    assert_ne!(
        first_metadata[llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY],
        second_metadata[llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY]
    );
}

#[test]
fn missing_chat_address_does_not_create_an_identity_context() {
    assert!(ModelCallIdentityContext::from_metadata(&HashMap::new()).is_none());
}
