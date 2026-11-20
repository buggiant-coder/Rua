use std::collections::{HashMap, HashSet};

use rua_core::{ChatAddress, ChatId};

#[test]
fn identity_uses_only_chat_id_and_ignores_extension_metadata() {
    let address = ChatAddress::new("shared-chat");
    let extended: ChatAddress = serde_json::from_value(serde_json::json!({
        "chat_id": "shared-chat",
        "future_field": {"owner": "different"}
    }))
    .unwrap();
    assert_eq!(address, extended);
    assert_eq!(address.identity_key(), extended.identity_key());
    assert_eq!(address.to_string(), extended.to_string());
    assert_eq!(HashSet::from([address.clone(), extended.clone()]).len(), 1);
    assert_eq!(HashMap::from([(address, 1), (extended, 2)]).len(), 1);
    assert_ne!(ChatAddress::new("first"), ChatAddress::new("second"));
}

#[test]
fn address_round_trip_preserves_unknown_fields_without_changing_identity() {
    let value = serde_json::json!({
        "chat_id": "chat-a",
        "future_address_field": ["future"]
    });
    let address: ChatAddress = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(address, ChatAddress::new("chat-a"));
    assert_eq!(serde_json::to_value(address).unwrap(), value);
    assert_eq!(
        serde_json::to_value(ChatAddress::new("chat-a")).unwrap(),
        serde_json::json!({"chat_id":"chat-a"})
    );
}

#[test]
fn address_exposes_independent_identity() {
    let address = ChatAddress::new("chat-a");
    assert_eq!(address.chat_id(), &ChatId::new("chat-a"));
    assert_eq!(address.identity_key(), "chat-a");
    assert_eq!(address.to_string(), "chat:chat-a");
}
