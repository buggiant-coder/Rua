use std::path::{Path, PathBuf};

use rua_cli::update::{has_rua_cargo_toml, is_cargo_install_path, InstallMethod};

#[test]
fn update_recognizes_cargo_install_bin_paths_without_running_cargo() {
    let unix_exe = Path::new("/Users/test/.cargo/bin/rua");
    let windows_exe = Path::new(r"C:\Users\test\.cargo\bin").join("rua.exe");

    assert!(is_cargo_install_path(unix_exe));
    assert!(is_cargo_install_path(&windows_exe));
}

#[test]
fn update_does_not_treat_arbitrary_bin_paths_as_cargo_installs() {
    for exe in [
        Path::new("/usr/local/bin/rua"),
        Path::new("/opt/homebrew/bin/rua"),
        Path::new("/tmp/cargo/bin/rua"),
    ] {
        assert!(
            !is_cargo_install_path(exe),
            "only real .cargo/bin paths should be cargo-install updates: {}",
            exe.display()
        );
    }
}

#[test]
fn update_detects_rua_rust_repository_from_cargo_toml_marker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("rua-rust");
    std::fs::create_dir_all(&repo).expect("repo dir");
    std::fs::write(
        repo.join("Cargo.toml"),
        r#"
[package]
name = "rua-cli"
"#,
    )
    .expect("cargo toml");

    assert!(has_rua_cargo_toml(&repo));
}

#[test]
fn update_rejects_missing_or_unrelated_cargo_toml() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("missing");
    let unrelated = tmp.path().join("unrelated");
    std::fs::create_dir_all(&missing).expect("missing dir");
    std::fs::create_dir_all(&unrelated).expect("unrelated dir");
    std::fs::write(
        unrelated.join("Cargo.toml"),
        r#"
[package]
name = "not-the-project"
"#,
    )
    .expect("unrelated cargo toml");

    assert!(!has_rua_cargo_toml(&missing));
    assert!(!has_rua_cargo_toml(&unrelated));
}

#[test]
fn install_method_display_is_stable_for_update_prompts() {
    assert_eq!(InstallMethod::CargoInstall.to_string(), "cargo install");
    assert_eq!(
        InstallMethod::Git {
            repo_dir: PathBuf::from("/repo/rua-rust"),
        }
        .to_string(),
        "git (/repo/rua-rust)"
    );
    assert_eq!(
        InstallMethod::Pip {
            installer: "uv pip".into(),
        }
        .to_string(),
        "uv pip"
    );
    assert_eq!(InstallMethod::Unknown.to_string(), "unknown");
}
