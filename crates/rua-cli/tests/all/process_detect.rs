use rua_cli::process_utils::{is_rua_process, is_rua_running, kill_stale_rua};
use rua_config::dirs::Paths;

#[test]
fn is_rua_running_returns_none_for_nonexistent_rua_root() {
    let paths = Paths::new(Some("/tmp/rua-test-nonexistent-workspace-xyz"));
    let result = is_rua_running(&paths);
    assert!(
        result.is_none(),
        "no rua process should be running for a random Rua Root"
    );
}

#[test]
fn kill_stale_rua_fails_for_invalid_pid() {
    let result = kill_stale_rua(999_999_999);
    assert!(result.is_err(), "killing a non-existent PID should fail");
}

#[test]
fn is_rua_process_matches_binary_name() {
    let cmd: Vec<String> = vec!["/usr/local/bin/rua", "server", "--rua-root", "/tmp/ws1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(is_rua_process("rua", &cmd, "/tmp/ws1"));
}

#[test]
fn is_rua_process_matches_cmdline_rua_server() {
    let cmd: Vec<String> = vec!["python", "-m", "rua-server", "--rua-root", "/tmp/ws1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(is_rua_process("python", &cmd, "/tmp/ws1"));
}

#[test]
fn is_rua_process_no_match_different_binary() {
    let cmd: Vec<String> = vec!["nginx", "-g", "daemon off"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(!is_rua_process("nginx", &cmd, "/tmp/ws1"));
}

#[test]
fn is_rua_process_different_rua_root_has_no_conflict() {
    let cmd: Vec<String> = vec!["rua", "server", "--rua-root", "/tmp/ws-other"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(!is_rua_process("rua", &cmd, "/tmp/ws1"));
}

#[test]
fn is_rua_process_same_rua_root_equals_form() {
    let cmd: Vec<String> = vec!["rua", "server", "--rua-root=/tmp/ws1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(is_rua_process("rua", &cmd, "/tmp/ws1"));
}

#[test]
fn is_rua_process_matches_rua_root_flag_forms() {
    let split_form: Vec<String> = vec!["rua-server", "--rua-root", "/tmp/ws1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(is_rua_process("rua-server", &split_form, "/tmp/ws1"));
    assert!(!is_rua_process("rua-server", &split_form, "/tmp/ws2"));

    let equals_form: Vec<String> = vec!["rua-server", "--rua-root=/tmp/ws1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(is_rua_process("rua-server", &equals_form, "/tmp/ws1"));
    assert!(!is_rua_process("rua-server", &equals_form, "/tmp/ws2"));
}

#[test]
fn is_rua_process_no_root_arg_vs_default() {
    let cmd: Vec<String> = vec!["rua", "server"]
        .into_iter()
        .map(String::from)
        .collect();
    let default_dir = rua_config::dirs::rua_root();
    assert!(is_rua_process("rua", &cmd, &default_dir.to_string_lossy()));
}

#[test]
fn is_rua_process_no_root_arg_vs_custom_has_no_conflict() {
    let cmd: Vec<String> = vec!["rua", "server"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(!is_rua_process("rua", &cmd, "/some/custom/workspace"));
}
