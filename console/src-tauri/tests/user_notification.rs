use rua_desktop_lib::user_notification::{
    notification_body, valid_rua_uri, UserNotificationRequest, UserNotificationSender,
};

fn request(body: Option<&str>, rua_uri: Option<&str>) -> UserNotificationRequest {
    UserNotificationRequest {
        target_device_id: "device-a".to_string(),
        sender: UserNotificationSender {
            id: "agent:researcher".to_string(),
            name: "Research Agent".to_string(),
        },
        title: "Run complete".to_string(),
        body: body.map(str::to_string),
        rua_uri: rua_uri.map(str::to_string),
    }
}

#[test]
fn native_notification_keeps_sender_and_body_distinct() {
    assert_eq!(
        notification_body(&request(Some("Analysis ready"), None)),
        "Research Agent\nAnalysis ready"
    );
    assert_eq!(notification_body(&request(None, None)), "Research Agent");
}

#[test]
fn notification_activation_accepts_only_safe_rua_uris_without_rewriting() {
    let uri = "rua://agents/researcher/chats/chat-42?turn=7#result";
    assert_eq!(valid_rua_uri(uri), Some(uri));

    for invalid in [
        "",
        " rua://agents/default/chats/a",
        "https://rua.example",
        "rua:/missing-host",
        "rua://user:secret@agents/default",
    ] {
        assert_eq!(valid_rua_uri(invalid), None, "uri={invalid:?}");
    }
}
