use rua_command::{build_control_registry, build_daemon_registry};

#[test]
fn control_command_inventory() {
    let reg = build_control_registry();
    let names: Vec<&str> = reg.list().iter().map(|(n, _)| *n).collect();
    let expected = ["skills", "stop"];
    assert_eq!(names, expected, "control command set drifted");
}

#[test]
fn daemon_command_inventory() {
    let reg = build_daemon_registry();
    let entries = reg.list();

    assert_eq!(
        entries.len(),
        10,
        "daemon registry: 9 commands + 1 alias = 10 entries"
    );

    let mut unique: Vec<&str> = entries.iter().map(|(n, _)| *n).collect();
    unique.dedup();
    let expected = [
        "approval",
        "approve",
        "daemon",
        "deny",
        "logs",
        "reload-config",
        "restart",
        "status",
        "version",
    ];
    assert_eq!(unique, expected, "daemon unique command set drifted");

    assert!(
        reg.get("reload_config").is_some(),
        "reload_config alias must exist in daemon registry"
    );
}

#[test]
fn total_command_count_guard() {
    let control_count = build_control_registry().list().len();
    let daemon_entries = build_daemon_registry().list().len();

    const CONVERSATION_COUNT: usize = 13;

    let total = control_count + daemon_entries + CONVERSATION_COUNT;
    assert_eq!(
        total, 25,
        "total command inventory changed — update this test and priority registry"
    );
}
