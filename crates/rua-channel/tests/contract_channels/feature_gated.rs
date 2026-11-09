#[cfg(any(
    all(feature = "imessage", target_os = "macos"),
    feature = "matrix",
    feature = "mqtt"
))]
use rua_channel::core::channel::Channel;
#[cfg(any(
    all(feature = "imessage", target_os = "macos"),
    feature = "matrix",
    feature = "mqtt"
))]
use rua_channel::core::content::IncomingContentPart;

#[cfg(any(
    all(feature = "imessage", target_os = "macos"),
    feature = "matrix",
    feature = "mqtt"
))]
fn assert_modern_channel<T: Channel>() {}

#[cfg(feature = "mqtt")]
#[test]
fn mqtt_required_feature_adapter_has_stable_contract() {
    use rua_channel::mqtt::{MqttChannel, MqttConfig};

    assert_modern_channel::<MqttChannel>();

    let channel = MqttChannel::new(MqttConfig::default());
    assert_eq!(channel.id(), "mqtt");

    let req = channel
        .build_request_from_native(serde_json::json!({
            "payload": "hello from mqtt",
            "topic": "agents/default/inbound",
            "agent_id": "agent-1",
            "metadata": {
                "source": "contract-test"
            }
        }))
        .expect("mqtt native normalization");

    assert_eq!(req.channel_id, "mqtt");
    assert_eq!(req.agent_id, "agent-1");
    assert_eq!(req.user_id, "default");
    assert_eq!(req.session_id, "agents/default/inbound");
    assert_eq!(req.conversation_id, "agents/default/inbound");
    assert_eq!(req.metadata["source"], "contract-test");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello from mqtt"),
        other => panic!("mqtt normalized to non-text input: {other:?}"),
    }
}

#[cfg(feature = "matrix")]
#[test]
fn matrix_required_feature_adapter_has_stable_contract() {
    use rua_channel::matrix::{MatrixChannel, MatrixConfig};

    assert_modern_channel::<MatrixChannel>();

    let channel = MatrixChannel::new(MatrixConfig {
        default_room_id: "!fallback:example.com".into(),
        ..Default::default()
    });
    assert_eq!(channel.id(), "matrix");

    let req = channel
        .build_request_from_native(serde_json::json!({
            "body": "hello matrix",
            "sender": "@alice:example.com",
            "room_id": "!room:example.com",
            "event_id": "$event123"
        }))
        .expect("matrix native normalization");

    assert_eq!(req.channel_id, "matrix");
    assert_eq!(req.user_id, "@alice:example.com");
    assert_eq!(req.session_id, "!room:example.com");
    assert_eq!(req.conversation_id, "!room:example.com");
    assert_eq!(req.message_id, Some("$event123".into()));
    assert_eq!(req.metadata["sender_id"], "@alice:example.com");
    assert_eq!(req.metadata["room_id"], "!room:example.com");
    assert_eq!(req.metadata["event_id"], "$event123");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello matrix"),
        other => panic!("matrix normalized to non-text input: {other:?}"),
    }
}

#[cfg(all(feature = "imessage", target_os = "macos"))]
#[test]
fn imessage_required_feature_adapter_has_stable_contract() {
    use rua_channel::imessage::{IMessageChannel, IMessageConfig};

    assert_modern_channel::<IMessageChannel>();

    let channel = IMessageChannel::new(IMessageConfig::default());
    assert_eq!(channel.id(), "imessage");

    #[cfg(target_os = "macos")]
    {
        let req = channel
            .build_request_from_native(serde_json::json!({
                "sender_id": "+1234567890",
                "text": "hello imessage",
                "rowid": 42
            }))
            .expect("macOS iMessage native normalization");

        assert_eq!(req.channel_id, "imessage");
        assert_eq!(req.user_id, "+1234567890");
        assert_eq!(req.session_id, "+1234567890");
        assert_eq!(req.conversation_id, "+1234567890");
        assert_eq!(req.message_id, Some("42".into()));
        match &req.input[0] {
            IncomingContentPart::Text { text } => assert_eq!(text, "hello imessage"),
            other => panic!("imessage normalized to non-text input: {other:?}"),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let err = channel
            .build_request_from_native(serde_json::json!({
                "sender_id": "+1234567890",
                "text": "hello",
                "rowid": 42
            }))
            .expect_err("non-macOS iMessage stub does not normalize native payloads");
        assert!(
            format!("{err}").contains("iMessage channel requires macOS"),
            "unexpected iMessage stub error: {err}"
        );
    }
}
