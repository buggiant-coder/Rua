use rua_channel::core::content::OutgoingContentPart;
use rua_channel::core::RenderStyle;
use rua_channel::dingtalk::renderer::{
    normalize_dingtalk_markdown, DingTalkPayload, MessageRenderer,
};

#[test]
fn markdown_card_from_simple_text() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let parts = vec![OutgoingContentPart::Markdown {
        text: "Hello **world**".into(),
    }];

    let payloads = renderer.render(&parts);

    assert_eq!(payloads.len(), 1);
    let json = payloads[0].to_json();
    assert_eq!(json["msgtype"], "markdown");
    assert!(json["markdown"]["text"]
        .as_str()
        .unwrap()
        .contains("Hello **world**"));
}

#[test]
fn long_code_block_produces_action_card_with_console_link() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown)
        .with_console_url("https://console.example.com".into());
    let long_code = "x\n".repeat(500);
    let text = format!("Here is code:\n\n```python\n{long_code}```\n\nDone.");
    let parts = vec![OutgoingContentPart::Markdown { text }];

    let payloads = renderer.render(&parts);

    assert_eq!(payloads.len(), 1);
    let json = payloads[0].to_json();
    assert_eq!(json["msgtype"], "actionCard");
    assert_eq!(
        json["actionCard"]["singleURL"],
        "https://console.example.com"
    );
    assert!(json["actionCard"]["text"]
        .as_str()
        .unwrap()
        .contains("代码已省略"));
}

#[test]
fn short_code_block_stays_markdown() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let parts = vec![OutgoingContentPart::Markdown {
        text: "```rust\nfn main() {}\n```".into(),
    }];

    let payloads = renderer.render(&parts);

    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0].to_json()["msgtype"], "markdown");
}

#[test]
fn table_markdown_degrades_to_list_for_dingtalk() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let table = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
    let parts = vec![OutgoingContentPart::Markdown { text: table.into() }];

    let payloads = renderer.render(&parts);

    let json = payloads[0].to_json();
    let text = json["markdown"]["text"].as_str().unwrap();
    assert!(text.contains("**Name**: Alice"));
    assert!(text.contains("**Age**: 30"));
    assert!(!text.contains("| --- |"));
}

#[test]
fn very_long_text_falls_back_to_plain_text_payload() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let long_text = "a".repeat(4000);
    let parts = vec![OutgoingContentPart::Text { text: long_text }];

    let payloads = renderer.render(&parts);

    assert_eq!(payloads[0].to_json()["msgtype"], "text");
}

#[test]
fn thinking_parts_are_filtered_from_dingtalk_payloads() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let parts = vec![
        OutgoingContentPart::Thinking {
            text: "internal reasoning".into(),
        },
        OutgoingContentPart::Text {
            text: "visible".into(),
        },
    ];

    let payloads = renderer.render(&parts);

    assert_eq!(payloads.len(), 1);
    let json = payloads[0].to_json();
    let text = json["markdown"]["text"].as_str().unwrap();
    assert!(!text.contains("internal reasoning"));
    assert!(text.contains("visible"));
}

#[test]
fn empty_parts_produce_no_payloads() {
    let renderer = MessageRenderer::new(RenderStyle::Markdown);

    assert!(renderer.render(&[]).is_empty());
}

#[test]
fn markdown_normalization_applies_list_spacing_dedent_and_table_conversion() {
    let normalized = normalize_dingtalk_markdown("Image: `xxx`\n3. **Make sure**");
    assert_eq!(normalized, "Image: `xxx`\n\n3. **Make sure**");

    let normalized = normalize_dingtalk_markdown("1. First\n2. Second");
    assert_eq!(normalized, "1. First\n2. Second");

    let normalized =
        normalize_dingtalk_markdown("    ```python\n    def foo():\n        pass\n```");
    assert!(normalized.starts_with("```python"));
    assert!(normalized.contains("def foo():"));

    let normalized = normalize_dingtalk_markdown("| A | B |\n| --- | --- |\n| 1 | 2 |");
    assert!(normalized.contains("**A**: 1"));
    assert!(normalized.contains("**B**: 2"));
}

#[test]
fn payload_to_json_shapes_are_stable() {
    let markdown = DingTalkPayload::Markdown {
        title: "t".into(),
        text: "body".into(),
    }
    .to_json();
    assert_eq!(markdown["msgtype"], "markdown");
    assert_eq!(markdown["markdown"]["title"], "t");
    assert_eq!(markdown["markdown"]["text"], "body");

    let action_card = DingTalkPayload::ActionCard {
        title: "t".into(),
        text: "body".into(),
        single_title: "btn".into(),
        single_url: "https://x".into(),
    }
    .to_json();
    assert_eq!(action_card["msgtype"], "actionCard");
    assert_eq!(action_card["actionCard"]["singleTitle"], "btn");

    let image = DingTalkPayload::Image {
        media_id: "mid123".into(),
    }
    .to_json();
    assert_eq!(image["msgtype"], "image");
    assert_eq!(image["image"]["media_id"], "mid123");
}
