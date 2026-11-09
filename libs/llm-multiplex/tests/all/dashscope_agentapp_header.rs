use llm_multiplex::dashscope::load_dashscope_headers;

#[test]
fn dashscope_headers_contain_agentapp() {
    let headers = load_dashscope_headers();
    assert!(
        headers.contains_key("x-dashscope-agentapp"),
        "headers should contain x-dashscope-agentapp"
    );

    let value = headers["x-dashscope-agentapp"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(value).unwrap();
    assert_eq!(parsed["agentType"], "rua");
    assert_eq!(parsed["moduleCode"], "model");
}
