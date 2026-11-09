use agent_types::{ContentPart, ToolResult, TruncationMeta};
use rua_core::tool::ToolContext;
use rua_tool::result_store::{hard_cap_meta, recall_key, CleanupPolicy, ToolResultOutputProcessor};
use rua_tool::{RawCommandCapture, ToolCallArchive, ToolResultStore};

#[test]
fn tool_context_without_conversation_scope_has_no_archive_directory() {
    let working_dir = tempfile::tempdir().expect("working dir");
    let ctx = ToolContext {
        working_dir: working_dir.path().to_path_buf(),
        ..Default::default()
    };

    let error = rua_tool::result_store::archive_dir_for_context(&ctx)
        .expect_err("conversation ownership is required");
    assert!(error
        .to_string()
        .contains("tool result storage requires conversation ownership"));
    assert!(!working_dir.path().join("tool_call_logs").exists());
}

#[tokio::test]
async fn processor_without_scope_cleans_raw_capture_without_archiving() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stdout_path = tmp.path().join("stdout.capture");
    let stderr_path = tmp.path().join("stderr.capture");
    std::fs::write(&stdout_path, "stdout").unwrap();
    std::fs::write(&stderr_path, "stderr").unwrap();
    let capture = RawCommandCapture {
        stdout_path: Some(stdout_path.clone()),
        stderr_path: Some(stderr_path.clone()),
        stdout_total_bytes: 6,
        stderr_total_bytes: 6,
        stdout_omitted_bytes: 0,
        stderr_omitted_bytes: 0,
        live_output_owned: false,
    };
    let ctx = ToolContext {
        working_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let mut result = ToolResult::text("call-unowned", "bounded output");
    result.metadata.insert(
        "raw_capture".to_string(),
        serde_json::to_value(capture).unwrap(),
    );

    assert!(
        !ToolResultOutputProcessor::from_tool_context(&ctx)
            .process("call-unowned", "bash", &serde_json::json!({}), &mut result,)
            .await
    );
    assert!(!stdout_path.exists());
    assert!(!stderr_path.exists());
    assert!(!tmp.path().join("tool_call_logs").exists());
}

#[tokio::test]
async fn archive_cleanup_removes_only_stale_raw_captures() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let capture_dir = tmp.path().join(".raw-capture");
    std::fs::create_dir_all(&capture_dir).expect("capture directory");
    let stale_path = capture_dir.join("stale.stdout");
    std::fs::write(&stale_path, "stale").expect("stale capture");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let fresh_path = capture_dir.join("fresh.stdout");
    std::fs::write(&fresh_path, "fresh").expect("fresh capture");

    let report = ToolCallArchive::new(tmp.path())
        .cleanup(CleanupPolicy {
            max_age: std::time::Duration::from_millis(25),
            max_total_bytes: u64::MAX,
        })
        .await
        .expect("cleanup archive");

    assert_eq!(report.files_removed, 1);
    assert_eq!(report.bytes_freed, 5);
    assert!(!stale_path.exists());
    assert!(fresh_path.exists());
}

#[tokio::test]
async fn tool_context_archives_under_rua_scope_not_execution_working_dir() {
    let working_dir = tempfile::tempdir().expect("working dir");
    let project_scope = tempfile::tempdir().expect("project scope");
    let mut ctx = ToolContext {
        working_dir: working_dir.path().to_path_buf(),
        ..Default::default()
    };
    ctx.tool_result_store_dir = Some(project_scope.path().join("tool_call_logs"));
    let raw_output = "project output\n".repeat(20_000);
    let mut result = ToolResult::text("call-project", raw_output.clone());

    ToolResultOutputProcessor::from_tool_context(&ctx)
        .process(
            "call-project",
            "Long_Output",
            &serde_json::json!({}),
            &mut result,
        )
        .await;

    let key = recall_key("call-project");
    let project_archive = ToolCallArchive::new(project_scope.path().join("tool_call_logs"))
        .read(&key)
        .await
        .expect("Project-owned archive");
    assert_eq!(project_archive.content, raw_output);
    assert!(!working_dir.path().join("tool_call_logs").exists());
}

