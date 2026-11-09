use rua_core::message::ContentPart;
use rua_server::services::console_chat::inject_file_path_hints;

fn file_part(name: &str, url: &str) -> ContentPart {
    ContentPart::File {
        url: url.to_string(),
        name: name.to_string(),
        media_type: None,
        size: None,
    }
}

fn text_part(text: &str) -> ContentPart {
    ContentPart::Text {
        text: text.to_string(),
    }
}

#[test]
fn injects_text_after_single_file() {
    let mut content = vec![text_part("看下这个"), file_part("1.md", "/abs/1.md")];
    inject_file_path_hints(&mut content);
    assert_eq!(content.len(), 3);
    assert!(matches!(&content[0], ContentPart::Text { text } if text == "看下这个"));
    assert!(matches!(&content[1], ContentPart::File { name, .. } if name == "1.md"));
    assert!(
        matches!(&content[2], ContentPart::Text { text } if text.contains("/abs/1.md")),
        "expected injected text mentioning the path, got {:?}",
        content[2]
    );
}

#[test]
fn injects_text_after_each_of_multiple_files() {
    let mut content = vec![
        text_part("两份"),
        file_part("a.pdf", "/p/a.pdf"),
        file_part("b.csv", "/p/b.csv"),
    ];
    inject_file_path_hints(&mut content);

    assert_eq!(content.len(), 5);
    assert!(matches!(&content[1], ContentPart::File { name, .. } if name == "a.pdf"));
    assert!(matches!(&content[2], ContentPart::Text { text } if text.contains("/p/a.pdf")));
    assert!(matches!(&content[3], ContentPart::File { name, .. } if name == "b.csv"));
    assert!(matches!(&content[4], ContentPart::Text { text } if text.contains("/p/b.csv")));
}

#[test]
fn no_op_when_no_file_blocks() {
    let mut content = vec![text_part("hi")];
    inject_file_path_hints(&mut content);
    assert_eq!(content.len(), 1);
    assert!(matches!(&content[0], ContentPart::Text { text } if text == "hi"));
}

#[test]
fn injects_text_after_image_audio_video_blocks() {
    let mut content = vec![
        ContentPart::Image {
            url: "/p/i.png".into(),
            media_type: None,
            alt: None,
        },
        ContentPart::Audio {
            url: "/p/a.mp3".into(),
            media_type: None,
        },
        ContentPart::Video {
            url: "/p/v.mp4".into(),
            media_type: None,
        },
    ];
    inject_file_path_hints(&mut content);

    assert_eq!(content.len(), 6);
    assert!(matches!(&content[0], ContentPart::Image { .. }));
    assert!(matches!(&content[1], ContentPart::Text { text } if text.contains("/p/i.png")));
    assert!(matches!(&content[2], ContentPart::Audio { .. }));
    assert!(matches!(&content[3], ContentPart::Text { text } if text.contains("/p/a.mp3")));
    assert!(matches!(&content[4], ContentPart::Video { .. }));
    assert!(matches!(&content[5], ContentPart::Text { text } if text.contains("/p/v.mp4")));
}

#[test]
fn injected_text_uses_python_phrasing() {
    let mut content = vec![file_part("doc.md", "/some/where/doc.md")];
    inject_file_path_hints(&mut content);
    let injected = match &content[1] {
        ContentPart::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert_eq!(
        injected,
        "用户上传文件 `doc.md`，已经下载到 /some/where/doc.md"
    );
}
