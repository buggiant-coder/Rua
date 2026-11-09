use rua_tool::result_store::{
    compress_output, continuation_meta, format_byte_size, format_token_count,
    format_truncation_warning, hard_cap_meta, recall_meta, truncation_kind, TruncationKind,
};

#[test]
fn compress_output_estimates_tokens_by_utf8_bytes_and_rounds() {
    let (_decorated, meta) = compress_output("abcdef", 3, "bash", Some("call-token-rounding"), 4.0);

    let meta = meta.expect("long output should produce truncation metadata");
    assert_eq!(meta.total_bytes, Some(6));
    assert_eq!(meta.est_tokens, Some(2));
    assert!(meta.recall_key.is_some());
}

#[test]
fn compress_output_counts_cjk_by_utf8_bytes() {
    let (_decorated, meta) = compress_output("你好", 3, "read", Some("call-cjk"), 2.0);

    let meta = meta.expect("long CJK output should produce truncation metadata");
    assert_eq!(meta.total_bytes, Some(6));
    assert_eq!(meta.est_tokens, Some(3));
}

#[test]
fn compress_output_invalid_divisor_falls_back_to_default() {
    let (_decorated, meta) = compress_output("abcdef", 3, "bash", Some("call-default"), 0.0);

    let meta = meta.expect("long output should produce truncation metadata");
    assert_eq!(meta.est_tokens, Some(2));
}

#[test]
fn compress_output_without_recall_key_is_hard_cap() {
    let (decorated, meta) = compress_output("line1\nline2\nline3\nline4", 12, "grep", None, 4.0);

    let meta = meta.expect("long output should produce truncation metadata");
    assert!(meta.recall_key.is_none());
    assert!(meta.retry_hint.is_some());
    assert!(decorated.contains("Output was not archived"));
}

#[test]
fn compress_output_keeps_visible_head_and_tail() {
    let output = [
        "HEAD-01",
        "HEAD-02",
        "MIDDLE-03",
        "MIDDLE-04",
        "MIDDLE-05",
        "MIDDLE-06",
        "TAIL-07",
        "TAIL-08",
    ]
    .join("\n");

    let (decorated, meta) = compress_output(&output, 36, "bash", Some("call-head-tail"), 4.0);

    assert!(meta.is_some(), "long output should be compressed");
    assert!(
        decorated.contains("HEAD-01"),
        "head content should remain visible: {decorated}"
    );
    assert!(
        decorated.contains("TAIL-08"),
        "tail content should remain visible: {decorated}"
    );
    assert!(
        decorated.contains("truncated"),
        "visible body should include the truncation marker: {decorated}"
    );
    assert!(
        !decorated.contains("MIDDLE-04"),
        "middle content should be elided: {decorated}"
    );
}

#[test]
fn byte_and_token_count_formatting_stays_stable() {
    assert_eq!(format_byte_size(45), "45B");
    assert_eq!(format_byte_size(234 * 1024), "234.0KB");
    assert_eq!(format_byte_size(8 * 1024 * 1024 + 200 * 1024), "8.2MB");

    assert_eq!(format_token_count(456), "456");
    assert_eq!(format_token_count(12_300), "12.3K");
    assert_eq!(format_token_count(2_100_000), "2.1M");
}

#[test]
fn continuation_marker_starts_with_xml_tag_and_includes_total_lines() {
    let meta = continuation_meta(
        "read",
        Some(175_199),
        Some(44 * 1024 * 1024),
        1,
        187,
        "read(file_path=\"/tmp/sla.txt\", start_line=188)",
    );
    let rendered = format_truncation_warning(&meta);

    assert!(rendered.starts_with("<truncated_output tool=\"read\">"));
    assert!(rendered.contains("175199 lines"));
    assert!(rendered.contains("Showing: lines 1-187"));
    assert!(rendered.contains("To continue, call read"));
    assert!(rendered.ends_with("</truncated_output>\n\n"));
}

#[test]
fn recall_marker_includes_recall_key_in_two_call_templates() {
    let meta = recall_meta("bash", Some(9000), 8_500_000, 2_125_000, "abc123");
    let rendered = format_truncation_warning(&meta);

    assert!(rendered.contains("recall_tool_result(recall_key=\"abc123\")"));
    assert!(rendered.contains("start_line=N, end_line=M"));
    assert!(rendered.contains("8.1MB") || rendered.contains("8.1 MB"));
}

#[test]
fn hard_cap_marker_carries_retry_hint() {
    let meta = hard_cap_meta(
        "grep",
        Some(5230),
        100,
        "Re-run with a more specific pattern.",
    );
    let rendered = format_truncation_warning(&meta);

    assert!(rendered.contains("Re-run with a more specific pattern."));
    assert!(rendered.contains("no continuation available"));
}

#[test]
fn truncation_warning_escapes_recall_key_attribute_values() {
    let meta = recall_meta("bash", None, 1, 1, "weird\"key");
    let rendered = format_truncation_warning(&meta);

    assert!(rendered.contains("recall_key=\"weird&quot;key\""));
}

#[test]
fn recall_warning_can_include_a_precise_continuation_hint() {
    let mut meta = recall_meta("bash", None, 1, 1, "k");
    meta.continuation_hint = Some("would-be-continuation".into());

    let rendered = format_truncation_warning(&meta);

    assert_eq!(truncation_kind(&meta), TruncationKind::Recall);
    assert!(rendered.contains("recall_tool_result"));
    assert!(rendered.contains("would-be-continuation"));
}

#[test]
fn truncation_kind_classifies_recovery_path() {
    let continuation = continuation_meta("read", Some(10), Some(100), 1, 3, "read(...)");
    let recall = recall_meta("bash", Some(10), 100, 25, "abc");
    let hard_cap = hard_cap_meta("grep", Some(10), 3, "Narrow the query.");

    assert_eq!(truncation_kind(&continuation), TruncationKind::Continuation);
    assert_eq!(truncation_kind(&recall), TruncationKind::Recall);
    assert_eq!(truncation_kind(&hard_cap), TruncationKind::HardCap);
}
