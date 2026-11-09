use rua_channel::core::console::WebUiSseAdapter;
use rua_core::event::AskQuestion;
use rua_core::tool::ToolResult;
use rua_core::RuntimeEvent;

#[test]
fn tool_approval_request_event_emits_approval_card() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::ToolApprovalRequest {
        id: "park-abc".into(),
        name: "bash".into(),
        arguments: serde_json::json!({"command": "rm -rf /tmp/test"}),
    });
    let all_json = events.join("\n");

    assert!(
        all_json.contains("tool_approval_request"),
        "SSE should contain tool_approval_request event; got: {all_json}"
    );
    assert!(
        all_json.contains("park-abc"),
        "SSE should contain the approval request id; got: {all_json}"
    );
    assert!(
        all_json.contains("bash"),
        "SSE should contain the tool name; got: {all_json}"
    );
}

#[test]
fn context_compaction_status_stays_a_non_message_event() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::ContextCompaction {
        status: "started".to_string(),
        mode: "blocking".to_string(),
        message: None,
    });

    assert_eq!(events.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(event["type"], "context_compaction");
    assert_eq!(event["status"], "started");
    assert_eq!(event["mode"], "blocking");
    assert_ne!(event["object"], "response");
    assert_ne!(event["object"], "message");
}

#[test]
fn iteration_limit_closes_summary_but_reports_an_interrupted_response() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());
    adapter.map_event(RuntimeEvent::TextDelta {
        delta: "Unfinished work remains".into(),
        message_id: Some("summary-1".into()),
    });
    let events = adapter.map_event(RuntimeEvent::End {
        finish_reason: Some("max_iterations".into()),
    });
    let events = events
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .collect::<Vec<_>>();
    let response = events
        .iter()
        .find(|event| event["object"] == "response")
        .unwrap();
    assert_eq!(response["status"], "interrupted");
    assert_eq!(response["finish_reason"], "max_iterations");
    assert!(events
        .iter()
        .any(|event| event["object"] == "message" && event["status"] == "completed"));
}

#[test]
fn text_delta_without_canonical_message_id_is_a_protocol_error() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::TextDelta {
        delta: "orphan".into(),
        message_id: None,
    });

    assert_eq!(events.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(event["type"], "error");
    assert!(event["message"]
        .as_str()
        .is_some_and(|message| message.contains("canonical message_id")));
}

#[test]
fn tool_result_without_approval_pending_event_has_no_approval_card() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    adapter.map_event(RuntimeEvent::ToolCall {
        call: rua_core::tool::ToolCall::new("call-2", "bash", serde_json::json!({"command":"ls"})),
    });

    let result = ToolResult {
        call_id: "call-2".into(),
        name: Some("bash".into()),
        content: vec![rua_core::ContentPart::Text {
            text: "file1.txt\nfile2.txt".into(),
        }],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let events = adapter.map_event(RuntimeEvent::ToolResult { result });
    let all_json = events.join("\n");

    assert!(
        !all_json.contains("tool_approval_request"),
        "SSE should NOT contain tool_approval_request for normal tool result; got: {all_json}"
    );
}

#[test]
fn tool_result_media_message_uses_canonical_attachment_id() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());
    let result = ToolResult {
        call_id: "call-media".into(),
        name: Some("Image_Tool".into()),
        content: vec![rua_core::ContentPart::Image {
            url: "data:image/png;base64,abc".into(),
            media_type: Some("image/png".into()),
            alt: Some("chart".into()),
        }],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let events = adapter.map_event(RuntimeEvent::ToolResult { result });
    let media = events
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .find(|event| event["object"] == "message" && event["type"] == "message")
        .expect("media message");

    assert_eq!(media["id"], "msg-tr-call-media-attachment");
    let completed = events
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .find(|event| {
            event["id"] == "msg-tr-call-media-attachment" && event["status"] == "completed"
        })
        .unwrap();
    assert_eq!(
        completed["content"][0]["image_url"],
        "data:image/png;base64,abc"
    );
}

#[test]
fn ask_user_event_emits_ask_user_request_card() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::AskUser {
        request_id: "ask-abc".into(),
        questions: vec![AskQuestion {
            question: "Pick one".into(),
            options: vec!["A".into(), "B".into()],
            multi_select: false,
        }],
    });
    let values = events
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .collect::<Vec<_>>();
    let completed = values
        .iter()
        .find(|value| {
            value["object"] == "message"
                && value["type"] == "ask_user_request"
                && value["status"] == "completed"
        })
        .expect("completed ask_user_request message");

    assert_eq!(completed["content"][0]["type"], "ask_user_request");
    assert_eq!(completed["content"][0]["data"]["request_id"], "ask-abc");
    assert_eq!(completed["content"][0]["data"]["status"], "waiting");
    assert_eq!(
        completed["content"][0]["data"]["questions"][0]["question"],
        "Pick one"
    );
}

