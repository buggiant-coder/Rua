use rua_channel::dingtalk::ai_card::{
    build_create_card_body, build_create_card_body_with_initial_text, build_deliver_card_body,
    build_streaming_update_body,
};

#[test]
fn create_card_group_includes_at_user() {
    let body = build_create_card_body("tpl-id", "card_abc", "content", "staff-1", true);

    assert_eq!(body["cardTemplateId"], "tpl-id");
    assert_eq!(body["outTrackId"], "card_abc");
    assert_eq!(body["callbackType"], "STREAM");
    assert_eq!(body["cardData"]["cardParamMap"]["content"], "");
    assert_eq!(body["imGroupOpenSpaceModel"]["supportForward"], true);
    assert_eq!(body["imRobotOpenSpaceModel"]["supportForward"], true);
    assert_eq!(body["cardAtUserIds"][0], "staff-1");
    assert_eq!(body["userIdType"], 1);
}

#[test]
fn create_card_p2p_omits_at_user() {
    let body = build_create_card_body("tpl-id", "card_abc", "content", "staff-1", false);

    assert_eq!(body["cardTemplateId"], "tpl-id");
    assert_eq!(body["cardData"]["cardParamMap"]["content"], "");

    assert!(body.get("cardAtUserIds").is_none());
    assert!(body.get("userIdType").is_none());
}

#[test]
fn create_card_can_show_processing_text_before_first_delta() {
    let body = build_create_card_body_with_initial_text(
        "tpl-id",
        "card_abc",
        "content",
        "处理中...",
        "staff-1",
        true,
    );

    assert_eq!(body["cardData"]["cardParamMap"]["content"], "处理中...");
    assert_eq!(body["cardAtUserIds"][0], "staff-1");
}

#[test]
fn create_card_group_without_staff_omits_at_user() {
    let body = build_create_card_body("tpl-id", "card_abc", "content", "", true);
    assert!(body.get("cardAtUserIds").is_none());
    assert!(body.get("userIdType").is_none());
}

#[test]
fn deliver_card_group_uses_im_group_space() {
    let body = build_deliver_card_body("card_abc", "robot-1", "conv-9", "staff-1", true);

    assert_eq!(body["outTrackId"], "card_abc");
    assert_eq!(body["userIdType"], 1);
    assert_eq!(body["openSpaceId"], "dtv1.card//IM_GROUP.conv-9");
    assert_eq!(body["imGroupOpenDeliverModel"]["robotCode"], "robot-1");
    assert!(body.get("imRobotOpenDeliverModel").is_none());
}

#[test]
fn deliver_card_p2p_uses_im_robot_space() {
    let body = build_deliver_card_body("card_abc", "robot-1", "conv-9", "staff-1", false);

    assert_eq!(body["outTrackId"], "card_abc");
    assert_eq!(body["openSpaceId"], "dtv1.card//IM_ROBOT.staff-1");
    assert_eq!(body["imRobotOpenDeliverModel"]["spaceType"], "IM_ROBOT");
    assert!(body.get("imGroupOpenDeliverModel").is_none());
}

#[test]
fn streaming_update_fields() {
    let body = build_streaming_update_body("card_abc", "content", "hello", false);

    assert_eq!(body["outTrackId"], "card_abc");
    assert_eq!(body["key"], "content");
    assert_eq!(body["content"], "hello");
    assert_eq!(body["isFull"], true);
    assert_eq!(body["isFinalize"], false);
    assert_eq!(body["isError"], false);

    assert!(body["guid"]
        .as_str()
        .map(|s| !s.is_empty())
        .unwrap_or(false));
}

#[test]
fn streaming_update_finalize_sets_flag() {
    let body = build_streaming_update_body("card_abc", "content", "done", true);
    assert_eq!(body["isFinalize"], true);
}
