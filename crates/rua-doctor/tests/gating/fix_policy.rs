use rua_doctor::fix;

#[test]
fn safe_sync_noninteractive_and_interactive_fix_sets_are_explicit() {
    assert!(fix::is_safe_fix("ensure-working-dir"));
    assert!(fix::is_safe_fix("ensure-workspace-dirs"));
    assert!(fix::is_safe_fix("validate-all-jobs-json"));
    assert!(!fix::is_safe_fix("reconcile-workspace-skills"));
    assert!(!fix::is_safe_fix("rebuild-console-npm"));
    assert!(!fix::is_safe_fix("unknown-id"));

    assert!(fix::is_sync_fix("reconcile-workspace-skills"));
    assert!(!fix::is_sync_fix("ensure-working-dir"));

    for fix_id in [
        "ensure-working-dir",
        "ensure-workspace-dirs",
        "validate-all-jobs-json",
        "reconcile-workspace-skills",
    ] {
        assert!(
            fix::is_noninteractive_fix(fix_id),
            "{fix_id} must be allowed in non-interactive doctor fix runs"
        );
    }

    assert!(fix::is_interactive_fix("seed-missing-agent-json"));
    assert!(fix::is_interactive_fix("reset-invalid-agent-json"));
    assert!(fix::is_interactive_fix("install-git-bash"));
    assert!(!fix::is_interactive_fix("ensure-working-dir"));
    assert!(!fix::is_interactive_fix("reconcile-workspace-skills"));
}

#[test]
fn known_fix_covers_safe_sync_and_interactive_but_rejects_unknown_ids() {
    assert!(fix::is_known_fix("ensure-working-dir"));
    assert!(fix::is_known_fix("validate-all-jobs-json"));
    assert!(fix::is_known_fix("reconcile-workspace-skills"));
    assert!(fix::is_known_fix("seed-missing-agent-json"));
    assert!(fix::is_known_fix("install-git-bash"));
    assert!(!fix::is_known_fix("unknown-id"));
}

#[test]
fn install_git_bash_uses_explicit_winget_command() {
    assert_eq!(
        fix::git_bash_winget_command(),
        vec![
            "winget",
            "install",
            "--id",
            "Git.Git",
            "-e",
            "--source",
            "winget",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]
    );
}

#[test]
fn unknown_fix_id_returns_a_failed_public_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = fix::run_fix("nonexistent-fix", tmp.path());

    assert_eq!(result.fix_id, "nonexistent-fix");
    assert!(!result.success);
    assert!(result.message.contains("unknown fix_id"));
    assert!(result.actions.is_empty());
}

#[test]
fn ensure_working_dir_reports_existing_directory_without_mutation_actions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = fix::run_fix("ensure-working-dir", tmp.path());

    assert_eq!(result.fix_id, "ensure-working-dir");
    assert!(result.success);
    assert!(result.message.contains("already exists"));
    assert!(result.actions.is_empty());
}
