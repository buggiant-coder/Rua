use llm_multiplex::tag_parser::{extract_tool_call, TagParseMode, TagParser, TagSegment};

#[test]
fn strict_xml_basic() {
    let content = r#"<function=search><parameter=query>hello world</parameter></function>"#;
    let (name, args) = extract_tool_call(content).expect("should parse strict XML");
    assert_eq!(name, "search");
    assert_eq!(args["query"], "hello world");
}

#[test]
fn strict_xml_multiple_params() {
    let content = r#"<function=write_file><parameter=path>/tmp/test.txt</parameter><parameter=content>line1
line2</parameter></function>"#;
    let (name, args) = extract_tool_call(content).expect("should parse");
    assert_eq!(name, "write_file");
    assert_eq!(args["path"], "/tmp/test.txt");
    assert_eq!(args["content"], "line1\nline2");
}

#[test]
fn strict_xml_qwen_sample() {
    let content = r#"<function=bash>
<parameter=cmd>ls -la /tmp</parameter>
</function>"#;
    let (name, args) = extract_tool_call(content).expect("should parse Qwen output");
    assert_eq!(name, "bash");
    assert_eq!(args["cmd"], "ls -la /tmp");
}

#[test]
fn strict_xml_whitespace_around_values() {
    let content = r#"<function= read_file >
  <parameter= path >  /tmp/x  </parameter>
</function>"#;
    let (name, args) = extract_tool_call(content).expect("should handle whitespace");
    assert_eq!(name, "read_file");
    assert_eq!(args["path"], "/tmp/x");
}

#[test]
fn lenient_xml_no_closing_tags() {
    let content = r#"<function=search>
<parameter=query>rust async
<parameter=limit>10"#;
    let (name, args) = extract_tool_call(content).expect("should parse lenient XML");
    assert_eq!(name, "search");
    assert_eq!(args["query"], "rust async");
    assert_eq!(args["limit"], "10");
}

#[test]
fn lenient_xml_no_function_close() {
    let content = r#"<function=bash><parameter=cmd>echo hello</parameter>"#;
    let (name, args) = extract_tool_call(content).expect("should parse without </function>");
    assert_eq!(name, "bash");
    assert_eq!(args["cmd"], "echo hello");
}

#[test]
fn lenient_xml_mixed_closing() {
    let content = r#"<function=multi>
<parameter=a>val_a</parameter>
<parameter=b>val_b
<parameter=c>val_c"#;
    let (name, args) = extract_tool_call(content).expect("should parse mixed closing tags");
    assert_eq!(name, "multi");
    assert_eq!(args["a"], "val_a");
    assert_eq!(args["b"], "val_b");
    assert_eq!(args["c"], "val_c");
}

#[test]
fn fallback_json_first() {
    let content = r#"{"name": "bash", "arguments": {"cmd": "ls"}}"#;
    let (name, args) = extract_tool_call(content).expect("JSON should parse first");
    assert_eq!(name, "bash");
    assert_eq!(args["cmd"], "ls");
}

#[test]
fn fallback_json_string_arguments_are_decoded() {
    let content = r#"{"name":"bash","arguments":"{\"cmd\":\"ls\",\"cwd\":\"/tmp\"}"}"#;
    let (name, args) = extract_tool_call(content).expect("JSON string args should parse");
    assert_eq!(name, "bash");
    assert_eq!(args["cmd"], "ls");
    assert_eq!(args["cwd"], "/tmp");
}

#[test]
fn fallback_json_missing_name_is_rejected() {
    let content = r#"{"arguments":{"cmd":"ls"}}"#;
    assert!(extract_tool_call(content).is_none());
}

#[test]
fn fallback_strict_xml_when_json_fails() {
    let content = r#"<function=read><parameter=path>/etc/hosts</parameter></function>"#;
    let (name, args) = extract_tool_call(content).expect("strict XML fallback");
    assert_eq!(name, "read");
    assert_eq!(args["path"], "/etc/hosts");
}

