use rua_provider::{resume_local_models, LocalModelStatus};

#[test]
fn empty_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let results = resume_local_models(dir.path());
    assert!(results.is_empty());
}

#[test]
fn no_local_models_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let results = resume_local_models(dir.path());
    assert!(results.is_empty());
}

#[test]
fn dead_pid_is_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let lm_dir = dir.path().join("local_models");
    std::fs::create_dir_all(&lm_dir).unwrap();

    let pid_file = lm_dir.join("test-model.pid");
    std::fs::write(&pid_file, "999999999").unwrap();

    let results = resume_local_models(dir.path());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].model_id, "test-model");
    assert_eq!(results[0].status, LocalModelStatus::Stopped);
    assert!(!pid_file.exists());
}

#[cfg(unix)]
#[test]
fn alive_pid_marked_running() {
    let dir = tempfile::tempdir().unwrap();
    let lm_dir = dir.path().join("local_models");
    std::fs::create_dir_all(&lm_dir).unwrap();

    let my_pid = std::process::id();
    let pid_file = lm_dir.join("self-model.pid");
    std::fs::write(&pid_file, my_pid.to_string()).unwrap();

    let results = resume_local_models(dir.path());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].model_id, "self-model");
    assert_eq!(results[0].pid, my_pid);
    assert_eq!(results[0].status, LocalModelStatus::Running);
    assert!(pid_file.exists());
}

#[test]
fn non_pid_files_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let lm_dir = dir.path().join("local_models");
    std::fs::create_dir_all(&lm_dir).unwrap();

    std::fs::write(lm_dir.join("readme.txt"), "not a pid").unwrap();
    std::fs::write(lm_dir.join("config.json"), "{}").unwrap();

    let results = resume_local_models(dir.path());
    assert!(results.is_empty());
}

#[test]
fn invalid_pid_content_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let lm_dir = dir.path().join("local_models");
    std::fs::create_dir_all(&lm_dir).unwrap();

    std::fs::write(lm_dir.join("bad.pid"), "not-a-number").unwrap();

    let results = resume_local_models(dir.path());
    assert!(results.is_empty());
}
