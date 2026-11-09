use std::process::Command;

fn rua_bin() -> String {
    env!("CARGO_BIN_EXE_rua").to_string()
}

#[test]
fn doctor_with_workspace_reports_custom_path() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("test-workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let output = Command::new(rua_bin())
        .args([
            "--rua-root",
            workspace.to_str().unwrap(),
            "doctor",
            "--timeout",
            "0",
        ])
        .output()
        .expect("failed to run rua doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(workspace.to_str().unwrap()),
        "doctor output should reference custom workspace path.\nGot:\n{stdout}"
    );
}

#[test]
fn doctor_without_workspace_uses_default() {
    let output = Command::new(rua_bin())
        .args(["doctor", "--timeout", "0"])
        .output()
        .expect("failed to run rua doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(".rua"),
        "doctor output should reference default .rua path.\nGot:\n{stdout}"
    );
}

#[test]
fn doctor_fix_dry_run_with_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("fix-workspace");

    let output = Command::new(rua_bin())
        .args([
            "--rua-root",
            workspace.to_str().unwrap(),
            "doctor",
            "fix",
            "--dry-run",
        ])
        .output()
        .expect("failed to run rua doctor fix");

    assert!(
        output.status.success(),
        "doctor fix --dry-run should succeed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
