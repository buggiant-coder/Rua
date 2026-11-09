use rua_cli::process_utils::is_rua_process;

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn rua_process_matches_space_separated_rua_root_arg() {
    let cmd = args(&["rua", "server", "--rua-root", "/tmp/ws1"]);
    assert!(is_rua_process("rua", &cmd, "/tmp/ws1"));
}

#[test]
fn rua_process_matches_equals_rua_root_arg() {
    let cmd = args(&["rua", "server", "--rua-root=/tmp/ws2"]);
    assert!(is_rua_process("rua", &cmd, "/tmp/ws2"));
}

#[test]
fn rua_server_matches_space_separated_rua_root_arg() {
    let cmd = args(&["rua-server", "--rua-root", "/tmp/ws3"]);
    assert!(is_rua_process("rua-server", &cmd, "/tmp/ws3"));
}

#[test]
fn rua_server_matches_equals_rua_root_arg() {
    let cmd = args(&["rua", "server", "--rua-root=/tmp/ws4"]);
    assert!(is_rua_process("rua", &cmd, "/tmp/ws4"));
}

#[test]
fn rua_server_binary_name_matches_without_cmdline_marker() {
    let cmd = args(&["/usr/local/bin/rua-server", "--rua-root", "/tmp/ws5"]);
    assert!(is_rua_process("rua-server", &cmd, "/tmp/ws5"));
}

#[test]
fn rua_process_rejects_different_rua_root() {
    let cmd = args(&["rua", "server", "--rua-root", "/tmp/ws1"]);
    assert!(!is_rua_process("rua", &cmd, "/tmp/ws2"));
}

#[test]
fn rua_process_matches_lexically_equivalent_rua_root_paths() {
    let cmd = args(&["rua", "server", "--rua-root", "/tmp/rua-ws/"]);
    assert!(is_rua_process("rua", &cmd, "/tmp/rua-ws"));

    let cmd = args(&["rua-server", "--rua-root=/tmp/rua-ws/./project"]);
    assert!(is_rua_process("rua-server", &cmd, "/tmp/rua-ws/project"));
}

#[test]
fn rua_process_rejects_parent_dir_paths_that_normalize_elsewhere() {
    let cmd = args(&["rua", "server", "--rua-root", "/tmp/rua-ws/../other"]);
    assert!(!is_rua_process("rua", &cmd, "/tmp/rua-ws"));
}

#[test]
fn rua_server_rejects_different_rua_root() {
    let cmd = args(&["rua-server", "--rua-root", "/tmp/ws1"]);
    assert!(!is_rua_process("rua-server", &cmd, "/tmp/ws2"));
}

#[test]
fn rua_process_without_root_arg_matches_default_rua_root_only() {
    let cmd = args(&["rua", "server"]);
    let default_dir = rua_config::dirs::rua_root();

    assert!(is_rua_process("rua", &cmd, &default_dir.to_string_lossy()));
    assert!(!is_rua_process("rua", &cmd, "/some/custom/workspace"));
}

#[test]
fn non_rua_process_is_ignored_even_with_matching_rua_root() {
    let cmd = args(&["other", "--rua-root", "/tmp/ws1"]);
    assert!(!is_rua_process("other", &cmd, "/tmp/ws1"));
}
