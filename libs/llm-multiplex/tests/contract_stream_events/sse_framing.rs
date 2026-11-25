use llm_multiplex::transport::parse_sse_lines;

#[test]
fn openai_sse_waits_for_blank_line_across_transport_chunks() {
    let (events, remaining) = parse_sse_lines("data: {\"choices\":[\n");

    assert!(
        events.is_empty(),
        "a data-line newline is not an SSE event terminator"
    );

    let buffer = format!("{remaining}data: {{\"delta\":{{\"content\":\"hello\"}}}}]}}\n\n");
    let (events, remaining) = parse_sse_lines(&buffer);

    assert_eq!(
        events,
        vec!["{\"choices\":[\n{\"delta\":{\"content\":\"hello\"}}]}"]
    );
    assert!(remaining.is_empty());
}

#[test]
fn openai_sse_waits_for_crlf_blank_line_across_transport_chunks() {
    let event = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\r\n";
    let (events, remaining) = parse_sse_lines(event);

    assert!(events.is_empty());
    assert_eq!(remaining, event);

    let (events, remaining) = parse_sse_lines(&format!("{remaining}\r\n"));

    assert_eq!(
        events,
        vec!["{\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}"]
    );
    assert!(remaining.is_empty());
}

#[test]
fn openai_sse_accepts_data_field_without_optional_space() {
    let (events, remaining) = parse_sse_lines("data:{\"choices\":[]}\n\n");

    assert_eq!(events, vec!["{\"choices\":[]}"]);
    assert!(remaining.is_empty());
}

#[test]
fn utf8_stream_buffer_preserves_every_split_boundary() {
    use llm_multiplex::transport::Utf8StreamBuffer;

    // Each of these characters is multi-byte in UTF-8: 3-byte CJK and a 4-byte
    // emoji, which no chunk boundary may corrupt.
    let text = "架构🔧失败，遗留。";
    let bytes = text.as_bytes();

    for split in 1..bytes.len() {
        let mut buffer = Utf8StreamBuffer::new();
        let visible = buffer.push_bytes(&bytes[..split]);
        assert!(
            text.starts_with(visible),
            "split after {split} bytes surfaced text that is not a prefix of the real text"
        );
        assert!(
            !visible.contains('\u{FFFD}'),
            "split after {split} bytes surfaced a replacement character before the rest arrived"
        );
        let whole = buffer.push_bytes(&bytes[split..]);

        assert_eq!(
            whole, text,
            "split after {split} bytes corrupted the stream"
        );
        assert!(
            !whole.contains('\u{FFFD}'),
            "split after {split} bytes introduced a replacement character"
        );
    }
}

#[test]
fn utf8_stream_buffer_preserves_byte_at_a_time_streaming() {
    use llm_multiplex::transport::Utf8StreamBuffer;

    let text = "稳定失败的 remote_project_approval_and_acp…留。";
    let mut buffer = Utf8StreamBuffer::new();
    let mut seen = String::new();

    for byte in text.as_bytes() {
        seen.push_str(buffer.push_bytes(std::slice::from_ref(byte)));
        buffer.set_remaining(String::new());
    }

    assert_eq!(seen, text);
    assert!(!seen.contains('\u{FFFD}'));
}

#[test]
fn utf8_stream_buffer_keeps_partial_character_out_of_visible_text() {
    use llm_multiplex::transport::Utf8StreamBuffer;

    let mut buffer = Utf8StreamBuffer::new();
    // First two bytes of the 3-byte character 「留」, then the question mark.
    let text = "留?";

    let visible = buffer.push_bytes(&text.as_bytes()[..2]);
    assert_eq!(
        visible, "",
        "an incomplete trailing character must not surface as replacement text"
    );

    let visible = buffer.push_bytes(&text.as_bytes()[2..]);
    assert_eq!(visible, "留?");
}