#[test]
fn tool_result_delta_streams_into_existing_tool_output_card() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    adapter.map_event(RuntimeEvent::ToolCall {
        call: rua_core::tool::ToolCall::new(
            "call-stream",
            "Long_Running_Tool",
            serde_json::json!({"runner":"codex"}),
        ),
    });

    let first = adapter.map_event(RuntimeEvent::ToolResultDelta {
        id: "call-stream".into(),
        name: Some("Long_Running_Tool".into()),
        output_delta: "hello".into(),
    });
    let first_json = first.join("\n");
    assert!(
        first_json.contains("\"type\":\"plugin_call_output\""),
        "first delta should open a tool output card; got: {first_json}"
    );
    assert!(
        first_json.contains("\"output\":\"hello\""),
        "first delta should render visible output; got: {first_json}"
    );

    let second = adapter.map_event(RuntimeEvent::ToolResultDelta {
        id: "call-stream".into(),
        name: Some("Long_Running_Tool".into()),
        output_delta: " world".into(),
    });
    let second_json = second.join("\n");
    assert!(
        second_json.contains("\"output\":\"hello world\""),
        "second delta should render accumulated output; got: {second_json}"
    );
    assert!(
        second_json.contains("\"delta\":false"),
        "tool output updates must replace content[0], because the frontend only merges that slot; got: {second_json}"
    );
}

#[test]
fn tool_call_delta_patches_late_name_and_streams_arguments() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let opened = adapter.map_event(RuntimeEvent::ToolCallDelta {
        id: "call-write".into(),
        index: 0,
        name: None,
        arguments_delta: String::new(),
    });
    let opened = opened
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .collect::<Vec<_>>();
    assert!(opened.iter().any(|event| {
        event["object"] == "content"
            && event["msg_id"] == "msg-tc-call-write"
            && event["data"]["name"] == ""
    }));

    let named = adapter.map_event(RuntimeEvent::ToolCallDelta {
        id: String::new(),
        index: 0,
        name: Some("Write".into()),
        arguments_delta: String::new(),
    });
    let named = named
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .collect::<Vec<_>>();
    assert!(named.iter().any(|event| {
        event["object"] == "content"
            && event["msg_id"] == "msg-tc-call-write"
            && event["delta"] == false
            && event["data"]["name"] == "Write"
    }));

    for fragment in [r#"{"path":"notes.txt","content":"#, "long content", r#""}"#] {
        let events = adapter.map_event(RuntimeEvent::ToolCallDelta {
            id: String::new(),
            index: 0,
            name: None,
            arguments_delta: fragment.into(),
        });
        let event = events
            .iter()
            .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
            .find(|event| event["object"] == "content" && event["delta"] == true)
            .expect("arguments delta frame");
        assert_eq!(event["data"]["name"], "Write");
        assert_eq!(event["data"]["arguments_delta"], fragment);
    }

    let result = adapter.map_event(RuntimeEvent::ToolResultDelta {
        id: "call-write".into(),
        name: None,
        output_delta: "written".into(),
    });
    let result = result
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .find(|event| event["object"] == "content" && event["type"] == "data")
        .expect("tool result frame");
    assert_eq!(result["data"]["name"], "Write");
}

#[test]
fn repeated_authoritative_tool_snapshots_replace_terminal_response_entry() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    adapter.map_event(RuntimeEvent::ToolCall {
        call: rua_core::tool::ToolCall::new(
            "call-snapshot",
            "bash",
            serde_json::json!({"command":"printf first"}),
        ),
    });
    let replacement = adapter.map_event(RuntimeEvent::ToolCall {
        call: rua_core::tool::ToolCall::new(
            "call-snapshot",
            "bash",
            serde_json::json!({"command":"printf final"}),
        ),
    });
    assert!(
        replacement.iter().all(|event| !event
            .contains("\"status\":\"in_progress\",\"error\":null,\"id\":\"msg-tc-call-snapshot\"")),
        "replacement snapshot must not reopen the existing tool widget"
    );

    let end = adapter.map_event(RuntimeEvent::End {
        finish_reason: Some("stop".into()),
    });
    let response: serde_json::Value =
        serde_json::from_str(end.last().expect("terminal response")).unwrap();
    let matching = response["output"]
        .as_array()
        .expect("response output")
        .iter()
        .filter(|message| message["id"] == "msg-tc-call-snapshot")
        .collect::<Vec<_>>();

    assert_eq!(matching.len(), 1);
    assert_eq!(
        matching[0]["content"][0]["data"]["arguments"],
        serde_json::json!("{\"command\":\"printf final\"}")
    );
}