#[test]
fn fallback_lenient_xml_when_strict_fails() {
    let content = r#"<function=read><parameter=path>/etc/hosts"#;
    let (name, args) = extract_tool_call(content).expect("lenient XML fallback");
    assert_eq!(name, "read");
    assert_eq!(args["path"], "/etc/hosts");
}

#[test]
fn fallback_none_on_garbage() {
    assert!(extract_tool_call("random text no tags").is_none());
}

#[test]
fn streaming_strict_xml_inside_tool_call() {
    let input = r#"Hello <tool_call><function=search><parameter=query>test</parameter></function></tool_call> done"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t == "Hello "));
    assert!(
        matches!(&segments[1], TagSegment::ToolCall { name, arguments }
            if name == "search" && arguments["query"] == "test")
    );
    assert!(matches!(&segments[2], TagSegment::Text(t) if t == " done"));
}

#[test]
fn streaming_lenient_xml_inside_tool_call() {
    let input =
        r#"<tool_call><function=bash><parameter=cmd>echo hi<parameter=dir>/tmp</tool_call>"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();

    assert_eq!(segments.len(), 1);
    assert!(
        matches!(&segments[0], TagSegment::ToolCall { name, arguments }
            if name == "bash" && arguments["cmd"] == "echo hi" && arguments["dir"] == "/tmp")
    );
}

#[test]
fn streaming_json_still_works() {
    let input = r#"<tool_call>{"name":"ls","arguments":{"path":"/"}}</tool_call>"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();

    assert_eq!(segments.len(), 1);
    assert!(
        matches!(&segments[0], TagSegment::ToolCall { name, arguments }
            if name == "ls" && arguments["path"] == "/")
    );
}

#[test]
fn streaming_multiple_json_tool_calls_preserve_surrounding_text() {
    let input = r#"text<tool_call>{"name":"a","arguments":{}}</tool_call>mid<tool_call>{"name":"b","arguments":{}}</tool_call>end"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();

    assert_eq!(segments.len(), 5);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t == "text"));
    assert!(matches!(&segments[1], TagSegment::ToolCall { name, .. } if name == "a"));
    assert!(matches!(&segments[2], TagSegment::Text(t) if t == "mid"));
    assert!(matches!(&segments[3], TagSegment::ToolCall { name, .. } if name == "b"));
    assert!(matches!(&segments[4], TagSegment::Text(t) if t == "end"));
}

#[test]
fn split_tool_call_tag_buffers_until_the_close_tag_arrives() {
    let mut parser = TagParser::new();

    parser.push("<tool");
    assert!(parser.drain().is_empty());
    assert!(parser.is_buffering_structured_segment());

    parser.push(r#"_call>{"name":"search","arguments":{"query":"ru"#);
    assert!(parser.drain().is_empty());
    assert!(parser.is_buffering_structured_segment());

    parser.push(r#"st async"}}</tool_call>after"#);
    let segments = parser.drain();

    assert_eq!(segments.len(), 2);
    assert!(
        matches!(&segments[0], TagSegment::ToolCall { name, arguments }
            if name == "search" && arguments["query"] == "rust async")
    );
    assert!(matches!(&segments[1], TagSegment::Text(t) if t == "after"));
}

#[test]
fn streaming_unclosed_think_flushes_as_text_at_finish() {
    let mut parser = TagParser::new();
    parser.push("hello<think>partial reasoning");
    let segments = parser.finish();

    assert_eq!(segments.len(), 2);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t == "hello"));
    assert!(matches!(&segments[1], TagSegment::Text(t) if t == "partial reasoning"));
}

