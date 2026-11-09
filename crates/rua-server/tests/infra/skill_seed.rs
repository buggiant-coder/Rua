use rua_server::seed::skills::ensure_builtin_skill_pool_seeded;
use tempfile::TempDir;

#[test]
fn skill_seed_records_entry_per_materialized_builtin_skill() {
    let tmp = TempDir::new().unwrap();

    let report = ensure_builtin_skill_pool_seeded(tmp.path());

    assert!(report
        .outcomes
        .iter()
        .any(|outcome| outcome.entry_id == "skill_pool:guidance"));
    assert!(tmp.path().join("skill_pool/guidance/SKILL.md").is_file());
    assert!(tmp.path().join("skill_pool/cron/SKILL.md").is_file());
    assert!(
        rua_core::seed_manifest::is_entry_materialized(tmp.path(), "skill_pool:guidance").unwrap()
    );
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "skill_pool/guidance/SKILL.md"
    )
    .unwrap());
    assert!(
        rua_core::seed_manifest::is_entity_untouched_seed(tmp.path(), "skill_pool/guidance")
            .unwrap()
    );
}

#[test]
fn skill_seed_does_not_claim_existing_skill_directory_without_manifest_entry() {
    let tmp = TempDir::new().unwrap();
    let guidance = tmp.path().join("skill_pool/guidance");
    std::fs::create_dir_all(&guidance).unwrap();
    std::fs::write(guidance.join("SKILL.md"), "custom guidance").unwrap();

    ensure_builtin_skill_pool_seeded(tmp.path());

    assert!(
        !rua_core::seed_manifest::is_entry_materialized(tmp.path(), "skill_pool:guidance").unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(guidance.join("SKILL.md")).unwrap(),
        "custom guidance"
    );
    assert!(rua_core::seed_manifest::is_entry_materialized(tmp.path(), "skill_pool:cron").unwrap());
}

#[test]
fn modifying_one_seeded_skill_only_marks_that_skill_entity_as_modified() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_skill_pool_seeded(tmp.path());

    std::fs::write(
        tmp.path().join("skill_pool/guidance/SKILL.md"),
        "modified guidance",
    )
    .unwrap();

    assert!(
        !rua_core::seed_manifest::is_entity_untouched_seed(tmp.path(), "skill_pool/guidance")
            .unwrap()
    );
    assert!(
        rua_core::seed_manifest::is_entity_untouched_seed(tmp.path(), "skill_pool/cron").unwrap()
    );
}

#[test]
fn skill_seed_does_not_recreate_deleted_materialized_skill() {
    let tmp = TempDir::new().unwrap();
    ensure_builtin_skill_pool_seeded(tmp.path());
    std::fs::remove_dir_all(tmp.path().join("skill_pool/guidance")).unwrap();

    ensure_builtin_skill_pool_seeded(tmp.path());

    assert!(!tmp.path().join("skill_pool/guidance").exists());
    assert!(
        rua_core::seed_manifest::is_entry_materialized(tmp.path(), "skill_pool:guidance").unwrap()
    );
}
