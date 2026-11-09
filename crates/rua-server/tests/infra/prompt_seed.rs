use rua_server::seed::prompts::ensure_builtin_prompt_library_seeded;
use tempfile::TempDir;

#[test]
fn prompt_seed_records_builtin_entry_and_file_hashes() {
    let tmp = TempDir::new().unwrap();

    let report = ensure_builtin_prompt_library_seeded(tmp.path());

    assert!(report
        .outcomes
        .iter()
        .any(|outcome| outcome.entry_id == "prompt_library:builtin:v2"));
    assert!(rua_core::seed_manifest::is_entry_materialized(
        tmp.path(),
        "prompt_library:builtin:v2"
    )
    .unwrap());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "prompt_library/90_DEFAULT_PROFILE.md"
    )
    .unwrap());
    assert!(tmp
        .path()
        .join("prompt_library/92_QA_WORKFLOW.md")
        .is_file());
}

#[test]
fn prompt_seed_does_not_claim_existing_prompt_files_without_manifest_entry() {
    let tmp = TempDir::new().unwrap();
    let prompt_dir = tmp.path().join("prompt_library");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    for name in rua_prompts::builtin::BUILTIN_PROMPTS
        .iter()
        .map(|prompt| prompt.path)
    {
        std::fs::write(prompt_dir.join(name), format!("custom {name}")).unwrap();
    }

    ensure_builtin_prompt_library_seeded(tmp.path());

    assert!(!rua_core::seed_manifest::is_entry_materialized(
        tmp.path(),
        "prompt_library:builtin:v2"
    )
    .unwrap());
    assert_eq!(
        std::fs::read_to_string(prompt_dir.join("90_DEFAULT_PROFILE.md")).unwrap(),
        "custom 90_DEFAULT_PROFILE.md"
    );
}

#[test]
fn prompt_seed_records_only_missing_files_when_some_prompts_are_occupied() {
    let tmp = TempDir::new().unwrap();
    let prompt_dir = tmp.path().join("prompt_library");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::write(
        prompt_dir.join("90_DEFAULT_PROFILE.md"),
        "custom default profile",
    )
    .unwrap();

    ensure_builtin_prompt_library_seeded(tmp.path());

    let manifest = rua_core::seed_manifest::read_manifest(tmp.path())
        .unwrap()
        .unwrap();
    let entry = manifest.entries.get("prompt_library:builtin:v2").unwrap();
    assert!(!entry
        .paths
        .contains(&"prompt_library/90_DEFAULT_PROFILE.md".to_string()));
    assert!(entry
        .paths
        .contains(&"prompt_library/92_QA_WORKFLOW.md".to_string()));
    assert_eq!(
        std::fs::read_to_string(prompt_dir.join("90_DEFAULT_PROFILE.md")).unwrap(),
        "custom default profile"
    );
}

#[test]
fn prompt_seed_does_not_rerun_after_entry_materialized_even_if_file_deleted() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_prompt_library_seeded(tmp.path());
    std::fs::remove_file(tmp.path().join("prompt_library/90_DEFAULT_PROFILE.md")).unwrap();

    ensure_builtin_prompt_library_seeded(tmp.path());

    assert!(!tmp
        .path()
        .join("prompt_library/90_DEFAULT_PROFILE.md")
        .exists());
}

#[test]
fn prompt_seed_preserves_both_modified_and_untouched_old_files() {
    let tmp = TempDir::new().unwrap();
    let prompt_dir = tmp.path().join("prompt_library");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::write(prompt_dir.join("SOUL.md"), "seeded soul").unwrap();
    std::fs::write(prompt_dir.join("AGENT_zOther.md"), "seeded other").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "prompt_library:builtin",
        "legacy_builtin_prompt_library",
        &["prompt_library/SOUL.md", "prompt_library/AGENT_zOther.md"],
    )
    .unwrap();
    std::fs::write(prompt_dir.join("SOUL.md"), "user edited soul").unwrap();

    ensure_builtin_prompt_library_seeded(tmp.path());

    assert_eq!(
        std::fs::read_to_string(prompt_dir.join("SOUL.md")).unwrap(),
        "user edited soul"
    );
    assert_eq!(
        std::fs::read_to_string(prompt_dir.join("AGENT_zOther.md")).unwrap(),
        "seeded other"
    );
    assert!(prompt_dir.join("90_DEFAULT_PROFILE.md").is_file());
}
