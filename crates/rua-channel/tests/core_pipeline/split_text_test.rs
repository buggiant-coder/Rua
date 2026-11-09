use rua_channel::core::content::split_text;

#[test]
fn short_text_unchanged() {
    let result = split_text("hello world", 100);
    assert_eq!(result, vec!["hello world"]);
}

#[test]
fn exact_boundary() {
    let text = "a".repeat(3000);
    let result = split_text(&text, 3000);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].len(), 3000);
}

#[test]
fn prefers_newline_split() {
    let first = "a".repeat(1500);
    let second = "b".repeat(2000);
    let text = format!("{first}\n{second}");
    let result = split_text(&text, 2000);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], format!("{first}\n"));
    assert_eq!(result[1], second);
}

#[test]
fn prefers_space_over_force_split() {
    let first = "a".repeat(1000);
    let second = "b".repeat(1500);
    let text = format!("{first} {second}");
    let result = split_text(&text, 1500);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], format!("{first} "));
    assert_eq!(result[1], second);
}

#[test]
fn utf8_safe_no_panic() {
    let text = format!("{}🎉{}", "a".repeat(99), "b".repeat(100));
    let result = split_text(&text, 100);
    assert!(result.len() >= 2);
    for chunk in &result {
        assert!(!chunk.is_empty());
    }
    let reassembled: String = result.into_iter().collect();
    assert_eq!(reassembled, text);
}

#[test]
fn cjk_characters_not_truncated() {
    let text = "你好世界".repeat(500);
    let result = split_text(&text, 3000);
    assert!(result.len() >= 2);
    let reassembled: String = result.into_iter().collect();
    assert_eq!(reassembled, text);
}

#[test]
fn code_fence_split_closes_and_reopens() {
    let body = "x = 1\n".repeat(60);
    let text = format!("```python\n{body}```");
    let result = split_text(&text, 120);

    assert!(
        result.len() >= 2,
        "expected the fenced block to split, got {} chunk(s)",
        result.len()
    );
    for chunk in &result {
        let ticks = chunk.matches("```").count();
        assert_eq!(ticks % 2, 0, "unbalanced code fence in chunk: {chunk:?}");
        assert!(
            chunk.contains("```python"),
            "language tag not preserved in chunk: {chunk:?}"
        );
    }
}

#[test]
fn code_fence_tilde_marker_preserved() {
    let body = "data line\n".repeat(40);
    let text = format!("~~~\n{body}~~~");
    let result = split_text(&text, 120);

    assert!(result.len() >= 2, "expected the tilde fence to split");
    for chunk in &result {
        let fences = chunk.matches("~~~").count();
        assert_eq!(fences % 2, 0, "unbalanced tilde fence in chunk: {chunk:?}");
        assert!(
            !chunk.contains("```"),
            "tilde fence must not become backticks: {chunk:?}"
        );
    }
}

#[test]
fn gfm_table_repeats_header_per_chunk() {
    let rows: String = (0..40).map(|i| format!("| {i} | val{i} |\n")).collect();
    let text = format!("| id | name |\n|----|------|\n{rows}");
    let result = split_text(&text, 80);

    assert!(
        result.len() >= 2,
        "expected the table to split across chunks, got {}",
        result.len()
    );
    for chunk in &result {
        let lines: Vec<&str> = chunk.lines().collect();
        assert!(lines.len() >= 3, "chunk is not a full table: {chunk:?}");

        assert_eq!(
            lines[0], "| id | name |",
            "missing header in chunk: {chunk:?}"
        );
        assert_eq!(
            lines[1], "|----|------|",
            "missing separator in chunk: {chunk:?}"
        );
    }
}
