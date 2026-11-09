use rua_server::services::console_chat::extract_disabled_mcp_tools;

#[test]
fn disabled_mcp_tools_are_projected_to_agent_tool_names() {
    let mcp = Some(serde_json::json!({
        "clients": {
            "github": {
                "disabled_tools": ["search", "create_issue"]
            },
            "filesystem": {
                "disabled_tools": ["read_doc"]
            },
            "empty": {
                "disabled_tools": []
            }
        }
    }));

    let mut disabled = extract_disabled_mcp_tools(&mcp);
    disabled.sort();

    assert_eq!(
        disabled,
        vec![
            "mcp__filesystem__read_doc".to_string(),
            "mcp__github__create_issue".to_string(),
            "mcp__github__search".to_string(),
        ]
    );
}
