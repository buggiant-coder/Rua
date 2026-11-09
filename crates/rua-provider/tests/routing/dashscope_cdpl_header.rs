use rua_provider::manager::inject_dashscope_cdpl_header;

#[test]
fn coding_plan_url_gets_cdpl_header() {
    let mut headers = serde_json::Map::new();
    inject_dashscope_cdpl_header("https://coding.dashscope.aliyuncs.com/v1", &mut headers);
    assert!(
        headers.contains_key("X-DashScope-Cdpl"),
        "coding-plan URL should get X-DashScope-Cdpl header"
    );
    let value = headers["X-DashScope-Cdpl"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(value).unwrap();
    assert_eq!(parsed["agentType"], "rua");
    assert_eq!(parsed["moduleCode"], "model");
}

#[test]
fn token_plan_url_gets_cdpl_header() {
    let mut headers = serde_json::Map::new();
    inject_dashscope_cdpl_header(
        "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        &mut headers,
    );
    assert!(
        headers.contains_key("X-DashScope-Cdpl"),
        "token-plan URL should get X-DashScope-Cdpl header"
    );
    let value = headers["X-DashScope-Cdpl"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(value).unwrap();
    assert_eq!(parsed["agentType"], "rua");
}

#[test]
fn non_dashscope_url_no_cdpl_header() {
    let mut headers = serde_json::Map::new();
    inject_dashscope_cdpl_header("https://api.example.com/v1", &mut headers);
    assert!(
        !headers.contains_key("X-DashScope-Cdpl"),
        "non-dashscope URL should not get X-DashScope-Cdpl header"
    );
}
