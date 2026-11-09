use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rua_skills::{
    builtin_signature, builtin_signatures, builtin_skill_names, ensure_skill_pool_initialized_in,
    extract_builtin_to, import_builtin_skills_in, list_builtin_import_candidates_in,
    update_single_builtin_in, BuiltinStatus, BUILTIN_SKILLS,
};
use serde_json::Value;
use tempfile::TempDir;

const EXPECTED_BUILTINS: &[&str] = &[
    "browser_cdp",
    "browser_visible",
    "channel_message",
    "rua_source_index",
    "cron",
    "dingtalk_channel",
    "docx",
    "file_reader",
    "guidance",
    "himalaya",
    "make_plan",
    "multi_agent_collaboration",
    "news",
    "pdf",
    "pptx",
    "toolchain-setup",
    "xlsx",
];

#[test]
fn embedded_dir_holds_all_seventeen_builtins() {
    let names = builtin_skill_names();
    assert_eq!(names.len(), 17, "expected exactly 17 builtin skills");
    let want: HashSet<&str> = EXPECTED_BUILTINS.iter().copied().collect();
    let got: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, want, "builtin skill set must match the BRIEF");

    for name in &names {
        let dir = BUILTIN_SKILLS
            .get_dir(name)
            .unwrap_or_else(|| panic!("BUILTIN_SKILLS missing {name}"));
        let has_skill_md = dir.entries().iter().any(|e| {
            e.as_file()
                .and_then(|f| f.path().file_name())
                .and_then(|n| n.to_str())
                == Some("SKILL.md")
        });
        assert!(has_skill_md, "{name}/SKILL.md not embedded");
    }
}

#[test]
fn embedded_dir_includes_subtrees_for_office_skills() {
    for skill in ["docx", "pptx", "xlsx"] {
        let scripts = BUILTIN_SKILLS
            .get_dir(skill)
            .unwrap_or_else(|| panic!("missing {skill} dir"))
            .get_dir(format!("{skill}/scripts"));
        assert!(scripts.is_some(), "{skill}/scripts/ not embedded");
    }
    let himalaya_refs = BUILTIN_SKILLS
        .get_dir("himalaya")
        .expect("missing himalaya dir")
        .get_file("himalaya/references/configuration.md");
    assert!(
        himalaya_refs.is_some(),
        "himalaya/references/configuration.md not embedded",
    );
}

#[test]
fn document_skills_extract_licensed_helpers_and_remove_obsolete_files() {
    let tmp = TempDir::new().unwrap();
    for name in ["docx", "pdf", "pptx", "xlsx"] {
        let target = tmp.path().join(name);
        extract_builtin_to(name, &target).unwrap();
        let parsed = rua_skills::parse_skill_md_file(&target).unwrap();
        assert_eq!(parsed.version, "2.0", "{name} import version");
        let license = fs::read_to_string(target.join("LICENSE.txt")).unwrap();
        assert!(license.contains("Version 2.0, January 2004"), "{name}");
        assert!(!license.contains("ADDITIONAL RESTRICTIONS"), "{name}");
        assert!(target.join("NOTICE.md").is_file(), "{name} attribution");
        if name != "pdf" {
            let old_helper = target.join("scripts/office/pack.py");
            fs::write(&old_helper, "obsolete helper fixture").unwrap();
            extract_builtin_to(name, &target).unwrap();
            assert!(!old_helper.exists(), "{name} must replace the old tree");
            assert!(target
                .join("scripts/office/helpers/pptx_chart.py")
                .is_file());
            assert!(!target.join("scripts/office/unpack.py").exists());
        }
    }
    assert!(tmp.path().join("docx/scripts/merge_runs.py").is_file());
    assert!(!tmp.path().join("pptx/editing.md").exists());
    assert!(!tmp.path().join("pptx/pptxgenjs.md").exists());
}

#[test]
fn builtin_signature_is_stable_and_per_skill() {
    let sigs = builtin_signatures();
    assert_eq!(sigs.len(), 17);
    for name in EXPECTED_BUILTINS {
        let sig = builtin_signature(name).unwrap_or_else(|| panic!("no signature for {name}"));
        assert_eq!(sig.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            sig.chars().all(|c| c.is_ascii_hexdigit()),
            "signature must be hex"
        );
    }

    let docx = builtin_signature("docx").unwrap();
    let pdf = builtin_signature("pdf").unwrap();
    assert_ne!(docx, pdf);
}

#[test]
fn toolchain_setup_is_instruction_only_and_uses_existing_agent_tools() {
    let dir = BUILTIN_SKILLS
        .get_dir("toolchain-setup")
        .expect("toolchain-setup builtin");
    let content = dir
        .get_file("toolchain-setup/SKILL.md")
        .and_then(|file| std::str::from_utf8(file.contents()).ok())
        .expect("toolchain-setup SKILL.md");

    assert!(content.contains("runtime_info"));
    assert!(content.contains("bash"));
    assert!(content.contains("明确确认"));
    assert!(content.contains("国内镜像"));
    assert!(
        dir.get_dir("toolchain-setup/scripts").is_none(),
        "Toolchain Setup must guide the Agent instead of shipping an installer script"
    );
}

