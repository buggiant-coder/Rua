use std::sync::Arc;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::result_store::{compress_output, recall_key, ToolCallArchive, ToolResultStore};
use rua_tool::ToolResultBlob;

fn build_recall_tool() -> Arc<dyn Tool> {
    use rua_tool::extended::recall::RecallToolResultTool;
    Arc::new(RecallToolResultTool::new())
}

fn make_ctx(working_dir: std::path::PathBuf) -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: working_dir.clone(),
        tool_result_store_dir: Some(working_dir),
        metadata: std::collections::HashMap::new(),
        cancel: None,
        task_runtime: None,

        ..Default::default()
    }
}

fn extract_text(result: &rua_core::tool::ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|p| match p {
            agent_types::ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

async fn save_archive(
    store: &Arc<dyn ToolResultStore>,
    tool_use_id: &str,
    command: &str,
    output: String,
) -> String {
    let key = recall_key(tool_use_id);
    let blob = ToolResultBlob {
        id: key.clone(),
        tool_name: "bash".into(),
        content: output,
        mime_type: "text/plain".into(),
        created_at: chrono::Utc::now(),
        input: Some(serde_json::json!({"command": command})),
    };
    store.save(blob).await.expect("save should succeed");
    key
}

#[tokio::test]
async fn marker_carries_size_metadata_and_recall_key() {
    let original = "line\n".repeat(20_000);
    let tool_use_id = "toolu_marker_size";
    let (summary, meta) = compress_output(&original, 50 * 1024, "bash", Some(tool_use_id), 4.0);
    let meta = meta.expect("compressed output must produce metadata");

    let key = meta.recall_key.as_deref().expect("recall key");
    assert_eq!(key, recall_key(tool_use_id));
    assert_eq!(meta.total_lines, Some(20_000));
    assert_eq!(meta.total_bytes, Some(original.len() as u64));
    assert!(meta.est_tokens.is_some_and(|t| t > 0));
    assert!(
        summary.starts_with("<truncated_output tool=\"bash\">"),
        "marker must lead the body; got: {summary}"
    );
    assert!(
        summary.contains(key),
        "marker should contain the recall key; got: {summary}"
    );
    assert!(
        summary.contains("20000 lines"),
        "marker should mention total line count; got: {summary}"
    );
    assert!(
        summary.contains("recall_tool_result"),
        "marker should tell the LLM how to call the recall tool; got: {summary}"
    );
    assert!(
        summary.contains("start_line=N"),
        "marker should hint at slicing; got: {summary}"
    );
}

#[tokio::test]
async fn marker_metadata_is_none_when_below_threshold() {
    let small = "hello world\n".to_string();
    let (summary, meta) = compress_output(&small, 50 * 1024, "bash", Some("toolu_short"), 4.0);
    assert!(meta.is_none(), "no metadata when output is below threshold");
    assert_eq!(summary, small);
}

#[tokio::test]
async fn full_recall_returns_all_lines_with_header() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));

    let mut original = String::new();
    for i in 1..=500 {
        original.push_str(&format!("line {i}\n"));
    }
    let key = save_archive(
        &store,
        "toolu_full_recall",
        "yes line | head -500",
        original.clone(),
    )
    .await;

    let recall_tool = build_recall_tool();

    let args = serde_json::json!({"recall_key": key});
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    let text = extract_text(&result);

    assert!(text.contains("Tool: bash"));
    assert!(
        text.contains("yes line | head -500"),
        "should echo input; got: {text}"
    );
    assert!(
        text.contains("lines 1-500 of 500"),
        "header should report range over total; got: {text}"
    );
    assert!(text.contains("line 1\n"));
    assert!(text.contains("line 500\n"));
}

#[tokio::test]
async fn recall_accepts_agent_suffixed_key_from_model() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));
    let key = save_archive(
        &store,
        "toolu_agent_suffix",
        "cat file",
        "full archived output\n".to_string(),
    )
    .await;

    let recall_tool = build_recall_tool();
    let args = serde_json::json!({"recall_key": format!("{key}:default")});
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    let text = extract_text(&result);

    assert!(
        !result.is_error,
        "agent-suffixed key should fall back to the archived key"
    );
    assert!(text.contains("full archived output"));
}

#[tokio::test]
async fn recall_reads_only_project_scope_archive() {
    let agent_workspace = tempfile::tempdir().unwrap();
    let project_working_dir = tempfile::tempdir().unwrap();
    let project_scope = tempfile::tempdir().unwrap();
    let configured_store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(
        agent_workspace.path().join("tool_call_logs"),
    ));
    let project_store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(
        project_scope.path().join("tool_call_logs"),
    ));
    let configured_key = save_archive(
        &configured_store,
        "toolu_project_recall",
        "codegraph explore",
        "agent-scoped stale output\n".to_string(),
    )
    .await;
    let key = save_archive(
        &project_store,
        "toolu_project_recall",
        "codegraph explore",
        "project-scoped archived output\n".to_string(),
    )
    .await;
    assert_eq!(key, configured_key);

    let recall_tool = build_recall_tool();
    let mut ctx = make_ctx(project_working_dir.path().to_path_buf());
    ctx.tool_result_store_dir = Some(project_scope.path().join("tool_call_logs"));
    let result = recall_tool
        .call(serde_json::json!({"recall_key": key}), ctx)
        .await
        .unwrap();
    let text = extract_text(&result);

    assert!(
        !result.is_error,
        "Project Chat recall should use the Rua-owned Project scope"
    );
    assert!(text.contains("project-scoped archived output"));
    assert!(!text.contains("agent-scoped stale output"));
    assert!(!project_working_dir.path().join("tool_call_logs").exists());
}

