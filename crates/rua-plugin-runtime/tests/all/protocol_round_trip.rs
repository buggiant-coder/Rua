use rua_plugin_runtime::protocol::*;

#[test]
fn register_hook_params_round_trip() {
    let p = RegisterHookParams {
        plugin_id: "p1".into(),
        hook_point: "before_llm_call".into(),
        hook_name: "log-prompt".into(),
        callback: "on_before_llm".into(),
        priority: 50,
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: RegisterHookParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn register_hook_result_round_trip() {
    let r = RegisterHookResult { registered: true };
    let json = serde_json::to_string(&r).unwrap();
    let back: RegisterHookResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn register_skill_params_round_trip() {
    let p = RegisterSkillParams {
        plugin_id: "p1".into(),
        name: "my-skill".into(),
        description: "A skill".into(),
        content: "---\nname: my-skill\n---\nbody".into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: RegisterSkillParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn register_skill_result_round_trip() {
    let r = RegisterSkillResult { registered: true };
    let json = serde_json::to_string(&r).unwrap();
    let back: RegisterSkillResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn register_cli_params_round_trip() {
    let p = RegisterCliParams {
        plugin_id: "p1".into(),
        name: "my-cmd".into(),
        description: "A command".into(),
        callback: "on_cmd".into(),
        args: vec![CliArgDef {
            name: "query".into(),
            description: "Search query".into(),
            required: true,
        }],
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: RegisterCliParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn register_cli_result_round_trip() {
    let r = RegisterCliResult { registered: true };
    let json = serde_json::to_string(&r).unwrap();
    let back: RegisterCliResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn invoke_hook_params_round_trip() {
    let p = InvokeHookParams {
        hook_point: "startup".into(),
        payload: r#"{"key":"val"}"#.into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: InvokeHookParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn invoke_hook_result_round_trip() {
    let r = InvokeHookResult {
        results: vec![InvokeHookResultEntry {
            plugin_id: "p1".into(),
            hook_name: "init".into(),
            output: "ok".into(),
        }],
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: InvokeHookResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn invoke_skill_params_round_trip() {
    let p = InvokeSkillParams {
        plugin_id: "p1".into(),
        skill_name: "search".into(),
        input: "query text".into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: InvokeSkillParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn invoke_skill_result_round_trip() {
    let r = InvokeSkillResult {
        output: "found it".into(),
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: InvokeSkillResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn invoke_cli_params_round_trip() {
    let p = InvokeCliParams {
        plugin_id: "p1".into(),
        cmd_name: "deploy".into(),
        args: serde_json::json!({"env": "prod"}),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: InvokeCliParams = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn invoke_cli_result_round_trip() {
    let r = InvokeCliResult {
        output: "deployed".into(),
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: InvokeCliResult = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn capability_round_trip() {
    let caps = vec![
        PluginCapability::Tools,
        PluginCapability::Hooks,
        PluginCapability::Skills,
        PluginCapability::Cli,
    ];
    let json = serde_json::to_string(&caps).unwrap();
    assert_eq!(json, r#"["tools","hooks","skills","cli"]"#);
    let back: Vec<PluginCapability> = serde_json::from_str(&json).unwrap();
    assert_eq!(caps, back);
}

#[test]
fn jsonrpc_request_round_trip() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(serde_json::json!(1)),
        method: METHOD_REGISTER_HOOK.into(),
        params: Some(serde_json::json!({"plugin_id": "p1"})),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(req, back);
}

#[test]
fn jsonrpc_success_response_round_trip() {
    let resp = JsonRpcResponse::success(
        serde_json::json!(1),
        serde_json::json!({"registered": true}),
    );
    let json = serde_json::to_string(&resp).unwrap();
    let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(resp, back);
    assert!(back.error.is_none());
    assert!(back.result.is_some());
}

#[test]
fn jsonrpc_error_response_round_trip() {
    let resp = JsonRpcResponse::error(
        serde_json::json!(2),
        METHOD_NOT_FOUND,
        "Method not found: plugin/unknown",
    );
    let json = serde_json::to_string(&resp).unwrap();
    let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(resp, back);
    assert!(back.result.is_none());
    let err = back.error.unwrap();
    assert_eq!(err.code, METHOD_NOT_FOUND);
}

#[test]
fn unknown_method_returns_method_not_found() {
    let resp = method_not_found_response(serde_json::json!(99), "plugin/nonexistent");
    assert!(resp.result.is_none());
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, METHOD_NOT_FOUND);
    assert!(err.message.contains("plugin/nonexistent"));
}

#[test]
fn all_methods_constant_has_six_entries() {
    assert_eq!(ALL_METHODS.len(), 6);
    assert!(ALL_METHODS.contains(&METHOD_REGISTER_HOOK));
    assert!(ALL_METHODS.contains(&METHOD_REGISTER_SKILL));
    assert!(ALL_METHODS.contains(&METHOD_REGISTER_CLI));
    assert!(ALL_METHODS.contains(&METHOD_INVOKE_HOOK));
    assert!(ALL_METHODS.contains(&METHOD_INVOKE_SKILL));
    assert!(ALL_METHODS.contains(&METHOD_INVOKE_CLI));
}

#[test]
fn register_hook_params_default_priority() {
    let json =
        r#"{"plugin_id":"p1","hook_point":"startup","hook_name":"init","callback":"on_start"}"#;
    let p: RegisterHookParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.priority, 100);
}

#[test]
fn error_codes_in_expected_ranges() {
    assert_eq!(PARSE_ERROR, -32700);
    assert_eq!(INVALID_REQUEST, -32600);
    assert_eq!(METHOD_NOT_FOUND, -32601);
    assert_eq!(INVALID_PARAMS, -32602);
    assert_eq!(INTERNAL_ERROR, -32603);

    assert_eq!(PLUGIN_NOT_FOUND, -32000);
    assert_eq!(HOOK_DISPATCH_FAILED, -32001);
    assert_eq!(SKILL_NOT_FOUND, -32002);
    assert_eq!(CLI_COMMAND_NOT_FOUND, -32003);
    assert_eq!(INVOCATION_FAILED, -32004);
}