#[test]
fn extract_builtin_writes_full_subtree() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("docx");
    extract_builtin_to("docx", &target).unwrap();

    assert!(target.join("SKILL.md").is_file());
    assert!(target.join("scripts").is_dir());

    assert!(
        target.join("scripts").join("accept_changes.py").is_file(),
        "scripts/accept_changes.py must be extracted",
    );

    extract_builtin_to("docx", &target).unwrap();
    assert!(target.join("SKILL.md").is_file());
}

#[test]
fn extract_builtin_unknown_name_errors() {
    let tmp = TempDir::new().unwrap();
    let err = extract_builtin_to("does_not_exist", &tmp.path().join("x"));
    assert!(err.is_err(), "extracting an unknown skill must fail");
}

#[test]
fn first_run_initializes_pool_with_seventeen_skill_md_files() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    let created = ensure_skill_pool_initialized_in(&pool).unwrap();
    assert!(
        created,
        "first ensure_skill_pool_initialized call must report `created`"
    );

    let count = std::fs::read_dir(&pool)
        .unwrap()
        .filter(|e| {
            let e = e.as_ref().unwrap();
            e.path().is_dir() && e.path().join("SKILL.md").exists()
        })
        .count();
    assert_eq!(count, 17, "pool must hold all 17 builtin skill dirs");

    assert!(pool.join("docx").join("scripts").is_dir());
    assert!(pool.join("himalaya").join("references").is_dir());
    assert!(pool
        .join("himalaya")
        .join("references")
        .join("configuration.md")
        .is_file());

    let manifest = read_pool_manifest(&pool);
    let names = manifest
        .get("builtin_skill_names")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(names, 17, "manifest.builtin_skill_names must list 17 names");
}

#[test]
fn ensure_skill_pool_initialized_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    assert!(ensure_skill_pool_initialized_in(&pool).unwrap());

    let created_again = ensure_skill_pool_initialized_in(&pool).unwrap();
    assert!(!created_again);

    let count = std::fs::read_dir(&pool)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().join("SKILL.md").exists())
        .count();
    assert_eq!(count, 17);
}

#[test]
fn ensure_existing_pool_does_not_reconcile_manifest() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    assert!(ensure_skill_pool_initialized_in(&pool).unwrap());
    let before = read_pool_manifest(&pool);

    let stray_dir = pool.join("locally_added");
    fs::create_dir_all(&stray_dir).unwrap();
    fs::write(
        stray_dir.join("SKILL.md"),
        "---\nname: locally_added\ndescription: manually copied\n---\n# Local\n",
    )
    .unwrap();

    assert!(!ensure_skill_pool_initialized_in(&pool).unwrap());
    let after = read_pool_manifest(&pool);
    assert_eq!(
        after, before,
        "read-path initialization must not rescan and rewrite the pool manifest",
    );
}

#[test]
fn list_builtin_import_candidates_starts_all_missing() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    let cands = list_builtin_import_candidates_in(&pool).unwrap();
    assert_eq!(cands.len(), 17);
    for c in &cands {
        assert_eq!(
            c.status,
            BuiltinStatus::Missing,
            "{} must start as Missing",
            c.name
        );
        assert_eq!(c.current_version_text, "");
        assert_eq!(c.current_source, "");
    }
}

#[test]
fn list_returns_current_after_first_import() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    let report = import_builtin_skills_in(&pool, None, false).unwrap();
    assert_eq!(report.imported.len(), 17);
    assert!(report.updated.is_empty());
    assert!(report.unchanged.is_empty());
    assert!(report.conflicts.is_empty());

    let cands = list_builtin_import_candidates_in(&pool).unwrap();
    for c in &cands {
        assert_eq!(
            c.status,
            BuiltinStatus::Current,
            "{} should be Current",
            c.name
        );
        assert_eq!(c.current_source, "builtin");
    }

    let report2 = import_builtin_skills_in(&pool, None, false).unwrap();
    assert!(report2.imported.is_empty());
    assert!(report2.updated.is_empty());
    assert_eq!(report2.unchanged.len(), 17);
}

#[test]
fn outdated_status_triggers_update() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    import_builtin_skills_in(&pool, None, false).unwrap();

    patch_pool_manifest_signature(&pool, "cron", "deadbeef");
    let candidates = list_builtin_import_candidates_in(&pool).unwrap();
    let cron = candidates.iter().find(|c| c.name == "cron").unwrap();
    assert_eq!(cron.status, BuiltinStatus::Outdated);

    let report = import_builtin_skills_in(&pool, Some(&["cron".to_string()]), false).unwrap();
    assert_eq!(report.updated, vec!["cron".to_string()]);
    assert!(report.imported.is_empty());
    assert!(report.unchanged.is_empty());
    assert!(report.conflicts.is_empty());

    let candidates = list_builtin_import_candidates_in(&pool).unwrap();
    let cron = candidates.iter().find(|c| c.name == "cron").unwrap();
    assert_eq!(cron.status, BuiltinStatus::Current);
}