#[tokio::test]
async fn slice_by_start_and_end_line() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));

    let mut original = String::new();
    for i in 1..=100 {
        original.push_str(&format!("line {i}\n"));
    }
    let key = save_archive(&store, "toolu_slice", "seq 100", original).await;

    let recall_tool = build_recall_tool();
    let args = serde_json::json!({
        "recall_key": key,
        "start_line": 10,
        "end_line": 12,
    });
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    let text = extract_text(&result);

    assert!(
        text.contains("lines 10-12 of 100"),
        "header should reflect the slice range; got: {text}"
    );
    assert!(text.contains("line 10\n"));
    assert!(text.contains("line 11\n"));
    assert!(text.contains("line 12\n"));
    assert!(
        !text.contains("line 9\n"),
        "should not include line before start; got: {text}"
    );
    assert!(
        !text.contains("line 13\n"),
        "should not include line after end; got: {text}"
    );
}

#[tokio::test]
async fn max_bytes_caps_slice_at_line_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));

    let mut original = String::new();
    for i in 1..=100 {
        original.push_str(&format!(
            "line {i:03} padded with extra characters to inflate width\n"
        ));
    }
    let key = save_archive(&store, "toolu_max_bytes", "yes", original).await;

    let recall_tool = build_recall_tool();

    let args = serde_json::json!({
        "recall_key": key,
        "max_bytes": 256,
    });
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    let text = extract_text(&result);

    assert!(
        result.truncation.is_some(),
        "legacy max_bytes must still bound"
    );
    assert!(
        text.contains("To continue from the next omitted line"),
        "should expose continuation when truncated; got: {text}"
    );
    assert!(
        !text.contains("line 100"),
        "should not include the whole archive when capped; got tail: {}",
        text.lines().rev().take(3).collect::<Vec<_>>().join(" / "),
    );
}

#[tokio::test]
async fn recall_slice_is_token_bounded_and_returns_continuation_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));
    let original = (1..=200)
        .map(|line| format!("第{line:03}行：这是用于验证多字节 token 分片的内容"))
        .collect::<Vec<_>>()
        .join("\n");
    let key = save_archive(&store, "toolu_token_slice", "cat cjk.txt", original).await;
    let recall_tool = build_recall_tool();

    let result = recall_tool
        .call(
            serde_json::json!({
                "recall_key": key,
                "max_tokens": 200,
            }),
            make_ctx(tmp.path().to_path_buf()),
        )
        .await
        .unwrap();

    assert!(rua_tool::result_store::estimate_text_tokens(&extract_text(&result), 4.0) <= 200);
    let truncation = result.truncation.as_ref().expect("continuation metadata");
    assert_eq!(truncation.recall_key.as_deref(), Some(key.as_str()));
    assert_eq!(truncation.shown_start, Some(1));
    let shown_end = truncation.shown_end.expect("shown end");
    assert!(shown_end < 200);
    let continuation = truncation
        .continuation_hint
        .as_deref()
        .expect("continuation hint");
    assert!(continuation.contains(&format!("start_line={}", shown_end + 1)));

    let next = recall_tool
        .call(
            serde_json::json!({
                "recall_key": key,
                "start_line": shown_end + 1,
                "max_tokens": 200,
            }),
            make_ctx(tmp.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(extract_text(&next).contains(&format!("第{:03}行", shown_end + 1)));
}

#[tokio::test]
async fn recall_returns_error_when_key_missing() {
    let tmp = tempfile::tempdir().unwrap();

    let recall_tool = build_recall_tool();
    let args = serde_json::json!({"recall_key": "nonexistent_key_abc"});
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    assert!(result.is_error, "missing key should yield an error result");
}

#[tokio::test]
async fn fire_and_forget_save_eventually_visible_to_recall() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolResultStore> = Arc::new(ToolCallArchive::new(tmp.path()));

    let key = recall_key("toolu_race");
    let blob = ToolResultBlob {
        id: key.clone(),
        tool_name: "bash".into(),
        content: "the full content\n".to_string(),
        mime_type: "text/plain".into(),
        created_at: chrono::Utc::now(),
        input: Some(serde_json::json!({"command": "echo hi"})),
    };

    let store_for_task = Arc::clone(&store);
    let handle = tokio::spawn(async move {
        store_for_task
            .save(blob)
            .await
            .expect("save should succeed");
    });

    handle.await.unwrap();

    let recall_tool = build_recall_tool();
    let args = serde_json::json!({"recall_key": key});
    let result = recall_tool
        .call(args, make_ctx(tmp.path().to_path_buf()))
        .await
        .unwrap();
    let text = extract_text(&result);
    assert!(text.contains("the full content"));
    assert!(text.contains("echo hi"));
}
