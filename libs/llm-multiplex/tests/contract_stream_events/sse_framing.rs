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
