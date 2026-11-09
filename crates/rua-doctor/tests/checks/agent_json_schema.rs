use rua_config::dirs::Paths;
use rua_doctor::agent_json::validate_agent_json;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn agent_json_schema_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-ajson-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    assert!(
        results.iter().any(|r| r.name == "agent_json_schema"),
        "agent_json_schema check must appear in run_all_checks"
    );
}

#[test]
fn valid_agent_json_pass() {
    let tmp = std::env::temp_dir().join("rua-doctor-ajson-valid");
    let _ = std::fs::create_dir_all(&tmp);

    let ws = tmp.join("ws-default");
    let _ = std::fs::create_dir_all(&ws);

    let agent_json = serde_json::json!({
        "id": "default",
        "name": "Default Agent",
        "description": "test agent",
        "workspace_dir": ws.to_str().unwrap(),
        "running": {},
        "llm_routing": {}
    });
    std::fs::write(
        ws.join("agent.json"),
        serde_json::to_string_pretty(&agent_json).unwrap(),
    )
    .unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": "default",
            "profiles": {
                "default": {
                    "id": "default",
                    "workspace_dir": ws.to_str().unwrap(),
                    "enabled": true
                }
            }
        },
        "channels": {}
    });
    std::fs::write(
        tmp.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let r = results
        .iter()
        .find(|r| r.name == "agent_json_schema")
        .unwrap();
    assert_eq!(r.status, CheckStatus::Pass, "valid agent.json → Pass");
    assert!(
        r.message.contains("valid"),
        "message should mention valid: {}",
        r.message
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn broken_agent_json_name_missing_fail() {
    let tmp = std::env::temp_dir().join("rua-doctor-ajson-noname");
    let _ = std::fs::create_dir_all(&tmp);

    let ws = tmp.join("ws-broken");
    let _ = std::fs::create_dir_all(&ws);

    let agent_json = serde_json::json!({
        "id": "broken",
        "name": "",
        "workspace_dir": ws.to_str().unwrap(),
        "running": {},
        "llm_routing": {}
    });
    std::fs::write(
        ws.join("agent.json"),
        serde_json::to_string_pretty(&agent_json).unwrap(),
    )
    .unwrap();

    let config = serde_json::json!({
        "agents": {
            "active_agent": "broken",
            "profiles": {
                "broken": {
                    "id": "broken",
                    "workspace_dir": ws.to_str().unwrap(),
                    "enabled": true
                }
            }
        },
        "channels": {}
    });
    std::fs::write(
        tmp.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let r = results
        .iter()
        .find(|r| r.name == "agent_json_schema")
        .unwrap();
    assert_eq!(
        r.status,
        CheckStatus::Fail,
        "empty name → Fail: {}",
        r.message
    );
    assert!(
        r.message.contains("error"),
        "message should mention error: {}",
        r.message
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unknown_provider_warn() {
    let tmp = std::env::temp_dir().join("rua-doctor-ajson-unknownprov");
    let _ = std::fs::create_dir_all(&tmp);

    let diag = validate_agent_json(
        "test",
        &std::path::PathBuf::from("/tmp/no-such-file.json"),
        &[],
    );
    assert!(!diag.deserialize_ok);

    let agent_file = tmp.join("test-agent.json");
    let agent_json = serde_json::json!({
        "id": "test",
        "name": "Test",
        "workspace_dir": "/tmp",
        "default_model": {
            "provider_id": "ghost-provider",
            "model": "gpt-4"
        },
        "running": {},
        "llm_routing": {}
    });
    std::fs::write(
        &agent_file,
        serde_json::to_string_pretty(&agent_json).unwrap(),
    )
    .unwrap();

    let diag = validate_agent_json(
        "test",
        &agent_file,
        &["openai".to_string(), "azure".to_string()],
    );
    assert!(diag.deserialize_ok);
    assert!(
        diag.provider_warnings
            .iter()
            .any(|w| w.contains("ghost-provider")),
        "should warn about unknown provider"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn no_agent_json_skip() {
    let tmp = std::env::temp_dir().join("rua-doctor-ajson-nojson");
    let _ = std::fs::create_dir_all(&tmp);

    let ws = tmp.join("ws-empty");
    let _ = std::fs::create_dir_all(&ws);

    let config = serde_json::json!({
        "agents": {
            "active_agent": "default",
            "profiles": {
                "default": {
                    "id": "default",
                    "workspace_dir": ws.to_str().unwrap(),
                    "enabled": true
                }
            }
        },
        "channels": {}
    });
    std::fs::write(
        tmp.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let r = results
        .iter()
        .find(|r| r.name == "agent_json_schema")
        .unwrap();
    assert_eq!(
        r.status,
        CheckStatus::Pass,
        "no agent.json present → Pass (0 files): {}",
        r.message
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
