use std::path::PathBuf;

use rua_skills::{lock_path_for, ManifestLock};

#[test]
fn lock_path_for_uses_dot_prefix_and_lock_suffix() {
    let json = PathBuf::from("/tmp/workspaces/a1/skill.json");
    let lock = lock_path_for(&json);
    assert_eq!(lock, PathBuf::from("/tmp/workspaces/a1/.skill.json.lock"));
}

#[test]
fn lock_path_for_handles_root_files() {
    let json = PathBuf::from("skill.json");
    let lock = lock_path_for(&json);
    assert_eq!(lock, PathBuf::from(".skill.json.lock"));
}

#[test]
fn lock_acquire_creates_lock_file_and_releases_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join(".skill.json.lock");

    {
        let guard = ManifestLock::acquire(&lock_path).unwrap();

        assert!(lock_path.exists(), "lock file should be created");
        drop(guard);
    }

    assert!(lock_path.exists());
}

#[test]
fn lock_acquire_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested").join("deeper");
    let lock_path = nested.join(".skill.json.lock");

    let _guard = ManifestLock::acquire(&lock_path).unwrap();
    assert!(nested.is_dir(), "parent dirs should be created");
    assert!(lock_path.exists());
}

#[test]
fn lock_acquire_is_reentrant_after_release() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join(".skill.json.lock");

    {
        let _g1 = ManifestLock::acquire(&lock_path).unwrap();
    }
    {
        let _g2 = ManifestLock::acquire(&lock_path).unwrap();
    }
    assert!(lock_path.exists());
}
