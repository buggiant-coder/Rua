use agent_types::{ChatRequest, ModelDialogueItem, Role};
use llm_multiplex::debug_capture::{
    self, record_logical_request, record_named_json, record_provider_request,
    record_provider_response, ActiveCapture, CHAT_ADDRESS_METADATA_KEY, MODEL_CALL_ID_METADATA_KEY,
};
use rua_core::ChatAddress;
use serde_json::{json, Map, Value};

fn test_capture() -> (std::path::PathBuf, ChatAddress, Map<String, Value>) {
    let suffix = uuid::Uuid::new_v4().to_string();
    let capture_id = format!("capture-{suffix}");
    let root = std::env::temp_dir()
        .join(format!("rua-llm-debug-capture-test-{suffix}"))
        .join("llm-captures")
        .join(&capture_id);
    let address = ChatAddress::project(format!("project-{suffix}"), format!("chat-{suffix}"));
    debug_capture::register_capture(
        address.clone(),
        ActiveCapture {
            capture_id,
            capture_path: root.clone(),
        },
    );
    let metadata = Map::from_iter([
        (
            CHAT_ADDRESS_METADATA_KEY.to_string(),
            serde_json::to_value(&address).expect("chat address serializes"),
        ),
        (MODEL_CALL_ID_METADATA_KEY.to_string(), json!("call-1")),
    ]);
    (root, address, metadata)
}

#[test]
fn writes_artifacts_from_model_call_identity_without_capture_configuration() {
    let (root, address, metadata) = test_capture();

    assert!(record_named_json(
        &metadata,
        "logical-request.json",
        &json!({"messages": ["hello"]})
    ));
    for (provider, endpoint, model) in [
        ("openai", "/chat/completions", "gpt-test"),
        ("anthropic", "/v1/messages", "claude-test"),
        ("openai", "/chat/completions", "gpt-test"),
    ] {
        assert!(record_provider_request(
            &metadata,
            provider,
            endpoint,
            &json!({"model": model})
        ));
    }
    assert!(record_provider_response(
        &metadata,
        "openai",
        "/responses",
        &json!({"type": "response.completed", "response": {"id": "resp-1"}})
    ));

    debug_capture::flush_capture(&root, std::time::Duration::from_secs(5)).unwrap();

    let call_dir = root.join("calls/call-1");
    let provider_files = std::fs::read_dir(&call_dir)
        .expect("call dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("provider-request-") && name.ends_with(".json")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(provider_files.len(), 3);
    assert!(provider_files.iter().all(|path| {
        !path
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .contains("-000")
    }));
    let providers = provider_files
        .iter()
        .map(|path| {
            serde_json::from_slice::<Value>(&std::fs::read(path).expect("provider artifact"))
                .expect("provider JSON")
        })
        .collect::<Vec<_>>();
    assert!(providers.iter().any(|value| value["provider"] == "openai"));
    assert!(providers
        .iter()
        .any(|value| value["provider"] == "anthropic"));
    assert!(providers.iter().all(|value| value.get("headers").is_none()));
    let response_files = std::fs::read_dir(&call_dir)
        .expect("call dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("provider-response-") && name.ends_with(".json")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(response_files.len(), 1);
    let response: Value =
        serde_json::from_slice(&std::fs::read(&response_files[0]).expect("response artifact"))
            .expect("response JSON");
    assert_eq!(response["provider"], "openai");
    assert_eq!(response["body"]["response"]["id"], "resp-1");

    debug_capture::unregister_capture(&address);
    debug_capture::flush_capture(&root, std::time::Duration::from_secs(5)).unwrap();
    std::fs::remove_dir_all(
        root.parent()
            .expect("capture parent")
            .parent()
            .expect("test root"),
    )
    .expect("cleanup");
}

#[test]
fn logical_request_records_only_model_call_identity_and_projection_status() {
    let (root, address, metadata) = test_capture();
    let mut request = ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "summarize")]);
    request.metadata = metadata;

    record_logical_request(&request);
    debug_capture::flush_capture(&root, std::time::Duration::from_secs(5)).unwrap();

    let call_dir = root.join("calls/call-1");
    let call: Value =
        serde_json::from_slice(&std::fs::read(call_dir.join("call.json")).expect("call artifact"))
            .expect("call JSON");
    assert_eq!(call["model_call_id"], "call-1");
    assert_eq!(
        call["chat_address"],
        serde_json::to_value(&address).unwrap()
    );
    assert!(call.get("purpose").is_none());
    assert!(call.get("parent_model_call_id").is_none());

    let projection: Value = serde_json::from_slice(
        &std::fs::read(call_dir.join("projection.json")).expect("projection placeholder"),
    )
    .expect("projection JSON");
    assert_eq!(projection["status"], "unavailable");
    assert_eq!(
        projection["reason"],
        "context_projection_evidence_not_recorded"
    );

    debug_capture::unregister_capture(&address);
    debug_capture::flush_capture(&root, std::time::Duration::from_secs(5)).unwrap();
    std::fs::remove_dir_all(
        root.parent()
            .expect("capture parent")
            .parent()
            .expect("test root"),
    )
    .expect("cleanup");
}

#[test]
fn unregistered_chat_address_is_a_noop() {
    let address = ChatAddress::agent("agent-1", "chat-1");
    let metadata = Map::from_iter([
        (
            CHAT_ADDRESS_METADATA_KEY.to_string(),
            serde_json::to_value(&address).unwrap(),
        ),
        (MODEL_CALL_ID_METADATA_KEY.to_string(), json!("call-1")),
    ]);
    assert!(!record_named_json(
        &metadata,
        "logical-request.json",
        &json!({"ignored": true})
    ));
}
