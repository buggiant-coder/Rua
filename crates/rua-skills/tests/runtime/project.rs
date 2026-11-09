use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rua_skills::{
    discover_project_skills, discover_project_skills_with_limits, project_skill_watch_plan,
    ProjectSkillDiagnosticKind, ProjectSkillDiscoveryLimits, ProjectSkillWatcher,
};

fn write_skill(directory: &std::path::Path, name: &str, body: &str) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} description\n---\n\n{body}"),
    )
    .unwrap();
}

#[test]
fn discovery_is_rooted_only_at_agents_skills_and_ignores_hidden_directories() {
    let host = tempfile::tempdir().unwrap();
    let project = host.path().join("project");
    write_skill(&project.join(".agents/skills/one"), "one", "one body");
    write_skill(
        &project.join(".agents/skills/nested/two"),
        "two",
        "two body",
    );
    write_skill(
        &project.join(".agents/skills/.hidden/secret"),
        "hidden",
        "hidden body",
    );
    write_skill(
        &project.join(".codex/skills/codex-only"),
        "codex-only",
        "codex body",
    );
    write_skill(
        &host.path().join(".agents/skills/parent"),
        "parent",
        "parent",
    );

    let discovery = discover_project_skills(&project);
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.parsed.name.as_str())
            .collect::<Vec<_>>(),
        vec!["two", "one"]
    );
    assert!(discovery
        .skills
        .iter()
        .all(|skill| skill.base_dir.starts_with(project.join(".agents/skills"))));
}

#[test]
fn malformed_skill_is_isolated_and_missing_root_is_empty() {
    let project = tempfile::tempdir().unwrap();
    assert!(discover_project_skills(project.path()).skills.is_empty());

    let root = project.path().join(".agents/skills");
    write_skill(&root.join("valid"), "valid", "valid body");
    std::fs::create_dir_all(root.join("invalid")).unwrap();
    std::fs::write(
        root.join("invalid/SKILL.md"),
        "---\nname: [broken\n---\nbody",
    )
    .unwrap();

    let discovery = discover_project_skills(project.path());
    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].parsed.name, "valid");
    assert!(discovery.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == ProjectSkillDiagnosticKind::SkillUnreadableOrMalformed
            && diagnostic.path.ends_with("invalid/SKILL.md")
    }));
}

#[test]
fn discovery_reports_depth_and_directory_bounds() {
    assert_eq!(
        ProjectSkillDiscoveryLimits::default(),
        ProjectSkillDiscoveryLimits {
            max_depth: 6,
            max_directories: 2_000,
        }
    );
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".agents/skills");
    write_skill(&root.join("level-one"), "level-one", "body");
    write_skill(
        &root.join("level-one/level-two/too-deep"),
        "too-deep",
        "body",
    );
    for name in ["a", "b", "c", "d"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }

    let depth_limited = discover_project_skills_with_limits(
        project.path(),
        ProjectSkillDiscoveryLimits {
            max_depth: 1,
            max_directories: 100,
        },
    );
    assert!(depth_limited
        .skills
        .iter()
        .all(|skill| skill.parsed.name != "too-deep"));
    assert!(depth_limited
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.kind == ProjectSkillDiagnosticKind::DepthLimitReached }));

    let directory_limited = discover_project_skills_with_limits(
        project.path(),
        ProjectSkillDiscoveryLimits {
            max_depth: 6,
            max_directories: 3,
        },
    );
    assert!(directory_limited.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == ProjectSkillDiagnosticKind::DirectoryLimitReached
    }));
}

#[cfg(unix)]
#[test]
fn unreadable_skill_is_skipped_without_hiding_valid_skills() {
    use std::os::unix::fs::PermissionsExt;

    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".agents/skills");
    write_skill(&root.join("valid"), "valid", "valid body");
    write_skill(&root.join("unreadable"), "unreadable", "unreadable body");
    let unreadable = root.join("unreadable/SKILL.md");
    let original = std::fs::metadata(&unreadable).unwrap().permissions();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();

    let discovery = discover_project_skills(project.path());

    std::fs::set_permissions(&unreadable, original).unwrap();
    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].parsed.name, "valid");
    assert!(discovery.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == ProjectSkillDiagnosticKind::SkillUnreadableOrMalformed
            && diagnostic.path.ends_with("unreadable/SKILL.md")
    }));
}

#[cfg(unix)]
#[test]
fn directory_and_skill_file_symlinks_work_without_duplicates_or_cycles() {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = project.path().join(".agents/skills");
    std::fs::create_dir_all(&root).unwrap();

    let directory_target = external.path().join("directory-target");
    write_skill(&directory_target, "directory-link", "directory link body");
    symlink(&root, directory_target.join("cycle")).unwrap();
    symlink(&directory_target, root.join("alias-one")).unwrap();
    symlink(&directory_target, root.join("alias-two")).unwrap();

    let file_target = external.path().join("linked-SKILL.md");
    std::fs::write(
        &file_target,
        "---\nname: file-link\ndescription: file link\n---\n\nfile link body",
    )
    .unwrap();
    let file_skill = root.join("file-skill");
    std::fs::create_dir_all(&file_skill).unwrap();
    symlink(&file_target, file_skill.join("SKILL.md")).unwrap();

    let discovery = discover_project_skills(project.path());
    let names = discovery
        .skills
        .iter()
        .map(|skill| skill.parsed.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "directory-link")
            .count(),
        1
    );
    assert_eq!(names.iter().filter(|name| **name == "file-link").count(), 1);
}

#[tokio::test]
async fn watcher_invalidates_creation_edit_removal_and_retargets_to_skill_root() {
    let project = tempfile::tempdir().unwrap();
    let changes = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&changes);
    let watcher = ProjectSkillWatcher::start(
        project.path(),
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert_eq!(watcher.plan(), &project_skill_watch_plan(project.path()));

    let skill = project.path().join(".agents/skills/watched");
    write_skill(&skill, "watched", "first");
    tokio::time::timeout(Duration::from_secs(5), async {
        while changes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("creation invalidation");
    drop(watcher);

    let changes = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&changes);
    let watcher = ProjectSkillWatcher::start(
        project.path(),
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert!(watcher.plan().recursive);
    assert!(watcher.plan().path.ends_with(".agents/skills"));

    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: watched\ndescription: watched\n---\n\nsecond",
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while changes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("edit invalidation");

    let before_remove = changes.load(Ordering::SeqCst);
    std::fs::remove_dir_all(&skill).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while changes.load(Ordering::SeqCst) <= before_remove {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("removal invalidation");
}

#[cfg(unix)]
#[tokio::test]
async fn watcher_invalidates_project_skill_symlink_retargeting() {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = project.path().join(".agents/skills");
    std::fs::create_dir_all(&root).unwrap();
    let first = external.path().join("first");
    let second = external.path().join("second");
    write_skill(&first, "linked", "first");
    write_skill(&second, "linked", "second");
    let link = root.join("linked");
    symlink(&first, &link).unwrap();

    let changes = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&changes);
    let _watcher = ProjectSkillWatcher::start(
        project.path(),
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::remove_file(&link).unwrap();
    symlink(&second, &link).unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while changes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("symlink retarget invalidation");
    assert_eq!(
        discover_project_skills(project.path()).skills[0]
            .parsed
            .body,
        "second"
    );
}
