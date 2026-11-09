use std::path::Path;
use std::thread;

use rua_skills::{is_main_thread, SkillEnvOverrides};

#[test]
fn spawned_thread_is_not_main() {
    let result = thread::spawn(is_main_thread).join().unwrap();
    assert!(
        !result,
        "is_main_thread() must return false on a spawned thread",
    );
}

#[test]
fn named_thread_is_not_main_unless_named_main() {
    let result = thread::Builder::new()
        .name("worker-7".into())
        .spawn(is_main_thread)
        .unwrap()
        .join()
        .unwrap();
    assert!(
        !result,
        "is_main_thread() must return false for a thread named 'worker-7'",
    );

    let result_main = thread::Builder::new()
        .name("main".into())
        .spawn(is_main_thread)
        .unwrap()
        .join()
        .unwrap();
    assert!(
        result_main,
        "is_main_thread() must return true for a thread named 'main'",
    );
}

#[test]
fn activate_from_different_thread_does_not_panic() {
    let dummy = Path::new("/tmp/__nonexistent_skill_workspace__");
    let _guard = SkillEnvOverrides::activate(dummy, "console").unwrap();

    let join_result = thread::spawn(move || {
        let _ = SkillEnvOverrides::activate(dummy, "console");
    })
    .join();

    assert!(
        join_result.is_ok(),
        "activate() from a different thread must not kill a conversation task",
    );
}
