use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_file_guard_and_skill_scanner_roundtrip() {
    let app = build_app();

    let file_before = request(
        &app.app,
        Method::GET,
        "/api/config/security/file-guard",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(file_before.status(), StatusCode::OK);
    let file_before_body = read_json(file_before).await;
    assert!(file_before_body["enabled"].is_boolean());
    assert!(file_before_body["paths"].is_array());
    assert!(file_before_body["builtin_paths"].is_array());

    let file_put = request(
        &app.app,
        Method::PUT,
        "/api/config/security/file-guard",
        Some(json!({
            "enabled": false,
            "paths": ["**/.env.local"],
            "disabled_patterns": []
        })),
        Some("default"),
    )
    .await;
    assert_eq!(file_put.status(), StatusCode::OK);
    let file_put_body = read_json(file_put).await;
    assert_eq!(file_put_body["enabled"], false);
    assert_eq!(file_put_body["paths"], json!(["**/.env.local"]));

    let scanner_before = request(
        &app.app,
        Method::GET,
        "/api/config/security/skill-scanner",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(scanner_before.status(), StatusCode::OK);
    let scanner_before_body = read_json(scanner_before).await;
    assert!(scanner_before_body["mode"].is_string());
    assert!(scanner_before_body["whitelist"].is_array());

    let scanner_put = request(
        &app.app,
        Method::PUT,
        "/api/config/security/skill-scanner",
        Some(json!({
            "mode": "off",
            "timeout": 7,
            "whitelist": []
        })),
        Some("default"),
    )
    .await;
    assert_eq!(scanner_put.status(), StatusCode::OK);
    let scanner_put_body = read_json(scanner_put).await;
    assert_eq!(scanner_put_body["mode"], "off");
    assert_eq!(scanner_put_body["timeout"], 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_no_auth_hosts_roundtrip_normalizes_and_rejects_invalid_ip_literals() {
    let app = build_app();

    let initial = request(
        &app.app,
        Method::GET,
        "/api/config/security/allow-no-auth-hosts",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(
        read_json(initial).await,
        json!({"hosts": ["127.0.0.1", "::1"]})
    );

    let update = request(
        &app.app,
        Method::PUT,
        "/api/config/security/allow-no-auth-hosts",
        Some(json!({
            "hosts": [" 127.0.0.1 ", "::1", "127.0.0.1", "  ::1  ", ""]
        })),
        Some("default"),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(
        read_json(update).await,
        json!({"hosts": ["127.0.0.1", "::1"]})
    );

    let after = request(
        &app.app,
        Method::GET,
        "/api/config/security/allow-no-auth-hosts",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(
        read_json(after).await,
        json!({"hosts": ["127.0.0.1", "::1"]})
    );

    let invalid = request(
        &app.app,
        Method::PUT,
        "/api/config/security/allow-no-auth-hosts",
        Some(json!({"hosts": ["127.0.0.1", "bad-ip-value"]})),
        Some("default"),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(invalid).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("Invalid IP address")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_allow_no_auth_hosts_aliases_global_auth_bypass_config() {
    let app = build_app();
    let agent_id = "integ_security_allow_no_auth";
    create_agent(&app.app, agent_id, "Allow no auth scoped").await;

    let scoped_path = format!("/api/agents/{agent_id}/config/security/allow-no-auth-hosts");

    let update = request(
        &app.app,
        Method::PUT,
        &scoped_path,
        Some(json!({"hosts": [" 203.0.113.10 ", "2001:db8::1", "203.0.113.10"]})),
        Some("default"),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(
        read_json(update).await,
        json!({"hosts": ["203.0.113.10", "2001:db8::1"]})
    );

    let scoped_get = request(&app.app, Method::GET, &scoped_path, None, Some("default")).await;
    assert_eq!(scoped_get.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_get).await,
        json!({"hosts": ["203.0.113.10", "2001:db8::1"]})
    );

    let root_get = request(
        &app.app,
        Method::GET,
        "/api/config/security/allow-no-auth-hosts",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(root_get.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_get).await,
        json!({"hosts": ["203.0.113.10", "2001:db8::1"]})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_tool_guard_alias_uses_header_selected_agent_without_process_wide_policy() {
    let app = build_app();
    let agent_a = "integ_security_root_tool_a";
    let agent_b = "integ_security_root_tool_b";
    create_agent(&app.app, agent_a, "Root tool guard A").await;
    create_agent(&app.app, agent_b, "Root tool guard B").await;

    let root_path = "/api/config/security/tool-guard";
    let scoped_a = format!("/api/agents/{agent_a}/config/security/tool-guard");
    let scoped_b = format!("/api/agents/{agent_b}/config/security/tool-guard");

    let default_before = request(&app.app, Method::GET, root_path, None, None).await;
    assert_eq!(default_before.status(), StatusCode::OK);
    assert_eq!(read_json(default_before).await["enabled"], true);

    let put_a = request(
        &app.app,
        Method::PUT,
        root_path,
        Some(json!({
            "enabled": true,
            "custom_rules": [],
            "disabled_rules": ["TOOL_CMD_DANGEROUS_RM"]
        })),
        Some(agent_a),
    )
    .await;
    assert_eq!(put_a.status(), StatusCode::OK);
    let put_a_body = read_json(put_a).await;
    assert!(put_a_body.get("guarded_tools").is_none());
    assert!(put_a_body.get("denied_tools").is_none());
    assert!(
        put_a_body.get("approval_mode").is_none(),
        "Tool Guard response must not expose Approval Mode"
    );

    let root_a = request(&app.app, Method::GET, root_path, None, Some(agent_a)).await;
    assert_eq!(root_a.status(), StatusCode::OK);
    assert!(read_json(root_a).await.get("denied_tools").is_none());

    let scoped_a_get = request(&app.app, Method::GET, &scoped_a, None, None).await;
    assert_eq!(scoped_a_get.status(), StatusCode::OK);
    assert!(read_json(scoped_a_get).await.get("approval_mode").is_none());

    let root_b = request(&app.app, Method::GET, root_path, None, Some(agent_b)).await;
    assert_eq!(root_b.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_b).await.get("denied_tools"),
        None,
        "root alias must not write process-wide tool guard state"
    );

    let scoped_b_get = request(&app.app, Method::GET, &scoped_b, None, None).await;
    assert_eq!(scoped_b_get.status(), StatusCode::OK);
    assert!(read_json(scoped_b_get).await.get("approval_mode").is_none());

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_guard_roundtrips_and_isolates_by_agent() {
    let app = build_app();
    let agent_a = "integ_security_tool_a";
    let agent_b = "integ_security_tool_b";
    create_agent(&app.app, agent_a, "Security tool A").await;
    create_agent(&app.app, agent_b, "Security tool B").await;

    let path_a = format!("/api/agents/{agent_a}/config/security/tool-guard");
    let path_b = format!("/api/agents/{agent_b}/config/security/tool-guard");

    let before_a = request(&app.app, Method::GET, &path_a, None, None).await;
    assert_eq!(before_a.status(), StatusCode::OK);
    let before_a_body = read_json(before_a).await;
    assert_eq!(before_a_body["enabled"], true);
    assert!(before_a_body.get("guarded_tools").is_none());
    assert!(before_a_body.get("denied_tools").is_none());

    let put_a = request(
        &app.app,
        Method::PUT,
        &path_a,
        Some(json!({
            "enabled": true,
            "custom_rules": [],
            "disabled_rules": ["TOOL_CMD_DANGEROUS_RM"]
        })),
        None,
    )
    .await;
    assert_eq!(put_a.status(), StatusCode::OK);
    let put_a_body = read_json(put_a).await;
    assert_eq!(put_a_body["enabled"], true);
    assert!(put_a_body.get("guarded_tools").is_none());
    assert!(put_a_body.get("denied_tools").is_none());
    assert!(put_a_body.get("approval_mode").is_none());

    let after_b = request(&app.app, Method::GET, &path_b, None, None).await;
    assert_eq!(after_b.status(), StatusCode::OK);
    assert!(read_json(after_b).await.get("denied_tools").is_none());

    let rules = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_a}/config/security/tool-guard/builtin-rules"),
        None,
        None,
    )
    .await;
    assert_eq!(rules.status(), StatusCode::OK);
    let rules_body = read_json(rules).await;
    let rules = rules_body.as_array().expect("builtin rules array");
    assert!(!rules.is_empty());
    assert!(rules[0]["id"].is_string());
    assert!(rules[0]["tools"].is_array());

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_approval_mode_roundtrips_and_isolates_between_agents() {
    let app = build_app();
    let agent_a = "integ_security_approval_a";
    let agent_b = "integ_security_approval_b";
    create_agent(&app.app, agent_a, "Security approval A").await;
    create_agent(&app.app, agent_b, "Security approval B").await;

    let path_a = format!("/api/agents/{agent_a}/config/security/approval-mode");
    let path_b = format!("/api/agents/{agent_b}/config/security/approval-mode");
    let direct_a = format!("/api/agents/{agent_a}/security/approval-mode");

    let default_a = request(&app.app, Method::GET, &path_a, None, None).await;
    assert_eq!(default_a.status(), StatusCode::OK);
    assert_eq!(read_json(default_a).await["approval_mode"], "hands_off");

    let legacy_a = request(
        &app.app,
        Method::PUT,
        &path_a,
        Some(json!({
            "approval_mode": "smart"
        })),
        None,
    )
    .await;
    assert_eq!(legacy_a.status(), StatusCode::OK);
    assert_eq!(read_json(legacy_a).await["approval_mode"], "hands_off");

    let put_a = request(
        &app.app,
        Method::PUT,
        &path_a,
        Some(json!({
            "approval_mode": "ask_first"
        })),
        None,
    )
    .await;
    assert_eq!(put_a.status(), StatusCode::OK);
    assert_eq!(read_json(put_a).await["approval_mode"], "ask_first");

    let b_after_a = request(&app.app, Method::GET, &path_b, None, None).await;
    assert_eq!(b_after_a.status(), StatusCode::OK);
    assert_eq!(
        read_json(b_after_a).await["approval_mode"],
        "hands_off",
        "agent A approval mode must not leak to agent B"
    );

    let put_b = request(
        &app.app,
        Method::PUT,
        &path_b,
        Some(json!({
            "approval_mode": "hands_off"
        })),
        None,
    )
    .await;
    assert_eq!(put_b.status(), StatusCode::OK);
    assert_eq!(read_json(put_b).await["approval_mode"], "hands_off");

    let a_after_b = request(&app.app, Method::GET, &direct_a, None, None).await;
    assert_eq!(a_after_b.status(), StatusCode::OK);
    assert_eq!(
        read_json(a_after_b).await["approval_mode"],
        "ask_first",
        "agent B approval mode must not overwrite agent A"
    );

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_approval_mode_is_rejected_without_mutating_config() {
    let app = build_app();
    let agent_id = "integ_security_invalid_approval_mode";
    create_agent(&app.app, agent_id, "Security invalid approval mode").await;

    let config_path = format!("/api/agents/{agent_id}/config/security/approval-mode");
    let direct_path = format!("/api/agents/{agent_id}/security/approval-mode");

    let seed = request(
        &app.app,
        Method::PUT,
        &config_path,
        Some(json!({
            "approval_mode": "ask_first"
        })),
        None,
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    assert_eq!(read_json(seed).await["approval_mode"], "ask_first");

    let invalid_config_route = request(
        &app.app,
        Method::PUT,
        &config_path,
        Some(json!({
            "approval_mode": "teleport"
        })),
        None,
    )
    .await;
    assert_eq!(
        invalid_config_route.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let invalid_config_body = read_json(invalid_config_route).await;
    assert_eq!(
        invalid_config_body["detail"][0]["loc"],
        json!(["body", "approval_mode"])
    );

    let after_config_reject = request(&app.app, Method::GET, &config_path, None, None).await;
    assert_eq!(after_config_reject.status(), StatusCode::OK);
    let after_config_body = read_json(after_config_reject).await;
    assert_eq!(after_config_body["approval_mode"], "ask_first");

    let invalid_direct_route = request(
        &app.app,
        Method::PUT,
        &direct_path,
        Some(json!({
            "approval_mode": "always"
        })),
        None,
    )
    .await;
    assert_eq!(
        invalid_direct_route.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let after_direct_reject = request(&app.app, Method::GET, &config_path, None, None).await;
    assert_eq!(after_direct_reject.status(), StatusCode::OK);
    let after_direct_body = read_json(after_direct_reject).await;
    assert_eq!(after_direct_body["approval_mode"], "ask_first");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_security_tool_guard_route_roundtrips_and_resets_builtin_template() {
    let app = build_app();
    let agent_id = "integ_security_direct_route";
    create_agent(&app.app, agent_id, "Security direct route").await;

    let direct_path = format!("/api/agents/{agent_id}/security/tool-guard");
    let config_path = format!("/api/agents/{agent_id}/config/security/tool-guard");

    let put = request(
        &app.app,
        Method::PUT,
        &direct_path,
        Some(json!({
            "enabled": true,
            "custom_rules": [],
            "disabled_rules": ["dangerous-bash"]
        })),
        None,
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let put_body = read_json(put).await;
    assert_eq!(put_body["enabled"], true);
    assert!(put_body.get("guarded_tools").is_none());
    assert!(put_body.get("denied_tools").is_none());
    assert_eq!(put_body["disabled_rules"], json!(["dangerous-bash"]));
    assert!(put_body.get("approval_mode").is_none());

    let via_config = request(&app.app, Method::GET, &config_path, None, None).await;
    assert_eq!(via_config.status(), StatusCode::OK);
    let via_config_body = read_json(via_config).await;
    assert!(via_config_body.get("guarded_tools").is_none());
    assert!(via_config_body.get("denied_tools").is_none());

    let builtin_rules = request(
        &app.app,
        Method::GET,
        &format!("{direct_path}/builtin-rules"),
        None,
        None,
    )
    .await;
    assert_eq!(builtin_rules.status(), StatusCode::OK);
    assert!(read_json(builtin_rules)
        .await
        .as_array()
        .expect("builtin rules")
        .iter()
        .any(|rule| rule["id"].is_string() && rule["tools"].is_array()));

    let reset = request(
        &app.app,
        Method::POST,
        &format!("{direct_path}/reset-to-builtin"),
        None,
        None,
    )
    .await;
    assert_eq!(reset.status(), StatusCode::OK);
    let reset_body = read_json(reset).await;
    assert_eq!(reset_body["enabled"], true);
    assert!(reset_body.get("guarded_tools").is_none());
    assert!(reset_body.get("denied_tools").is_none());
    assert_eq!(reset_body["disabled_rules"], json!([]));
    assert!(reset_body.get("approval_mode").is_none());

    let after_reset = request(&app.app, Method::GET, &config_path, None, None).await;
    assert_eq!(after_reset.status(), StatusCode::OK);
    assert!(read_json(after_reset).await.get("approval_mode").is_none());

    let approval_after_reset = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/security/approval-mode"),
        None,
        None,
    )
    .await;
    assert_eq!(approval_after_reset.status(), StatusCode::OK);
    assert_eq!(
        read_json(approval_after_reset).await["approval_mode"],
        "hands_off"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_file_guard_and_skill_scanner_routes_use_rust_config_contract() {
    let app = build_app();
    let agent_a = "integ_security_config_a";
    let agent_b = "integ_security_config_b";
    create_agent(&app.app, agent_a, "Security config scoped A").await;
    create_agent(&app.app, agent_b, "Security config scoped B").await;

    let file_path = format!("/api/agents/{agent_a}/config/security/file-guard");
    let file_put = request(
        &app.app,
        Method::PUT,
        &file_path,
        Some(json!({
            "enabled": true,
            "paths": ["**/secrets.toml"],
            "disabled_patterns": ["legacy-key"]
        })),
        None,
    )
    .await;
    assert_eq!(file_put.status(), StatusCode::OK);
    let file_body = read_json(file_put).await;
    assert_eq!(file_body["enabled"], true);
    assert_eq!(file_body["paths"], json!(["**/secrets.toml"]));

    let root_file = request(
        &app.app,
        Method::GET,
        "/api/config/security/file-guard",
        None,
        None,
    )
    .await;
    assert_eq!(root_file.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_file).await["paths"],
        json!(["**/secrets.toml"])
    );

    let file_path_b = format!("/api/agents/{agent_b}/config/security/file-guard");
    let scoped_file_b = request(&app.app, Method::GET, &file_path_b, None, None).await;
    assert_eq!(scoped_file_b.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_file_b).await["paths"],
        json!(["**/secrets.toml"]),
        "file guard is a process-wide safety policy exposed through scoped aliases"
    );

    let scanner_path = format!("/api/agents/{agent_b}/config/security/skill-scanner");
    let scanner_put = request(
        &app.app,
        Method::PUT,
        &scanner_path,
        Some(json!({
            "mode": "warn",
            "timeout": 11,
            "whitelist": []
        })),
        None,
    )
    .await;
    assert_eq!(scanner_put.status(), StatusCode::OK);
    let scanner_body = read_json(scanner_put).await;
    assert_eq!(scanner_body["mode"], "warn");
    assert_eq!(scanner_body["timeout"], 11);

    let root_scanner = request(
        &app.app,
        Method::GET,
        "/api/config/security/skill-scanner",
        None,
        None,
    )
    .await;
    assert_eq!(root_scanner.status(), StatusCode::OK);
    let root_scanner_body = read_json(root_scanner).await;
    assert_eq!(root_scanner_body["mode"], "warn");
    assert_eq!(root_scanner_body["timeout"], 11);

    delete_agent(&app.app, agent_a).await;
    delete_agent(&app.app, agent_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_scanner_whitelist_and_history_contracts_are_stable() {
    let app = build_app();
    let agent_id = "integ_security_skill_contract";
    create_agent(&app.app, agent_id, "Security skill contract").await;

    let base = format!("/api/agents/{agent_id}/config/security/skill-scanner");
    let whitelist_path = format!("{base}/whitelist");
    let add = request(
        &app.app,
        Method::POST,
        &whitelist_path,
        Some(json!({
            "skill_name": "trusted-skill",
            "content_hash": "sha256:test"
        })),
        None,
    )
    .await;
    assert_eq!(add.status(), StatusCode::CREATED);
    assert_eq!(
        read_json(add).await,
        json!({
            "skill_name": "trusted-skill",
            "content_hash": "sha256:test",
            "added_at": ""
        })
    );

    let scanner = request(&app.app, Method::GET, &base, None, None).await;
    assert_eq!(scanner.status(), StatusCode::OK);
    let scanner_body = read_json(scanner).await;
    assert!(scanner_body["whitelist"]
        .as_array()
        .expect("whitelist")
        .iter()
        .any(|entry| entry["skill_name"] == "trusted-skill"));

    let remove = request(
        &app.app,
        Method::DELETE,
        &format!("{whitelist_path}/trusted-skill"),
        None,
        None,
    )
    .await;
    assert_eq!(remove.status(), StatusCode::OK);
    assert_eq!(
        read_json(remove).await,
        json!({"success": true, "message": null})
    );

    let history = format!("{base}/blocked-history");
    let clear = request(&app.app, Method::DELETE, &history, None, None).await;
    assert_eq!(clear.status(), StatusCode::OK);
    assert_eq!(
        read_json(clear).await,
        json!({"success": true, "message": null})
    );

    let missing = request(
        &app.app,
        Method::DELETE,
        &format!("{history}/0"),
        None,
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing).await["detail"].is_string());

    delete_agent(&app.app, agent_id).await;
}