#[test]
fn import_skips_customized_conflict_unless_overwritten() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    fs::create_dir_all(&pool).unwrap();

    let cron_dir = pool.join("cron");
    fs::create_dir_all(&cron_dir).unwrap();
    fs::write(
        cron_dir.join("SKILL.md"),
        "---\nname: cron\ndescription: user-tweaked\n---\n# my cron\n",
    )
    .unwrap();

    rua_skills::reconcile_pool_manifest_with(&pool, &rua_skills::builtin_signatures()).unwrap();

    let candidates = list_builtin_import_candidates_in(&pool).unwrap();
    let cron = candidates.iter().find(|c| c.name == "cron").unwrap();
    assert_eq!(
        cron.status,
        BuiltinStatus::Conflict,
        "customized pool slot must be classified as Conflict"
    );

    let report = import_builtin_skills_in(&pool, Some(&["cron".to_string()]), false).unwrap();
    assert!(report.imported.is_empty());
    assert!(report.updated.is_empty());
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].name, "cron");

    let body = fs::read_to_string(cron_dir.join("SKILL.md")).unwrap();
    assert!(body.contains("user-tweaked"));

    let report = import_builtin_skills_in(&pool, Some(&["cron".to_string()]), true).unwrap();
    assert_eq!(report.updated, vec!["cron".to_string()]);
    assert_eq!(
        report.conflicts.len(),
        1,
        "conflicts list still records the conflict",
    );
    let body = fs::read_to_string(cron_dir.join("SKILL.md")).unwrap();
    assert!(!body.contains("user-tweaked"));
}

#[test]
fn ensure_skill_pool_initialized_skips_existing_customized_slot() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    fs::create_dir_all(&pool).unwrap();

    let cron_dir = pool.join("cron");
    fs::create_dir_all(&cron_dir).unwrap();
    let custom_body = "---\nname: cron\ndescription: legacy user copy\n---\n# my cron\n";
    fs::write(cron_dir.join("SKILL.md"), custom_body).unwrap();

    rua_skills::reconcile_pool_manifest_with(&pool, &rua_skills::builtin_signatures()).unwrap();

    ensure_skill_pool_initialized_in(&pool).unwrap();

    let body = fs::read_to_string(cron_dir.join("SKILL.md")).unwrap();
    assert!(
        body.contains("legacy user copy"),
        "ensure_skill_pool_initialized must not overwrite a customized slot",
    );
}

#[test]
fn update_single_builtin_refreshes_outdated_slot() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    import_builtin_skills_in(&pool, None, false).unwrap();

    patch_pool_manifest_signature(&pool, "cron", "deadbeef");

    let cand = update_single_builtin_in(&pool, "cron").unwrap();
    assert_eq!(cand.status, BuiltinStatus::Current);
}

#[test]
fn update_single_builtin_rejects_customized_slot() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    fs::create_dir_all(&pool).unwrap();
    let cron_dir = pool.join("cron");
    fs::create_dir_all(&cron_dir).unwrap();
    fs::write(
        cron_dir.join("SKILL.md"),
        "---\nname: cron\n---\n# user cron\n",
    )
    .unwrap();
    rua_skills::reconcile_pool_manifest_with(&pool, &rua_skills::builtin_signatures()).unwrap();

    let err = update_single_builtin_in(&pool, "cron")
        .expect_err("update_single_builtin must refuse to overwrite a customized slot");
    let msg = err.to_string();
    assert!(
        msg.contains("cron"),
        "error must name the conflicting skill"
    );
}

#[test]
fn update_single_builtin_rejects_unknown_name() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    let err =
        update_single_builtin_in(&pool, "no_such_skill").expect_err("unknown builtin must error");
    assert!(err.to_string().contains("no_such_skill"));
}

fn read_pool_manifest(pool: &Path) -> Value {
    let bytes = fs::read(pool.join("skill.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn patch_pool_manifest_signature(pool: &Path, name: &str, sig: &str) {
    let path = pool.join("skill.json");
    let bytes = fs::read(&path).unwrap();
    let mut payload: Value = serde_json::from_slice(&bytes).unwrap();
    if let Some(entry) = payload
        .get_mut("skills")
        .and_then(|v| v.as_object_mut())
        .and_then(|o| o.get_mut(name))
        .and_then(|v| v.as_object_mut())
    {
        entry.insert("signature".into(), Value::String(sig.to_string()));
    }
    fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}