#[tokio::test]
async fn execution_output_limit_uses_tokens_for_ascii_json_and_multibyte_text() {
    let cases = [
        ("ascii", "a".repeat(20_000)),
        (
            "json",
            serde_json::to_string(
                &(0..600)
                    .map(|index| serde_json::json!({"index": index, "ok": true}))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        ),
        ("cjk", "你好世界".repeat(2_000)),
    ];

    for (name, raw_output) in cases {
        assert!(raw_output.len() > 10_000, "{name} fixture must exceed 10KB");
        assert!(
            rua_core::token_estimate::estimate_serialized_tokens(&raw_output, 4.0) <= 10_000,
            "{name} fixture must fit the token budget"
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut result = ToolResult::text(format!("call-{name}"), raw_output.clone());
        let processor =
            ToolResultOutputProcessor::new_with_limits(tmp.path(), 10_000, 1024 * 1024, 4.0);

        let compacted = processor
            .process(
                &format!("call-{name}"),
                "Long_Output",
                &serde_json::json!({"fixture": name}),
                &mut result,
            )
            .await;

        assert!(!compacted, "{name} should not truncate at a byte threshold");
        assert_eq!(result.text_content(), raw_output);
    }
}

#[tokio::test]
async fn token_truncation_records_original_visible_and_archive_diagnostics() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = "line of output\n".repeat(20_000);
    let mut result = ToolResult::text("call-diagnostics", raw_output.clone());
    let processor =
        ToolResultOutputProcessor::new_with_limits(tmp.path(), 10_000, 1024 * 1024, 4.0);

    let compacted = processor
        .process(
            "call-diagnostics",
            "Long_Output",
            &serde_json::json!({}),
            &mut result,
        )
        .await;

    assert!(compacted);
    let diagnostics = result
        .metadata
        .get("tool_output")
        .and_then(serde_json::Value::as_object)
        .expect("tool output diagnostics");
    assert_eq!(
        diagnostics["original_bytes"].as_u64(),
        Some(raw_output.len() as u64)
    );
    assert!(diagnostics["original_tokens"]
        .as_u64()
        .is_some_and(|tokens| tokens > 10_000));
    assert!(diagnostics["visible_tokens"]
        .as_u64()
        .is_some_and(|tokens| tokens <= 10_000));
    assert_eq!(
        diagnostics["archive_key"].as_str(),
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.recall_key.as_deref())
    );
}

#[tokio::test]
async fn streaming_capture_archives_full_sidecars_while_model_sees_bounded_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stdout_path = tmp.path().join("stdout.capture");
    let stderr_path = tmp.path().join("stderr.capture");
    std::fs::write(&stdout_path, "x".repeat(5_000)).unwrap();
    std::fs::write(&stderr_path, "y".repeat(5_000)).unwrap();
    let capture = RawCommandCapture {
        stdout_path: Some(stdout_path.clone()),
        stderr_path: Some(stderr_path.clone()),
        stdout_total_bytes: 5_000,
        stderr_total_bytes: 5_000,
        stdout_omitted_bytes: 4_500,
        stderr_omitted_bytes: 4_500,
        live_output_owned: false,
    };
    let mut result = ToolResult::text(
        "call-stream",
        format!("{}\n{}", "x".repeat(500), "y".repeat(500)),
    );
    result.metadata.insert(
        "raw_capture".to_string(),
        serde_json::to_value(capture).unwrap(),
    );
    let processor = ToolResultOutputProcessor::new_with_limits(tmp.path(), 10_000, 1_024, 4.0);

    assert!(
        processor
            .process(
                "call-stream",
                "bash",
                &serde_json::json!({"command": "produce"}),
                &mut result,
            )
            .await
    );
    assert!(result.text_content().contains("recall_tool_result"));
    let blob = ToolCallArchive::new(tmp.path())
        .read(&recall_key("call-stream"))
        .await
        .expect("captured archive");
    assert!(blob.content.contains(&"x".repeat(5_000)));
    assert!(blob.content.contains(&"y".repeat(5_000)));
    assert!(!stdout_path.exists());
    assert!(!stderr_path.exists());
}

