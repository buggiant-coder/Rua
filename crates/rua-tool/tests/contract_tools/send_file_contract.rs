use std::collections::HashMap;

use agent_types::ContentPart;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::send_file::SendFileTool;
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
async fn send_file_returns_file_part_with_canonical_tool_name() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("report.pdf"), b"%PDF-1.4\n").expect("seed pdf");

    let tool = SendFileTool::new();
    assert_eq!(tool.name(), "send_file");

    let result = tool
        .call(json!({"file_path": "report.pdf"}), make_ctx(&root))
        .await
        .expect("send file result");

    assert!(!result.is_error, "got: {}", text(&result));
    assert_eq!(result.call_id, "send_file");
    assert!(
        result.content.iter().any(|part| matches!(
            part,
            ContentPart::File {
                url,
                name,
                media_type: Some(media_type),
                size: Some(size),
            } if url.starts_with("file://")
                && name == "report.pdf"
                && media_type == "application/pdf"
                && *size == 9
        )),
        "expected file content part with metadata: {:?}",
        result.content
    );
    assert!(text(&result).contains("File sent successfully"));
}

#[tokio::test]
async fn send_file_reports_missing_directory_and_unknown_mime_boundaries() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(root.path().join("folder")).expect("folder");
    std::fs::write(root.path().join("artifact.unknown"), "payload").expect("unknown");

    let tool = SendFileTool::new();

    let missing = tool
        .call(json!({"file_path": "missing.txt"}), make_ctx(&root))
        .await
        .expect("missing response");
    assert!(missing.is_error);
    assert!(text(&missing).contains("File not found"));

    let directory = tool
        .call(json!({"file_path": "folder"}), make_ctx(&root))
        .await
        .expect("directory response");
    assert!(directory.is_error);
    assert!(text(&directory).contains("is a directory"));

    let unknown = tool
        .call(json!({"file_path": "artifact.unknown"}), make_ctx(&root))
        .await
        .expect("unknown extension response");
    assert!(!unknown.is_error, "got: {}", text(&unknown));
    assert!(
        unknown.content.iter().any(|part| matches!(
            part,
            ContentPart::File {
                name,
                media_type: Some(media_type),
                ..
            } if name == "artifact.unknown" && media_type == "application/octet-stream"
        )),
        "unknown extensions should still be deliverable as octet-stream: {:?}",
        unknown.content
    );
}
