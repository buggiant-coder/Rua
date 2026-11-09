use std::path::Path;
use std::sync::Mutex;

use rua_skills::{
    skill_config_env_var_name, SkillEnvOverrides, SkillRequirements, SkillSource, WorkspaceManifest,
};
use serde_json::{Map, Value};
use tempfile::TempDir;

const CHANNEL: &str = "console";

static TEST_SERIALIZE: Mutex<()> = Mutex::new(());

fn make_workspace(skill_name: &str, require_envs: &[&str], config: Map<String, Value>) -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_workspace(tmp.path(), skill_name, require_envs, config);
    tmp
}

fn write_workspace(
    workspace_dir: &Path,
    skill_name: &str,
    require_envs: &[&str],
    config: Map<String, Value>,
) {
    let skill_dir = workspace_dir.join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: x\n---\nbody\n").unwrap();

    let metadata = serde_json::json!({
        "name": skill_name,
        "description": "",
        "version_text": "",
        "commit_text": "",
        "signature": "",
        "source": "customized",
        "protected": false,
        "requirements": {
            "require_bins": [],
            "require_envs": require_envs,
        },
        "updated_at": "",
        "tags": [],
        "emoji": "",
    });

    let entry = serde_json::json!({
        "enabled": true,
        "channels": ["all"],
        "source": "customized",
        "config": config,
        "metadata": metadata,
        "requirements": {
            "require_bins": [],
            "require_envs": require_envs,
        },
        "updated_at": "",
    });

    let manifest = serde_json::json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 1,
        "skills": { skill_name: entry },
    });

    std::fs::write(
        workspace_dir.join("skill.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn cfg(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    m
}

#[test]
fn skill_config_env_var_name_normalizes_like_python() {
    assert_eq!(
        skill_config_env_var_name("hello-world"),
        "RUA_SKILL_CONFIG_HELLO_WORLD",
    );
    assert_eq!(
        skill_config_env_var_name("My Skill 1"),
        "RUA_SKILL_CONFIG_MY_SKILL_1",
    );
    assert_eq!(skill_config_env_var_name(""), "RUA_SKILL_CONFIG_DEFAULT");
    assert_eq!(skill_config_env_var_name("***"), "RUA_SKILL_CONFIG_DEFAULT",);
    assert_eq!(skill_config_env_var_name("__a__"), "RUA_SKILL_CONFIG_A");
}

#[test]
fn workspace_manifest_default_is_constructible() {
    let m = WorkspaceManifest::default();
    assert!(m.skills.is_empty());
}

#[test]
fn skill_source_round_trip_via_manifest() {
    assert_eq!(SkillSource::Customized.as_str(), "customized");
    let req = SkillRequirements::default();
    assert!(req.require_envs.is_empty());
}

#[test]
fn activate_sets_env_then_drop_restores() {
    let _lock = TEST_SERIALIZE.lock().unwrap();
    let skill = "feat504_basic_skill";
    let key = "FEAT504_BASIC_KEY1";
    let blob_var = skill_config_env_var_name(skill);

    let ws = make_workspace(skill, &[key], cfg(&[(key, Value::String("v1".into()))]));

    assert!(std::env::var(key).is_err(), "pre-state should be empty");
    assert!(std::env::var(&blob_var).is_err());

    {
        let guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
        assert_eq!(std::env::var(key).unwrap(), "v1");
        let blob = std::env::var(&blob_var).unwrap();
        assert!(blob.contains(key));
        assert!(blob.contains("v1"));
        assert!(guard.keys().iter().any(|k| k == key));
        assert!(guard.keys().iter().any(|k| k == &blob_var));
    }

    assert!(std::env::var(key).is_err(), "drop must restore env");
    assert!(std::env::var(&blob_var).is_err());
}

#[test]
fn skips_keys_not_in_require_envs() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill = "feat504_skip_skill";
    let used = "FEAT504_SKIP_USED";
    let extra = "FEAT504_SKIP_EXTRA";
    let blob_var = skill_config_env_var_name(skill);

    let ws = make_workspace(
        skill,
        &[used],
        cfg(&[
            (used, Value::String("u".into())),
            (extra, Value::String("e".into())),
        ]),
    );

    let _guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
    assert_eq!(std::env::var(used).unwrap(), "u");
    assert!(
        std::env::var(extra).is_err(),
        "non-require_envs key must stay un-set",
    );
    let blob = std::env::var(&blob_var).unwrap();
    assert!(blob.contains(extra) && blob.contains("\"e\""));
}

#[test]
fn skips_null_and_empty_string_values() {
    let _lock = TEST_SERIALIZE.lock().unwrap();
    let skill = "feat504_empty_skill";
    let nul = "FEAT504_EMPTY_NULL";
    let empty = "FEAT504_EMPTY_EMPTY";
    let live = "FEAT504_EMPTY_LIVE";

    let ws = make_workspace(
        skill,
        &[nul, empty, live],
        cfg(&[
            (nul, Value::Null),
            (empty, Value::String(String::new())),
            (live, Value::String("yes".into())),
        ]),
    );

    let _guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
    assert!(std::env::var(nul).is_err());
    assert!(std::env::var(empty).is_err());
    assert_eq!(std::env::var(live).unwrap(), "yes");
}