#[test]
fn completed_tool_result_projects_text_payload_without_content_envelope() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());
    let events = adapter.map_event(RuntimeEvent::ToolResult {
        result: ToolResult::text("call-file-read", "780 | first\n781 | second"),
    });
    let payloads = events
        .iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(event).unwrap())
        .collect::<Vec<_>>();
    let completed_content = payloads
        .iter()
        .find(|event| event["object"] == "content" && event["status"] == "completed")
        .expect("completed content event");
    let completed_message = payloads
        .iter()
        .find(|event| event["object"] == "message" && event["status"] == "completed")
        .expect("completed message event");

    assert_eq!(
        completed_content["data"]["output"],
        "780 | first\n781 | second"
    );
    assert_eq!(
        completed_message["content"][0]["data"]["output"],
        "780 | first\n781 | second"
    );
}

#[test]
fn clear_command_does_not_emit_clear_history_metadata() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::TextDelta {
        delta: "History Cleared!".into(),
        message_id: Some("msg-clear".into()),
    });
    assert!(!events.is_empty());

    let end_events = adapter.map_event(RuntimeEvent::End {
        finish_reason: None,
    });

    let all_json: String = end_events.join("\n");
    assert!(
        !all_json.contains("clear_history"),
        "/clear must not emit clear_history metadata; got: {all_json}"
    );
}

#[test]
fn interjected_end_emits_interrupted_terminal_frame() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let events = adapter.map_event(RuntimeEvent::End {
        finish_reason: Some("interjected".into()),
    });
    let response: serde_json::Value = serde_json::from_str(
        events
            .last()
            .expect("end should emit a terminal response frame"),
    )
    .unwrap();

    assert_eq!(
        response.get("status").and_then(|value| value.as_str()),
        Some("interrupted")
    );
    assert_eq!(
        response
            .get("finish_reason")
            .and_then(|value| value.as_str()),
        Some("interjected")
    );
}

#[test]
fn response_frames_expose_chat_id_without_session_alias() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    let started = adapter.map_event(RuntimeEvent::TextDelta {
        delta: "hello".into(),
        message_id: Some("msg-text-1".into()),
    });
    let start_response: serde_json::Value = serde_json::from_str(
        started
            .iter()
            .find(|event| event.contains("\"object\":\"response\""))
            .expect("response start frame"),
    )
    .unwrap();
    assert_eq!(start_response["chat_id"], "chat-1");
    assert!(start_response.get("session_id").is_none());

    let completed = adapter.map_event(RuntimeEvent::End {
        finish_reason: Some("stop".into()),
    });
    let terminal: serde_json::Value =
        serde_json::from_str(completed.last().expect("terminal response")).unwrap();
    assert_eq!(terminal["chat_id"], "chat-1");
    assert!(terminal.get("session_id").is_none());
}

#[test]
fn terminal_reasoning_after_tool_output_stays_reasoning() {
    let mut adapter = WebUiSseAdapter::new("chat-1".into());

    adapter.map_event(RuntimeEvent::Start {
        session_id: rua_core::id::SessionId::new("chat-1"),
        agent_id: None,
    });
    adapter.map_event(RuntimeEvent::ThinkingDelta {
        delta: "Need a command.".into(),
        signature: None,
        message_id: Some("msg-reasoning-1".into()),
    });
    adapter.map_event(RuntimeEvent::ToolCall {
        call: rua_core::tool::ToolCall::new(
            "call-1",
            "bash",
            serde_json::json!({"command":"sleep 10 && echo 123"}),
        ),
    });
    adapter.map_event(RuntimeEvent::ToolResult {
        result: ToolResult {
            call_id: "call-1".into(),
            name: Some("bash".into()),
            content: vec![rua_core::ContentPart::Text {
                text: "123\n".into(),
            }],
            is_error: false,
            duration_ms: None,
            attachments: None,
            truncation: None,
            metadata: serde_json::Map::new(),
        },
    });
    adapter.map_event(RuntimeEvent::ThinkingDelta {
        delta: "命令已成功执行，等待 10 秒后输出了 `123`。".into(),
        signature: None,
        message_id: Some("msg-reasoning-2".into()),
    });

    let end_events = adapter.map_event(RuntimeEvent::End {
        finish_reason: Some("stop".into()),
    });
    let response: serde_json::Value = serde_json::from_str(
        end_events
            .last()
            .expect("end should emit a terminal response frame"),
    )
    .unwrap();
    let output = response
        .get("output")
        .and_then(|value| value.as_array())
        .expect("terminal response should include output");
    let last = output.last().expect("terminal response should have output");

    assert_eq!(
        last.get("type").and_then(|value| value.as_str()),
        Some("reasoning")
    );
    assert_eq!(
        last.get("content")
            .and_then(|value| value.as_array())
            .and_then(|content| content.first())
            .and_then(|content| content.get("text"))
            .and_then(|value| value.as_str()),
        Some("命令已成功执行，等待 10 秒后输出了 `123`。")
    );
}