#[test]
fn finish_parses_unclosed_tool_call_only_when_the_body_is_complete() {
    let mut parser = TagParser::new();
    parser.push(r#"<tool_call>{"name":"finish_later","arguments":{"ok":true}}"#);

    let segments = parser.finish();

    assert_eq!(segments.len(), 1);
    assert!(
        matches!(&segments[0], TagSegment::ToolCall { name, arguments }
            if name == "finish_later" && arguments["ok"] == true)
    );
}

#[test]
fn finish_keeps_complete_tool_calls_before_trailing_open_tool_call() {
    let mut parser = TagParser::new();
    parser.push(
        r#"text<tool_call>{"name":"first","arguments":{}}</tool_call><tool_call>still streaming"#,
    );

    let segments = parser.finish();

    assert_eq!(segments.len(), 2);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t == "text"));
    assert!(
        matches!(&segments[1], TagSegment::ToolCall { name, arguments }
        if name == "first" && arguments == &serde_json::json!({}))
    );
}

#[test]
fn finish_drops_incomplete_unclosed_tool_call() {
    let mut parser = TagParser::new();
    parser.push(r#"visible<tool_call>{"name":"broken","arguments":{"path":"#);

    let segments = parser.finish();

    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], TagSegment::Text(t) if t == "visible"));
}

#[test]
fn extract_thinking_removes_tags_and_preserves_plain_text() {
    let (thinking, plain) = llm_multiplex::tag_parser::extract_thinking_from_text(
        "before<think>reasoning</think>after",
    );

    assert_eq!(thinking, "reasoning");
    assert_eq!(plain, "beforeafter");
}

#[test]
fn extract_thinking_strips_outer_whitespace() {
    let (thinking, plain) =
        llm_multiplex::tag_parser::extract_thinking_from_text("<think>  spaced  </think>rest");

    assert_eq!(thinking, "spaced");
    assert_eq!(plain, "rest");
}

#[test]
fn extract_thinking_preserves_plain_text_when_no_tags_exist() {
    let (thinking, plain) = llm_multiplex::tag_parser::extract_thinking_from_text("plain text");

    assert_eq!(thinking, "");
    assert_eq!(plain, "plain text");
}

#[test]
fn extract_thinking_skips_empty_blocks_and_keeps_visible_content() {
    let (thinking, plain) =
        llm_multiplex::tag_parser::extract_thinking_from_text("<think></think>content");

    assert_eq!(thinking, "");
    assert_eq!(plain, "content");
}

#[test]
fn extract_thinking_concatenates_multiple_complete_blocks() {
    let (thinking, plain) = llm_multiplex::tag_parser::extract_thinking_from_text(
        "before<think>one</think>mid<think>\ntwo\n</think>after",
    );

    assert_eq!(thinking, "onetwo");
    assert_eq!(plain, "beforemidafter");
}

#[test]
fn streaming_strict_mode_error_on_all_fail() {
    let mut parser = TagParser::with_mode(TagParseMode::Strict);
    parser.push("<tool_call>garbage no format</tool_call>");
    let segments = parser.finish();
    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], TagSegment::Error(msg) if msg.contains("malformed")));
}

#[test]
fn empty_function_name_rejected() {
    assert!(extract_tool_call("<function=><parameter=x>1</parameter></function>").is_none());
}

#[test]
fn no_params_still_valid_strict_xml() {
    let (name, args) =
        extract_tool_call("<function=noop></function>").expect("no-param fn should parse");
    assert_eq!(name, "noop");
    assert!(args.as_object().unwrap().is_empty());
}

#[test]
fn xml_with_think_and_tool_call() {
    let input = r#"<think>reasoning</think>
<tool_call><function=search><parameter=q>rust</parameter></function></tool_call>"#;
    let mut parser = TagParser::new();
    parser.push(input);
    let segments = parser.finish();

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], TagSegment::Thinking(t) if t == "reasoning"));
    assert!(matches!(&segments[1], TagSegment::Text(t) if t == "\n"));
    assert!(matches!(&segments[2], TagSegment::ToolCall { name, .. } if name == "search"));
}