#[test]
fn non_string_values_are_json_encoded() {
    let _lock = TEST_SERIALIZE.lock().unwrap();
    let skill = "feat504_json_skill";
    let num = "FEAT504_JSON_NUM";
    let arr = "FEAT504_JSON_ARR";

    let ws = make_workspace(
        skill,
        &[num, arr],
        cfg(&[
            (num, Value::Number(42.into())),
            (arr, serde_json::json!([1, 2, "x"])),
        ]),
    );

    let _guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
    assert_eq!(std::env::var(num).unwrap(), "42");
    assert_eq!(std::env::var(arr).unwrap(), "[1,2,\"x\"]");
}

#[test]
fn rua_skill_config_var_is_always_set_even_with_no_require_envs() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill = "feat504_blob_only_skill";
    let blob_var = skill_config_env_var_name(skill);

    let ws = make_workspace(
        skill,
        &[],
        cfg(&[("inner", Value::String("payload".into()))]),
    );

    {
        let guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
        let blob = std::env::var(&blob_var).unwrap();
        assert!(blob.contains("inner") && blob.contains("payload"));
        assert!(guard.keys().iter().any(|k| k == &blob_var));
    }
    assert!(std::env::var(&blob_var).is_err());
}

#[test]
fn same_value_share_via_refcount() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill = "feat504_share_skill";
    let key = "FEAT504_SHARE_KEY";
    let blob_var = skill_config_env_var_name(skill);

    let ws_dir = tempfile::tempdir().unwrap();
    write_workspace(
        ws_dir.path(),
        skill,
        &[key],
        cfg(&[(key, Value::String("shared".into()))]),
    );

    let g1 = SkillEnvOverrides::activate(ws_dir.path(), CHANNEL).unwrap();
    let g2 = SkillEnvOverrides::activate(ws_dir.path(), CHANNEL).unwrap();

    assert_eq!(std::env::var(key).unwrap(), "shared");
    assert!(std::env::var(&blob_var).is_ok());

    drop(g1);
    assert_eq!(
        std::env::var(key).unwrap(),
        "shared",
        "env must survive first drop while another guard is alive",
    );
    assert!(std::env::var(&blob_var).is_ok());

    drop(g2);
    assert!(
        std::env::var(key).is_err(),
        "second drop must release the env",
    );
    assert!(std::env::var(&blob_var).is_err());
}

#[test]
fn concurrent_different_value_skips_second() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill_a = "feat504_diff_a";
    let skill_b = "feat504_diff_b";
    let key = "FEAT504_DIFF_KEY";
    let blob_a = skill_config_env_var_name(skill_a);
    let blob_b = skill_config_env_var_name(skill_b);

    let ws_a = tempfile::tempdir().unwrap();
    let ws_b = tempfile::tempdir().unwrap();
    write_workspace(
        ws_a.path(),
        skill_a,
        &[key],
        cfg(&[(key, Value::String("alpha".into()))]),
    );
    write_workspace(
        ws_b.path(),
        skill_b,
        &[key],
        cfg(&[(key, Value::String("beta".into()))]),
    );

    let g_a = SkillEnvOverrides::activate(ws_a.path(), CHANNEL).unwrap();
    assert_eq!(std::env::var(key).unwrap(), "alpha");

    let g_b = SkillEnvOverrides::activate(ws_b.path(), CHANNEL).unwrap();
    assert_eq!(
        std::env::var(key).unwrap(),
        "alpha",
        "different-value second guard must NOT overwrite the first",
    );
    assert!(
        !g_b.keys().iter().any(|k| k == key),
        "second guard must not record the conflicting key in its release list",
    );

    assert!(std::env::var(&blob_a).is_ok());
    assert!(std::env::var(&blob_b).is_ok());

    drop(g_b);
    assert_eq!(
        std::env::var(key).unwrap(),
        "alpha",
        "dropping the skipped guard must not yank the env",
    );
    assert!(std::env::var(&blob_b).is_err());

    drop(g_a);
    assert!(std::env::var(key).is_err());
    assert!(std::env::var(&blob_a).is_err());
}

#[test]
fn preexisting_env_var_is_protected() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill = "feat504_preset_skill";
    let key = "FEAT504_PRESET_KEY";
    let blob_var = skill_config_env_var_name(skill);

    let ws = make_workspace(
        skill,
        &[key],
        cfg(&[(key, Value::String("override-attempt".into()))]),
    );

    unsafe {
        std::env::set_var(key, "preexisting");
    }

    {
        let guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
        assert_eq!(
            std::env::var(key).unwrap(),
            "preexisting",
            "preexisting env must be preserved",
        );
        assert!(
            !guard.keys().iter().any(|k| k == key),
            "preset key must not be in guard's release list",
        );

        assert!(std::env::var(&blob_var).is_ok());
    }

    assert_eq!(
        std::env::var(key).unwrap(),
        "preexisting",
        "drop must leave the preexisting env untouched",
    );

    unsafe {
        std::env::remove_var(key);
    }
    assert!(std::env::var(&blob_var).is_err());
}

#[test]
fn final_state_leaves_no_residual_env() {
    let _lock = TEST_SERIALIZE.lock().unwrap();

    let skill = "feat504_residual_check_skill";
    let key = "FEAT504_RESIDUAL_KEY";
    let blob_var = skill_config_env_var_name(skill);

    let ws = make_workspace(skill, &[key], cfg(&[(key, Value::String("temp".into()))]));

    {
        let _guard = SkillEnvOverrides::activate(ws.path(), CHANNEL).unwrap();
        assert!(std::env::var(key).is_ok());
        assert!(std::env::var(&blob_var).is_ok());
    }

    assert!(std::env::var(key).is_err(), "key must be cleared on drop");
    assert!(
        std::env::var(&blob_var).is_err(),
        "RUA_SKILL_CONFIG_FOO must be cleared on drop",
    );
}
