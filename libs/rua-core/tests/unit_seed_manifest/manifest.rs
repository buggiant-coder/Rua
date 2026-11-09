use std::fs;

use rua_core::seed_manifest::{
    is_entity_untouched_seed, is_file_untouched_seed, manifest_path, read_manifest,
    record_seed_entry,
};
use tempfile::TempDir;

#[test]
fn record_seed_entry_persists_entry_and_file_hashes() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    let agent_json = data_dir.join("workspaces/rua_qa/agent.json");
    fs::create_dir_all(agent_json.parent().unwrap()).unwrap();
    fs::write(&agent_json, br#"{"id":"rua_qa"}"#).unwrap();

    record_seed_entry(
        data_dir,
        "agent:rua_qa",
        "agent_seed_catalog",
        &["workspaces/rua_qa/agent.json"],
    )
    .expect("record seed");

    let manifest = read_manifest(data_dir).unwrap().expect("manifest exists");
    assert_eq!(manifest.version, 1);
    assert!(manifest.entries.contains_key("agent:rua_qa"));
    assert!(manifest.files.contains_key("workspaces/rua_qa/agent.json"));
    assert_eq!(
        manifest.entries["agent:rua_qa"].paths,
        vec!["workspaces/rua_qa/agent.json"]
    );
    assert!(manifest_path(data_dir).is_file());
}

#[test]
fn record_seed_entry_preserves_existing_entries_when_adding_another() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    write_file(data_dir, "prompt_library/SOUL.md", "soul");
    write_file(data_dir, "skill_pool/guidance/SKILL.md", "skill");

    record_seed_entry(
        data_dir,
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/SOUL.md"],
    )
    .unwrap();
    record_seed_entry(
        data_dir,
        "skill_pool:guidance",
        "builtin_skill_pool",
        &["skill_pool/guidance/SKILL.md"],
    )
    .unwrap();

    let manifest = read_manifest(data_dir).unwrap().unwrap();
    assert!(manifest.entries.contains_key("prompt_library:builtin"));
    assert!(manifest.entries.contains_key("skill_pool:guidance"));
    assert!(manifest.files.contains_key("prompt_library/SOUL.md"));
    assert!(manifest.files.contains_key("skill_pool/guidance/SKILL.md"));
}

#[test]
fn record_seed_entry_merges_paths_for_existing_entry() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    write_file(data_dir, "prompt_library/SOUL.md", "soul");
    write_file(data_dir, "prompt_library/QA-Agent.md", "qa");

    record_seed_entry(
        data_dir,
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/SOUL.md"],
    )
    .unwrap();
    record_seed_entry(
        data_dir,
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/QA-Agent.md"],
    )
    .unwrap();

    let manifest = read_manifest(data_dir).unwrap().unwrap();
    assert_eq!(
        manifest.entries["prompt_library:builtin"].paths,
        vec!["prompt_library/QA-Agent.md", "prompt_library/SOUL.md"]
    );
    assert!(manifest.files.contains_key("prompt_library/SOUL.md"));
    assert!(manifest.files.contains_key("prompt_library/QA-Agent.md"));
}

#[test]
fn untouched_checks_distinguish_original_modified_missing_and_unrecorded_files() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    write_file(data_dir, "prompt_library/SOUL.md", "original");

    record_seed_entry(
        data_dir,
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/SOUL.md"],
    )
    .unwrap();

    assert!(is_file_untouched_seed(data_dir, "prompt_library/SOUL.md").unwrap());

    fs::write(data_dir.join("prompt_library/SOUL.md"), "modified").unwrap();
    assert!(!is_file_untouched_seed(data_dir, "prompt_library/SOUL.md").unwrap());

    fs::remove_file(data_dir.join("prompt_library/SOUL.md")).unwrap();
    assert!(!is_file_untouched_seed(data_dir, "prompt_library/SOUL.md").unwrap());
    assert!(!is_file_untouched_seed(data_dir, "prompt_library/OTHER.md").unwrap());
}

#[test]
fn entity_untouched_requires_every_current_file_to_be_recorded_and_unchanged() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();
    write_file(data_dir, "skill_pool/guidance/SKILL.md", "skill");
    write_file(
        data_dir,
        "skill_pool/guidance/references/config.md",
        "config",
    );

    record_seed_entry(
        data_dir,
        "skill_pool:guidance",
        "builtin_skill_pool",
        &[
            "skill_pool/guidance/SKILL.md",
            "skill_pool/guidance/references/config.md",
        ],
    )
    .unwrap();

    assert!(is_entity_untouched_seed(data_dir, "skill_pool/guidance").unwrap());

    fs::write(data_dir.join("skill_pool/guidance/SKILL.md"), "changed").unwrap();
    assert!(!is_entity_untouched_seed(data_dir, "skill_pool/guidance").unwrap());

    fs::write(data_dir.join("skill_pool/guidance/SKILL.md"), "skill").unwrap();
    write_file(data_dir, "skill_pool/guidance/new.md", "unrecorded");
    assert!(!is_entity_untouched_seed(data_dir, "skill_pool/guidance").unwrap());
}

#[test]
fn invalid_relative_paths_are_rejected() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path();

    let err = record_seed_entry(data_dir, "bad", "test", &["../outside"])
        .expect_err("parent traversal should be rejected");
    assert!(err.to_string().contains("invalid seed relative path"));

    let abs = data_dir.join("absolute.txt");
    fs::write(&abs, "x").unwrap();
    let err = record_seed_entry(data_dir, "bad", "test", &[abs.to_string_lossy().as_ref()])
        .expect_err("absolute path should be rejected");
    assert!(err.to_string().contains("invalid seed relative path"));
}

fn write_file(data_dir: &std::path::Path, rel: &str, content: &str) {
    let path = data_dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
