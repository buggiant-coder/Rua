use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::handoff::TaskHandoffWriteTool;

fn context(handoff_dir: &std::path::Path) -> ToolContext {
    ToolContext {
        metadata: std::collections::HashMap::from([(
            "task_handoff_dir".to_string(),
            serde_json::Value::String(handoff_dir.to_string_lossy().to_string()),
        )]),
        ..Default::default()
    }
}

#[tokio::test]
async fn handoff_write_is_confined_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let handoff_dir = dir.path().join("handoff");
    let result = TaskHandoffWriteTool
        .call(
            serde_json::json!({
                "path": "reports/summary.md",
                "content": "# Summary\n\nVerified."
            }),
            context(&handoff_dir),
        )
        .await
        .unwrap();

    assert!(result.text_content().contains("reports/summary.md"));
    assert_eq!(
        std::fs::read_to_string(handoff_dir.join("reports/summary.md")).unwrap(),
        "# Summary\n\nVerified."
    );
    assert!(std::fs::read_dir(handoff_dir.join("reports"))
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rua-handoff-")));
}

#[tokio::test]
async fn handoff_write_rejects_absolute_and_traversal_paths() {
    let dir = tempfile::tempdir().unwrap();
    let handoff_dir = dir.path().join("handoff");
    for path in [
        "/tmp/escape.txt",
        "../escape.txt",
        "nested/../../escape.txt",
    ] {
        let error = TaskHandoffWriteTool
            .call(
                serde_json::json!({"path": path, "content": "denied"}),
                context(&handoff_dir),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("handoff path"));
    }
    assert!(!dir.path().join("escape.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn handoff_write_rejects_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let handoff_dir = dir.path().join("handoff");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&handoff_dir).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, handoff_dir.join("escape")).unwrap();

    let error = TaskHandoffWriteTool
        .call(
            serde_json::json!({
                "path": "escape/report.md",
                "content": "denied"
            }),
            context(&handoff_dir),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("symlink"));
    assert!(!outside.join("report.md").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn handoff_write_rejects_symlink_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    let handoff_dir = dir.path().join("handoff");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &handoff_dir).unwrap();

    let error = TaskHandoffWriteTool
        .call(
            serde_json::json!({
                "path": "report.md",
                "content": "denied"
            }),
            context(&handoff_dir),
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Handoff Directory must be a real directory"));
    assert!(!outside.join("report.md").exists());
}
