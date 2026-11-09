use std::collections::HashMap;

use agent_types::ContentPart;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::media::{ViewImageTool, ViewVideoTool, MAX_IMAGE_SIZE};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(root: &TempDir) -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

#[tokio::test]
async fn view_image_accepts_image_urls_and_rejects_wrong_url_extensions() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = ViewImageTool::new();
    assert_eq!(tool.name(), "view_image");

    let jpg = tool
        .call(
            json!({"image_path": "https://example.com/photo.jpg?size=large"}),
            make_ctx(&root),
        )
        .await
        .expect("image url");
    assert!(!jpg.is_error, "got: {}", text(&jpg));
    assert!(jpg.content.iter().any(
        |part| matches!(part, ContentPart::Image { url, media_type: None, .. }
            if url == "https://example.com/photo.jpg?size=large")
    ));
    assert!(text(&jpg).contains("Image loaded from URL"));

    let no_extension = tool
        .call(
            json!({"image_path": "https://example.com/api/render"}),
            make_ctx(&root),
        )
        .await
        .expect("extensionless image url");
    assert!(!no_extension.is_error, "got: {}", text(&no_extension));

    let wrong_extension = tool
        .call(
            json!({"image_path": "https://example.com/doc.pdf"}),
            make_ctx(&root),
        )
        .await
        .expect("invalid image url");
    assert!(wrong_extension.is_error);
    assert!(
        text(&wrong_extension).contains("supported image format"),
        "got: {}",
        text(&wrong_extension)
    );
}

#[tokio::test]
async fn view_video_accepts_video_urls_and_rejects_wrong_url_extensions() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = ViewVideoTool::new();
    assert_eq!(tool.name(), "view_video");

    let mp4 = tool
        .call(
            json!({"video_path": "https://example.com/clip.mp4?token=1"}),
            make_ctx(&root),
        )
        .await
        .expect("video url");
    assert!(!mp4.is_error, "got: {}", text(&mp4));
    assert!(mp4.content.iter().any(
        |part| matches!(part, ContentPart::Video { url, media_type: None }
            if url == "https://example.com/clip.mp4?token=1")
    ));
    assert!(text(&mp4).contains("Video loaded from URL"));

    let no_extension = tool
        .call(
            json!({"video_path": "https://example.com/api/clip"}),
            make_ctx(&root),
        )
        .await
        .expect("extensionless video url");
    assert!(!no_extension.is_error, "got: {}", text(&no_extension));

    let wrong_extension = tool
        .call(
            json!({"video_path": "https://example.com/file.txt"}),
            make_ctx(&root),
        )
        .await
        .expect("invalid video url");
    assert!(wrong_extension.is_error);
    assert!(
        text(&wrong_extension).contains("supported video format"),
        "got: {}",
        text(&wrong_extension)
    );
}

#[tokio::test]
async fn view_image_loads_local_files_as_base64_image_parts() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("photo.png"), b"\x89PNG\r\n\x1a\n").expect("photo");

    let result = ViewImageTool::new()
        .call(json!({"image_path": "photo.png"}), make_ctx(&root))
        .await
        .expect("local image");

    assert!(!result.is_error, "got: {}", text(&result));
    assert!(result.content.iter().any(|part| {
        matches!(
            part,
            ContentPart::Image {
                url,
                media_type: Some(media_type),
                alt: Some(alt),
            } if url.starts_with("data:image/png;base64,")
                && media_type == "image/png"
                && alt == "photo.png"
        )
    }));
    assert!(text(&result).contains("Image loaded: photo.png"));
}

#[tokio::test]
async fn view_video_loads_local_files_as_base64_video_parts() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("clip.webm"), b"webm bytes").expect("clip");

    let result = ViewVideoTool::new()
        .call(json!({"video_path": "clip.webm"}), make_ctx(&root))
        .await
        .expect("local video");

    assert!(!result.is_error, "got: {}", text(&result));
    assert!(result.content.iter().any(|part| {
        matches!(
            part,
            ContentPart::Video {
                url,
                media_type: Some(media_type),
            } if url.starts_with("data:video/webm;base64,") && media_type == "video/webm"
        )
    }));
    assert!(text(&result).contains("Video loaded: clip.webm"));
}

#[tokio::test]
async fn media_tools_return_tool_errors_for_missing_unsupported_and_oversized_files() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("note.txt"), "not media").expect("note");
    std::fs::create_dir(root.path().join("folder.png")).expect("image dir");
    std::fs::create_dir(root.path().join("folder.mp4")).expect("video dir");
    let huge = root.path().join("huge.png");
    std::fs::File::create(&huge)
        .expect("create huge")
        .set_len(MAX_IMAGE_SIZE + 1)
        .expect("sparse huge image");

    let missing_image = ViewImageTool::new()
        .call(json!({"image_path": "missing.png"}), make_ctx(&root))
        .await
        .expect("missing image");
    assert!(missing_image.is_error);
    assert!(text(&missing_image).contains("File not found"));

    let unsupported_image = ViewImageTool::new()
        .call(json!({"image_path": "note.txt"}), make_ctx(&root))
        .await
        .expect("unsupported image");
    assert!(unsupported_image.is_error);
    assert!(text(&unsupported_image).contains("supported image format"));

    let directory_image = ViewImageTool::new()
        .call(json!({"image_path": "folder.png"}), make_ctx(&root))
        .await
        .expect("directory image response");
    assert!(directory_image.is_error);
    assert!(text(&directory_image).contains("is a directory"));

    let oversized_image = ViewImageTool::new()
        .call(json!({"image_path": "huge.png"}), make_ctx(&root))
        .await
        .expect("oversized image");
    assert!(oversized_image.is_error);
    assert!(text(&oversized_image).contains("Image file is too large"));

    let missing_video = ViewVideoTool::new()
        .call(json!({"video_path": "missing.mp4"}), make_ctx(&root))
        .await
        .expect("missing video");
    assert!(missing_video.is_error);
    assert!(text(&missing_video).contains("File not found"));

    let unsupported_video = ViewVideoTool::new()
        .call(json!({"video_path": "note.txt"}), make_ctx(&root))
        .await
        .expect("unsupported video");
    assert!(unsupported_video.is_error);
    assert!(text(&unsupported_video).contains("supported video format"));

    let directory_video = ViewVideoTool::new()
        .call(json!({"video_path": "folder.mp4"}), make_ctx(&root))
        .await
        .expect("directory video response");
    assert!(directory_video.is_error);
    assert!(text(&directory_video).contains("is a directory"));
}
