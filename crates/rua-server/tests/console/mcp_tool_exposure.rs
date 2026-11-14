use rua_server::services::console_chat::turn_agent_assignment_from_agent;

#[test]
fn agent_assignment_does_not_copy_tool_selection_into_a_second_authority() {
    let agent = serde_json::from_value(serde_json::json!({
        "id": "policy-agent",
        "name": "Policy Agent",
        "enabled": true,
        "tools": {
            "selection_policy": {"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[]},
            "builtin_tools": {"read":{"enabled":false}}
        },
        "mcp": {
            "clients": {"catalog":{"disabled_tools":["lookup"]}}
        }
    })).unwrap();

    let assignment = turn_agent_assignment_from_agent(&agent).unwrap();
    assert!(assignment.disabled_tools.is_empty());
}
