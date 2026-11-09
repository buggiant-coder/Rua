use std::collections::HashMap;
use std::sync::Arc;

use agent_types::ContentPart;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::media::{ViewImageTool, ViewVideoTool};
use rua_tool::extended::search::{GlobSearchTool, GrepSearchTool};
use rua_tool::{EditFileTool, MockCommandExecutor, MockResponse, ReadFileTool, ShellTool};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(root: &TempDir) -> ToolContext {
    let tool_result_store_dir = root.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).expect("tool result store");
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

fn patch_arg(patch: &str) -> serde_json::Value {
    json!({ "patch": patch })
}

#[tokio::test]
async fn file_tools_use_read_for_context_and_file_edit_for_all_mutations() {
    let root = tempfile::tempdir().expect("tempdir");
    let read = ReadFileTool::new();
    let edit = EditFileTool::new();

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Add File: notes.txt
+line1
+line2
+line3
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("create file");
    assert!(
        !result.is_error,
        "new file patches should succeed: {}",
        text(&result)
    );

    let read_result = read
        .call(
            json!({"file_path": "notes.txt", "start_line": 2, "limit": 2}),
            make_ctx(&root),
        )
        .await
        .expect("read file");
    let read_text = text(&read_result);
    assert!(read_text.contains("2 | line2"), "got: {read_text}");
    assert!(read_text.contains("3 | line3"), "got: {read_text}");
    assert!(!read_text.contains("1 | line1"), "got: {read_text}");

    let overwrite_result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: notes.txt
@@
-line1
-line2
-line3
+alpha alpha
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("update after read");
    assert!(
        !overwrite_result.is_error,
        "existing file update should be handled by edit: {}",
        text(&overwrite_result)
    );

    let missing_match = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: notes.txt
@@
-missing
+beta
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("edit missing match");
    assert!(missing_match.is_error);
    assert!(text(&missing_match).contains("Failed to find expected lines"));

    let replace = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: notes.txt
@@
-alpha alpha
+beta beta
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("replace");
    assert!(
        !replace.is_error,
        "explicit patch replacement should succeed: {}",
        text(&replace)
    );

    let append_like = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: notes.txt
@@
+tail
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("append-like patch");
    assert!(
        !append_like.is_error,
        "pure insertion patch should succeed: {}",
        text(&append_like)
    );

    let final_text = std::fs::read_to_string(root.path().join("notes.txt")).expect("read final");
    assert_eq!(final_text, "beta beta\ntail\n");
}

#[tokio::test]
async fn search_tools_preserve_qwenpaw_search_intent_with_rust_output_shape() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("a.rs"), "fn apple() {}\n").expect("a.rs");
    std::fs::write(root.path().join("b.txt"), "fn banana() {}\n").expect("b.txt");
    std::fs::create_dir(root.path().join("target")).expect("target dir");
    std::fs::write(root.path().join("target/hidden.rs"), "fn apple() {}\n").expect("hidden.rs");

    let grep_result = GrepSearchTool::new()
        .call(
            json!({
                "pattern": "fn",
                "path": ".",
                "include": ["*.rs"],
                "context_lines": 1
            }),
            make_ctx(&root),
        )
        .await
        .expect("grep");
    let grep_text = text(&grep_result);
    assert!(
        grep_text.contains("a.rs:1:> fn apple() {}"),
        "got: {grep_text}"
    );
    assert!(!grep_text.contains("b.txt"), "got: {grep_text}");
    assert!(!grep_text.contains("target/hidden.rs"), "got: {grep_text}");

    let glob_result = GlobSearchTool::new()
        .call(json!({"pattern": "**/*.rs", "path": "."}), make_ctx(&root))
        .await
        .expect("glob");
    let glob_text = text(&glob_result);
    assert!(glob_text.contains("a.rs"), "got: {glob_text}");
    assert!(!glob_text.contains("target/hidden.rs"), "got: {glob_text}");
}

#[tokio::test]
async fn bash_preserves_multiline_commands_without_installing_foreground_wait_as_deadline() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "echo first\n  echo second",
        MockResponse {
            stdout: "first\nsecond\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 7,
        },
    );
    let tool = ShellTool::new(executor.clone());

    assert_eq!(tool.name(), "bash");

    let result = tool
        .call(
            json!({
                "command": "echo first\n  echo second",
                "yield_seconds": 30
            }),
            make_ctx(&root),
        )
        .await
        .expect("bash call");
    assert!(!result.is_error, "got: {}", text(&result));
    assert!(
        text(&result).contains("first\nsecond"),
        "got: {}",
        text(&result)
    );

    let history = executor.history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].command, "echo first\n  echo second");
    assert!(
        history[0].timeout.is_zero(),
        "yield_seconds must not become an executor deadline"
    );
}

#[tokio::test]
async fn media_tools_validate_url_extensions_and_return_multimodal_parts() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("photo.png"), b"\x89PNG\r\n\x1a\n").expect("photo");
    std::fs::write(root.path().join("clip.mp4"), b"fake mp4").expect("video");

    let image_url = ViewImageTool::new()
        .call(
            json!({"image_path": "https://example.com/render"}),
            make_ctx(&root),
        )
        .await
        .expect("image url without extension");
    assert!(!image_url.is_error, "got: {}", text(&image_url));
    assert!(image_url.content.iter().any(
        |part| matches!(part, ContentPart::Image { url, .. } if url == "https://example.com/render")
    ));

    let invalid_video = ViewVideoTool::new()
        .call(
            json!({"video_path": "https://example.com/readme.txt"}),
            make_ctx(&root),
        )
        .await
        .expect("invalid video url");
    assert!(invalid_video.is_error);
    assert!(
        text(&invalid_video).contains("supported video format"),
        "got: {}",
        text(&invalid_video)
    );

    let local_image = ViewImageTool::new()
        .call(json!({"image_path": "photo.png"}), make_ctx(&root))
        .await
        .expect("local image");
    assert!(
        local_image.content.iter().any(|part| {
            matches!(
                part,
                ContentPart::Image {
                    url,
                    media_type: Some(media_type),
                    ..
                } if url.starts_with("data:image/png;base64,") && media_type == "image/png"
            )
        }),
        "expected base64 image part: {:?}",
        local_image.content
    );

    let local_video = ViewVideoTool::new()
        .call(json!({"video_path": "clip.mp4"}), make_ctx(&root))
        .await
        .expect("local video");
    assert!(
        local_video.content.iter().any(|part| {
            matches!(
                part,
                ContentPart::Video {
                    url,
                    media_type: Some(media_type),
                } if url.starts_with("data:video/mp4;base64,") && media_type == "video/mp4"
            )
        }),
        "expected base64 video part: {:?}",
        local_video.content
    );
}

#[test]
fn task_create_schema_is_tool_only_and_excludes_acp_conversations() {
    let spec = rua_tool::extended::task_tools::TaskCreateTool::spec();
    let runner_variants = spec.parameters["properties"]["runner"]["oneOf"]
        .as_array()
        .expect("task_create runner oneOf");
    assert_eq!(runner_variants.len(), 1);
    assert_eq!(
        runner_variants[0]["properties"]["type"]["enum"],
        json!(["tool"])
    );
    assert!(runner_variants.iter().all(|variant| {
        !variant.to_string().contains("acp")
            && !variant.to_string().contains("rua_agent")
            && !variant.to_string().contains("sub_agent")
    }));
}
