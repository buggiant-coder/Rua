use std::path::{Path, PathBuf};
use std::process::Command;

use rua_core::runtime_authority::{RuntimeAuthority, RuntimeAuthorityConfig};
use rua_core::runtime_layout::{HostProfile, RuntimeKind, RuntimeLayout};
use rua_project::{
    inspect_project_git_history_with, inspect_project_git_with, project_git_launch_plan,
};

/// Deterministic desktop Runtime Authority, independent of the host test environment.
fn desktop_authority(root: &Path) -> RuntimeAuthority {
    RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout {
            host_profile: HostProfile::Desktop,
            os_family: std::env::consts::OS.to_string(),
            linux_compat: None,
        },
        RuntimeAuthorityConfig {
            workspace_dir: root.to_path_buf(),
            cache_dir: root.join(".cache"),
            path_entries: std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .collect::<Vec<PathBuf>>(),
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    )
}

fn inspect_git(root: &Path) -> Result<rua_project::ProjectGitStatus, String> {
    inspect_project_git_with(&desktop_authority(root), root)
}

fn inspect_git_history(
    root: &Path,
    offset: usize,
    limit: usize,
) -> Result<rua_project::ProjectGitHistoryPage, String> {
    inspect_project_git_history_with(&desktop_authority(root), root, offset, limit)
}

#[test]
fn git_status_reports_branch_head_and_root_confined_changes() {
    let root = tempfile::tempdir().unwrap();
    run_git(root.path(), &["init", "-b", "main"]);
    run_git(root.path(), &["config", "user.email", "test@example.com"]);
    run_git(root.path(), &["config", "user.name", "Test"]);
    std::fs::write(root.path().join("tracked.txt"), "initial\n").unwrap();
    run_git(root.path(), &["add", "tracked.txt"]);
    run_git(root.path(), &["commit", "-m", "initial commit"]);
    std::fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();
    std::fs::write(root.path().join("new.txt"), "new\n").unwrap();

    let status = inspect_git(root.path()).unwrap();

    assert!(status.repository);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.head.as_ref().unwrap().summary, "initial commit");
    assert!(status
        .changes
        .iter()
        .any(|change| change.path == "tracked.txt" && change.status == "M"));
    assert!(status
        .changes
        .iter()
        .any(|change| change.path == "new.txt" && change.status == "?"));
}

#[test]
fn non_repository_returns_empty_status() {
    let root = tempfile::tempdir().unwrap();
    let status = inspect_git(root.path()).unwrap();
    assert!(!status.repository);
    assert!(status.changes.is_empty());
}

#[test]
fn git_history_returns_the_requested_page_and_total() {
    let root = tempfile::tempdir().unwrap();
    run_git(root.path(), &["init", "-b", "main"]);
    run_git(root.path(), &["config", "user.email", "test@example.com"]);
    run_git(root.path(), &["config", "user.name", "Test"]);
    for index in 1..=3 {
        std::fs::write(root.path().join("tracked.txt"), format!("{index}\n")).unwrap();
        run_git(root.path(), &["add", "tracked.txt"]);
        run_git(root.path(), &["commit", "-m", &format!("commit {index}")]);
    }

    let page = inspect_git_history(root.path(), 1, 1).unwrap();

    assert_eq!(page.offset, 1);
    assert_eq!(page.limit, 1);
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].summary, "commit 2");
    assert_eq!(page.items[0].parent_oids.len(), 1);
}

#[test]
fn git_history_includes_all_branch_topology_and_refs() {
    let root = tempfile::tempdir().unwrap();
    run_git(root.path(), &["init", "-b", "main"]);
    run_git(root.path(), &["config", "user.email", "test@example.com"]);
    run_git(root.path(), &["config", "user.name", "Test"]);
    std::fs::write(root.path().join("base.txt"), "base\n").unwrap();
    run_git(root.path(), &["add", "base.txt"]);
    run_git(root.path(), &["commit", "-m", "base"]);
    run_git(root.path(), &["switch", "-c", "feature"]);
    std::fs::write(root.path().join("feature.txt"), "feature\n").unwrap();
    run_git(root.path(), &["add", "feature.txt"]);
    run_git(root.path(), &["commit", "-m", "feature work"]);
    run_git(root.path(), &["switch", "main"]);
    std::fs::write(root.path().join("main.txt"), "main\n").unwrap();
    run_git(root.path(), &["add", "main.txt"]);
    run_git(root.path(), &["commit", "-m", "main work"]);
    run_git(
        root.path(),
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );

    let page = inspect_git_history(root.path(), 0, 20).unwrap();

    assert_eq!(page.total, 4);
    let merge = page
        .items
        .iter()
        .find(|commit| commit.summary == "merge feature")
        .unwrap();
    assert_eq!(merge.parent_oids.len(), 2);
    assert!(merge
        .refs
        .iter()
        .any(|reference| reference.contains("main")));
    assert!(page
        .items
        .iter()
        .any(|commit| commit.refs.iter().any(|reference| reference == "feature")));
}

fn run_git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git command failed: {args:?}");
}

#[test]
fn desktop_git_plan_keeps_the_native_process_environment() {
    let root = tempfile::tempdir().unwrap();
    let authority = desktop_authority(root.path());

    let plan = project_git_launch_plan(&authority, root.path()).unwrap();

    // Native hosts must not inherit the guest environment or the PRoot override.
    assert!(!plan.args.contains(&"-c".to_string()));
    assert!(!plan.args.iter().any(|arg| arg == "safe.directory=*"));
    assert!(plan.env.is_empty());
}

#[test]
fn mobile_git_plan_runs_inside_the_linux_compat_runtime() {
    let root = tempfile::tempdir().unwrap();
    let native = root.path().join("native");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(native.join("libproot.so"), b"").unwrap();
    let data_dir = root.path().join("files");
    let rootfs = data_dir.join("runtime/debian");
    std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
    std::fs::write(rootfs.join("usr/bin/env"), b"").unwrap();
    std::fs::write(rootfs.join("usr/bin/git"), b"").unwrap();

    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout {
            host_profile: HostProfile::MobileApk,
            os_family: "android".to_string(),
            linux_compat: Some(rua_core::runtime_layout::LinuxCompatLayout::android(
                &native,
                &data_dir,
                root.path(),
            )),
        },
        RuntimeAuthorityConfig {
            workspace_dir: root.path().join("workspace"),
            cache_dir: root.path().join("cache"),
            path_entries: Vec::new(),
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );
    assert_eq!(authority.runtime_kind(), RuntimeKind::LinuxCompat);

    let plan = project_git_launch_plan(&authority, root.path()).unwrap();

    // git must be reached through the runtime gate into PRoot, never through the host PATH.
    assert_eq!(
        plan.command.file_name().unwrap(),
        std::ffi::OsStr::new("librua-runtime-gate.so")
    );
    let proot_index = plan
        .args
        .iter()
        .position(|arg| arg.ends_with("libproot.so"))
        .expect("the gate must launch PRoot");
    assert!(
        plan.args[proot_index..].iter().any(|arg| arg == "git"),
        "git must be the guest program after PRoot arguments"
    );
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair[0] == "-c" && pair[1] == "safe.directory=*"));
    assert_eq!(
        plan.env.get("HOME").map(String::as_str),
        Some("/root"),
        "the guest environment must come from the Linux Compatibility layout"
    );
}
