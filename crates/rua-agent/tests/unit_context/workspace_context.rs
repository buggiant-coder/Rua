#[tokio::test]
async fn removed_memory_markdown_is_not_loaded_into_workspace_context() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upper = tmp.path().join("MEMORY");
    let lower = tmp.path().join("memory");
    std::fs::create_dir_all(&upper).expect("upper memory dir");
    std::fs::create_dir_all(&lower).expect("lower memory dir");
    std::fs::write(upper.join("2026-06-26.md"), "do not inject upper memory")
        .expect("write upper memory");
    std::fs::write(lower.join("2026-06-25.md"), "do not inject lower memory")
        .expect("write lower memory");

    let context = rua_agent::context::load_workspace_context(tmp.path()).await;

    assert!(
        context.memory_text.is_empty(),
        "Removed Memory Markdown must not be loaded into workspace context: {:?}",
        context.memory_text
    );
    assert!(
        !context.to_injection_text().contains("# Memories"),
        "Removed Memory Markdown must not render a Memories block"
    );
}
