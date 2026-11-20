use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::ReadFileTool;
use serde_json::json;
use tokio::sync::watch;

#[tokio::test]
async fn cancelled_byte_continuation_returns_no_more_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.txt");
    std::fs::write(&path, "12345678remaining").unwrap();
    let (cancel, stop) = watch::channel(false);
    let ctx = ToolContext {
        local_device_id: Some("desktop".into()),
        working_dir: directory.path().into(),
        tool_result_store_dir: Some(directory.path().join("tool-results")),
        raw_capture_max_bytes: 8,
        cancel: Some(stop),
        ..Default::default()
    };
    let first = ReadFileTool
        .call(json!({"file_path": path}), ctx.clone())
        .await
        .unwrap();
    let next = first.metadata["continuation"].clone();
    assert_eq!(next["byte_offset"], 8);
    cancel.send(true).unwrap();

    let stopped = ReadFileTool.call(next, ctx).await.unwrap();
    assert!(stopped.is_error, "{}", stopped.text_content());
    assert!(stopped.text_content().contains("Read cancelled"));
    assert!(!stopped.text_content().contains("remainin"));
    assert_eq!(stopped.metadata["file_location"]["path"], json!(path));
    assert_eq!(stopped.metadata["file_location"]["device_id"], "desktop");
}

#[tokio::test]
async fn cancelled_directory_read_stops_before_looking_up_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let ctx = ToolContext {
        working_dir: directory.path().into(),
        tool_result_store_dir: Some(directory.path().join("tool-results")),
        cancel: Some(watch::channel(true).1),
        ..Default::default()
    };
    for path in [empty, directory.path().join("missing")] {
        let stopped = ReadFileTool
            .call(json!({"file_path": path}), ctx.clone())
            .await
            .unwrap();
        assert!(
            stopped.is_error && stopped.text_content().contains("Read cancelled"),
            "{}",
            stopped.text_content()
        );
    }
}
