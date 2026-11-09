use llm_multiplex::providers::gemini::strip_function_response_ids;
use serde_json::json;

#[test]
fn strip_removes_id_from_function_response() {
    let mut body = json!({
        "contents": [
            {
                "role": "function",
                "parts": [
                    {
                        "functionResponse": {
                            "id": "abc-123",
                            "name": "get_weather",
                            "response": {"result": "sunny"}
                        }
                    }
                ]
            }
        ]
    });

    strip_function_response_ids(&mut body);

    let fr = &body["contents"][0]["parts"][0]["functionResponse"];
    assert!(fr.get("id").is_none(), "id should be stripped");
    assert_eq!(fr["name"], "get_weather");
    assert_eq!(fr["response"]["result"], "sunny");
}

#[test]
fn strip_preserves_parts_without_id() {
    let mut body = json!({
        "contents": [
            {
                "role": "function",
                "parts": [
                    {
                        "functionResponse": {
                            "name": "search",
                            "response": {"result": "found"}
                        }
                    }
                ]
            }
        ]
    });

    let expected = body.clone();
    strip_function_response_ids(&mut body);
    assert_eq!(body, expected);
}

#[test]
fn strip_handles_multiple_messages_and_parts() {
    let mut body = json!({
        "contents": [
            {
                "role": "user",
                "parts": [{"text": "hello"}]
            },
            {
                "role": "function",
                "parts": [
                    {
                        "functionResponse": {
                            "id": "id-1",
                            "name": "tool_a",
                            "response": {"result": "a"}
                        }
                    },
                    {
                        "functionResponse": {
                            "id": "id-2",
                            "name": "tool_b",
                            "response": {"result": "b"}
                        }
                    }
                ]
            }
        ]
    });

    strip_function_response_ids(&mut body);

    let parts = body["contents"][1]["parts"].as_array().unwrap();
    for part in parts {
        let fr = &part["functionResponse"];
        assert!(fr.get("id").is_none());
        assert!(fr.get("name").is_some());
        assert!(fr.get("response").is_some());
    }

    assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
}

#[test]
fn strip_no_op_on_empty_contents() {
    let mut body = json!({"contents": []});
    strip_function_response_ids(&mut body);
    assert_eq!(body, json!({"contents": []}));
}

#[test]
fn strip_no_op_on_missing_contents() {
    let mut body = json!({"generationConfig": {}});
    strip_function_response_ids(&mut body);
    assert_eq!(body, json!({"generationConfig": {}}));
}
