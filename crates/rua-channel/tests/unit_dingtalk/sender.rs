use rua_channel::dingtalk::DingTalkSender;

#[test]
fn build_text_body_uses_dingtalk_webhook_shape() {
    let body = DingTalkSender::build_text_body("hello");

    assert_eq!(body["msgtype"], "text");
    assert_eq!(body["text"]["content"], "hello");
}

#[test]
fn build_markdown_body_uses_dingtalk_webhook_shape() {
    let body = DingTalkSender::build_markdown_body("Title", "**bold**");

    assert_eq!(body["msgtype"], "markdown");
    assert_eq!(body["markdown"]["title"], "Title");
    assert_eq!(body["markdown"]["text"], "**bold**");
}

#[test]
fn build_media_body_formats_follow_rust_sender_contract() {
    let image = DingTalkSender::build_markdown_image_body("Image", "diagram", "@lALPD");
    assert_eq!(image["msgtype"], "markdown");
    assert_eq!(image["markdown"]["text"], "![diagram](@lALPD)");

    let fallback_alt = DingTalkSender::build_markdown_image_body("Image", "  ", "@lALPD");
    assert_eq!(fallback_alt["markdown"]["text"], "![image](@lALPD)");

    let file = DingTalkSender::build_file_body("@file");
    assert_eq!(file["msgtype"], "file");
    assert_eq!(file["file"]["media_id"], "@file");

    let video = DingTalkSender::build_video_body("@video", "mp4", 12, "@cover");
    assert_eq!(video["msgtype"], "video");
    assert_eq!(video["video"]["videoMediaId"], "@video");
    assert_eq!(video["video"]["videoType"], "mp4");
    assert_eq!(video["video"]["duration"], 12);
    assert_eq!(video["video"]["picMediaId"], "@cover");
}
