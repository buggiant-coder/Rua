use std::sync::Arc;

use agent_types::{ToolCall, ToolResult};
use react_agent::{AgentHook, NoopHook, SkillEnvHook, ToolDecision};
use serde_json::json;

fn make_test_call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: "test_tool".to_string(),
        arguments: json!({}),
        raw_arguments: None,
        provider_call_id: None,
        metadata: serde_json::Map::new(),
    }
}

fn make_test_result(id: &str) -> ToolResult {
    ToolResult::text(id.to_string(), "ok".to_string())
}

fn write_skill_fixture(dir: &std::path::Path, skill_name: &str, env_key: &str, env_val: &str) {
    let manifest = json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 1,
        "skills": {
            skill_name: {
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "config": {
                    env_key: env_val
                },
                "metadata": {
                    "name": skill_name,
                    "source": "customized",
                    "requirements": {
                        "require_envs": [env_key]
                    }
                },
                "requirements": {
                    "require_envs": [env_key]
                }
            }
        }
    });
    std::fs::write(
        dir.join("skill.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("skills").join(skill_name)).unwrap();
}

#[tokio::test]
async fn env_var_set_during_tool_call_and_restored_after() {
    let tmp = tempfile::tempdir().unwrap();
    let env_key = format!("RUA_SKILL_ENV_HOOK_TEST_{}", uuid::Uuid::new_v4().simple());
    let env_val = "hook_test_value";
    let skill_name = "test_env_skill";

    write_skill_fixture(tmp.path(), skill_name, &env_key, env_val);

    assert!(
        std::env::var(&env_key).is_err(),
        "env var must not exist before test"
    );

    let hook = SkillEnvHook::new(Arc::new(NoopHook), tmp.path().to_path_buf(), "console");
    let call = make_test_call("call-1");

    let decision = hook.before_tool_call(&call, 0).await.unwrap();
    assert!(matches!(decision, ToolDecision::Allow));
    assert_eq!(
        std::env::var(&env_key).unwrap(),
        env_val,
        "env var must be set after before_tool_call"
    );

    let result = make_test_result("call-1");
    hook.after_tool_call(&call, &result, 10).await.unwrap();
    assert!(
        std::env::var(&env_key).is_err(),
        "env var must be restored after after_tool_call"
    );
}
