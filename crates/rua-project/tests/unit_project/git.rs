use std::process::Command;

use rua_project::{inspect_project_git, inspect_project_git_history};

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

    let status = inspect_project_git(root.path()).unwrap();

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
    let status = inspect_project_git(root.path()).unwrap();
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

    let page = inspect_project_git_history(root.path(), 1, 1).unwrap();

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

    let page = inspect_project_git_history(root.path(), 0, 20).unwrap();

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