#[tokio::test]
async fn live_output_owned_capture_survives_result_archival() {
    let tmp = tempfile::tempdir().unwrap();
    let stdout_path = tmp.path().join("live-stdout.raw");
    let stderr_path = tmp.path().join("live-stderr.raw");
    std::fs::write(&stdout_path, "future stdout").unwrap();
    std::fs::write(&stderr_path, "future stderr").unwrap();
    let capture = RawCommandCapture {
        stdout_path: Some(stdout_path.clone()),
        stderr_path: Some(stderr_path.clone()),
        stdout_total_bytes: 13,
        stderr_total_bytes: 13,
        stdout_omitted_bytes: 0,
        stderr_omitted_bytes: 0,
        live_output_owned: true,
    };
    let mut result = ToolResult::text("live-call", "bounded output");
    result.metadata.insert(
        "raw_capture".to_string(),
        serde_json::to_value(capture).unwrap(),
    );
    let processor = ToolResultOutputProcessor::new_with_limits(tmp.path(), 10, 1_024, 4.0);

    processor
        .process("live-call", "bash", &serde_json::json!({}), &mut result)
        .await;

    assert!(stdout_path.exists());
    assert!(stderr_path.exists());
    let archived = ToolCallArchive::new(tmp.path())
        .read(&recall_key("live-call"))
        .await
        .unwrap();
    assert!(archived.content.contains("future stdout"));
    assert!(archived.content.contains("future stderr"));
}

#[tokio::test]
async fn one_shot_output_uses_the_same_raw_capture_boundary_after_archival() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = "z".repeat(20_000);
    let mut result = ToolResult::text("call-one-shot", raw_output.clone());
    let processor = ToolResultOutputProcessor::new_with_limits(tmp.path(), 100_000, 1_024, 4.0);

    assert!(
        processor
            .process(
                "call-one-shot",
                "Mcp_OneShot",
                &serde_json::json!({}),
                &mut result,
            )
            .await
    );
    assert!(result.text_content().len() < 2_000);
    let blob = ToolCallArchive::new(tmp.path())
        .read(&recall_key("call-one-shot"))
        .await
        .expect("one-shot archive");
    assert_eq!(blob.content, raw_output);
}

#[tokio::test]
async fn output_processor_archives_raw_output_before_compacting_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = (0..500)
        .map(|line| format!("line {line}: {}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut result = ToolResult::text("call-long", raw_output.clone());

    let processor = ToolResultOutputProcessor::new(tmp.path(), 512, 4.0);
    let compacted = processor
        .process(
            "call-long",
            "Long_Output",
            &serde_json::json!({"input": true}),
            &mut result,
        )
        .await;

    assert!(compacted);
    assert!(result.text_content().contains("<truncated_output"));
    assert!(result.text_content().contains("recall_tool_result"));
    assert_eq!(
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.recall_key.as_deref()),
        Some(recall_key("call-long").as_str())
    );

    let logger = ToolCallArchive::new(tmp.path());
    let blob = logger
        .read(&recall_key("call-long"))
        .await
        .expect("raw output");
    assert_eq!(blob.content, raw_output);

    let raw_log =
        tokio::fs::read_to_string(tmp.path().join(format!("{}.json", recall_key("call-long"))))
            .await
            .expect("raw log json");
    let raw_log: serde_json::Value = serde_json::from_str(&raw_log).expect("log json");
    assert_eq!(raw_log["tool_use_id"], "call-long");
}

#[tokio::test]
async fn output_processor_compacts_text_without_dropping_non_text_parts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = "x".repeat(5_000);
    let image = ContentPart::Image {
        url: "data:image/png;base64,abc".into(),
        media_type: Some("image/png".into()),
        alt: Some("chart".into()),
    };
    let mut result = ToolResult {
        call_id: "call-media".into(),
        name: Some("Media_Tool".into()),
        content: vec![
            ContentPart::Text {
                text: raw_output.clone(),
            },
            image.clone(),
        ],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let processor = ToolResultOutputProcessor::new(tmp.path(), 512, 4.0);
    let compacted = processor
        .process(
            "call-media",
            "Media_Tool",
            &serde_json::json!({"input": true}),
            &mut result,
        )
        .await;

    assert!(compacted);
    assert!(result.text_content().contains("recall_tool_result"));
    assert!(
        result.content.iter().any(|part| part == &image),
        "non-text content part should be preserved"
    );
}

