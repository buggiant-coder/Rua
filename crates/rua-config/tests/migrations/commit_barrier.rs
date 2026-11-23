use std::sync::mpsc;
use std::time::Duration;

use rua_config::{
    atomic_write, atomic_write_if_absent, remove_file_committed, with_commit_barrier,
};

#[test]
fn release_commit_excludes_normal_writes_but_not_unrelated_files() {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("security.json");
    let journal = temp.path().join("handoff.json");
    atomic_write(&policy, b"grant").unwrap();
    let (entered, entering) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let writer = with_commit_barrier(std::slice::from_ref(&policy), || {
        let path = policy.clone();
        let writer = std::thread::spawn(move || {
            entered.send(()).unwrap();
            atomic_write(&path, b"revoked").unwrap();
            done.send(()).unwrap();
        });
        entering.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(completed.recv_timeout(Duration::from_millis(40)).is_err());
        assert_eq!(std::fs::read(&policy)?, b"grant");
        atomic_write(&journal, b"released")?;
        Ok(writer)
    })
    .unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    writer.join().unwrap();
    assert_eq!(std::fs::read(policy).unwrap(), b"revoked");
    assert_eq!(std::fs::read(journal).unwrap(), b"released");
}

#[test]
fn deletion_uses_the_same_commit_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("security.json");
    atomic_write(&policy, b"grant").unwrap();
    let (entered, entering) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let writer = with_commit_barrier(std::slice::from_ref(&policy), || {
        let path = policy.clone();
        let writer = std::thread::spawn(move || {
            entered.send(()).unwrap();
            remove_file_committed(&path).unwrap();
            done.send(()).unwrap();
        });
        entering.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(completed.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(policy.exists());
        Ok(writer)
    })
    .unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    writer.join().unwrap();
    assert!(!policy.exists());
}

#[cfg(unix)]
#[test]
fn directory_aliases_and_missing_parents_share_the_same_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let alias = temp.path().join("alias");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let path = real.join("missing/security.json");
    let alias_path = alias.join("missing/security.json");
    let (entered, entering) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let writer = with_commit_barrier(&[path.clone(), alias_path.clone()], || {
        let writer = std::thread::spawn(move || {
            entered.send(()).unwrap();
            atomic_write(&alias_path, b"grant").unwrap();
            done.send(()).unwrap();
        });
        entering.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(completed.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(!path.exists());
        Ok(writer)
    })
    .unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    writer.join().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"grant");
}

#[test]
fn empty_initialization_never_replaces_an_existing_committed_grant() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("permission_rules.json");
    assert!(atomic_write_if_absent(&path, b"[]").unwrap());
    atomic_write(&path, br#"[{"id":"existing"}]"#).unwrap();
    assert!(!atomic_write_if_absent(&path, b"[]").unwrap());
    assert_eq!(std::fs::read(path).unwrap(), br#"[{"id":"existing"}]"#);
}