#[tokio::test]
async fn output_processor_does_not_recompact_tool_owned_truncation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut result = ToolResult {
        call_id: "call-read".into(),
        name: Some("read".into()),
        content: vec![ContentPart::Text {
            text: "already truncated".into(),
        }],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: Some(TruncationMeta {
            tool: "read".into(),
            total_lines: Some(100),
            total_bytes: Some(10_000),
            est_tokens: Some(2_500),
            shown_start: Some(1),
            shown_end: Some(20),
            continuation_hint: Some("read(path=\"x\", offset=20)".into()),
            recall_key: None,
            retry_hint: None,
        }),
        metadata: serde_json::Map::new(),
    };

    let processor = ToolResultOutputProcessor::new(tmp.path(), 1, 4.0);
    let compacted = processor
        .process(
            "call-read",
            "read",
            &serde_json::json!({"path": "x"}),
            &mut result,
        )
        .await;

    assert!(!compacted);
    assert_eq!(result.text_content(), "already truncated");
    assert_eq!(
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.continuation_hint.as_deref()),
        Some("read(path=\"x\", offset=20)")
    );

    let logger = ToolCallArchive::new(tmp.path());
    let blob = logger
        .read(&recall_key("call-read"))
        .await
        .expect("raw output");
    assert_eq!(blob.content, "already truncated");
}

#[tokio::test]
async fn already_bounded_recall_output_is_not_wrapped_again() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let text =
        "<truncated_output tool=\"recall_tool_result\">\ncontinuation\n</truncated_output>\n\nslice";
    let mut result = ToolResult::text("recall-call", text);
    result.name = Some("recall_tool_result".to_string());
    result.truncation = Some(rua_tool::result_store::recall_meta(
        "recall_tool_result",
        Some(100),
        10_000,
        2_500,
        "archive-key",
    ));
    let processor = ToolResultOutputProcessor::new_with_limits(tmp.path(), 1, 16, 4.0);

    let compacted = processor
        .process(
            "recall-call",
            "recall_tool_result",
            &serde_json::json!({}),
            &mut result,
        )
        .await;

    assert!(!compacted);
    assert_eq!(result.text_content(), text);
    assert_eq!(
        result.text_content().matches("<truncated_output").count(),
        1
    );
}

#[tokio::test]
async fn output_processor_can_compact_tool_owned_hard_cap_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = format!(
        "{}{}",
        rua_tool::result_store::format_truncation_warning(&hard_cap_meta(
            "grep",
            None,
            200,
            "Re-run with a narrower query.",
        )),
        "match line\n".repeat(500)
    );
    let mut result = ToolResult {
        call_id: "call-hard-cap".into(),
        name: Some("grep".into()),
        content: vec![ContentPart::Text {
            text: raw_output.clone(),
        }],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: Some(hard_cap_meta(
            "grep",
            None,
            200,
            "Re-run with a narrower query.",
        )),
        metadata: serde_json::Map::new(),
    };

    let processor = ToolResultOutputProcessor::new(tmp.path(), 512, 4.0);
    let compacted = processor
        .process(
            "call-hard-cap",
            "grep",
            &serde_json::json!({"pattern": "match"}),
            &mut result,
        )
        .await;

    assert!(compacted);
    assert!(result.text_content().contains("recall_tool_result"));
    assert_eq!(
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.recall_key.as_deref()),
        Some(recall_key("call-hard-cap").as_str())
    );

    let logger = ToolCallArchive::new(tmp.path());
    let blob = logger
        .read(&recall_key("call-hard-cap"))
        .await
        .expect("raw hard-cap output");
    assert_eq!(blob.content, raw_output);
}
