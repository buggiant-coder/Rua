use rua_channel::core::CommandRegistry;

#[test]
fn default_commands_have_correct_priority() {
    let reg = CommandRegistry::new();
    assert_eq!(reg.get_priority_level("/stop"), 0);

    assert_eq!(reg.get_priority_level("/acp"), 20);
    assert_eq!(reg.get_priority_level("/noacp"), 20);
    assert_eq!(reg.get_priority_level("/status"), 10);
    assert_eq!(reg.get_priority_level("/daemon status"), 10);
    assert_eq!(reg.get_priority_level("/daemon restart"), 10);
    assert_eq!(reg.get_priority_level("/version"), 10);
    assert_eq!(reg.get_priority_level("/logs"), 10);
    assert_eq!(reg.get_priority_level("/approve"), 10);
}

#[test]
fn daemon_status_wins_over_shorter_prefix() {
    let reg = CommandRegistry::new();

    assert!(reg.is_control_command("/daemon status"));
    assert_eq!(reg.get_priority_level("/daemon status"), 10);
}

#[test]
fn stopx_does_not_match() {
    let reg = CommandRegistry::new();
    assert!(!reg.is_control_command("/stopx"));
    assert_eq!(reg.get_priority_level("/stopx"), 20);
}

#[test]
fn non_slash_is_not_control() {
    let reg = CommandRegistry::new();
    assert!(!reg.is_control_command("hello"));
    assert!(!reg.is_control_command("stop"));
    assert_eq!(reg.get_priority_level("hello"), 20);
    assert_eq!(reg.get_priority_level(""), 20);
}

#[test]
fn case_insensitive_matching() {
    let reg = CommandRegistry::new();
    assert!(reg.is_control_command("/STOP"));
    assert!(reg.is_control_command("/Stop"));
    assert_eq!(reg.get_priority_level("/STOP"), 0);
    assert!(reg.is_control_command("/Daemon Status"));
    assert_eq!(reg.get_priority_level("/DAEMON STATUS"), 10);
}

#[test]
fn unknown_slash_command_returns_default() {
    let reg = CommandRegistry::new();
    assert!(!reg.is_control_command("/unknown"));
    assert_eq!(reg.get_priority_level("/unknown"), 20);
    assert_eq!(reg.get_priority_level("/compact"), 20);
}

#[test]
fn register_custom_command() {
    let mut reg = CommandRegistry::new();
    reg.register_command("/batch", 30);
    assert_eq!(reg.get_priority_level("/batch"), 30);
    assert!(reg.is_control_command("/batch"));
}

#[test]
fn prefix_boundary_whitespace() {
    let reg = CommandRegistry::new();

    assert!(reg.is_control_command("/stop "));
    assert!(reg.is_control_command("/stop something"));

    assert!(reg.is_control_command("/stop\tsomething"));

    assert!(reg.is_control_command("/stop\nsomething"));

    assert!(reg.is_control_command("/stop"));
}

#[test]
fn guard_control_commands_at_critical_level_0() {
    let reg = CommandRegistry::new();
    let critical = ["/stop"];
    for cmd in &critical {
        assert!(
            reg.is_control_command(cmd),
            "{cmd} must be in priority registry"
        );
        assert_eq!(
            reg.get_priority_level(cmd),
            0,
            "{cmd} must have critical priority 0"
        );
    }
}

#[test]
fn guard_daemon_subcommands_at_level_10() {
    let reg = CommandRegistry::new();
    let daemon_subs = [
        "/daemon status",
        "/daemon restart",
        "/daemon reload-config",
        "/daemon version",
        "/daemon logs",
        "/daemon approve",
    ];
    for cmd in &daemon_subs {
        assert!(
            reg.is_control_command(cmd),
            "{cmd} must be in priority registry"
        );
        assert_eq!(
            reg.get_priority_level(cmd),
            10,
            "{cmd} must have daemon priority 10"
        );
    }
}

#[test]
fn guard_daemon_short_aliases_at_level_10() {
    let reg = CommandRegistry::new();
    let aliases = [
        "/status",
        "/restart",
        "/reload-config",
        "/reload_config",
        "/version",
        "/logs",
        "/approve",
    ];
    for cmd in &aliases {
        assert!(
            reg.is_control_command(cmd),
            "{cmd} must be in priority registry"
        );
        assert_eq!(
            reg.get_priority_level(cmd),
            10,
            "{cmd} must have daemon priority 10"
        );
    }
}

#[test]
fn guard_conversation_commands_fallback_to_level_20() {
    let reg = CommandRegistry::new();
    let conversation_samples = [
        "/compact", "/new", "/clear", "/history", "/message", "/modes", "/mode", "/yes", "/no",
        "/prompts", "/prompt",
    ];
    for cmd in &conversation_samples {
        assert!(
            !reg.is_control_command(cmd),
            "{cmd} is a conversation command and must NOT be in priority registry"
        );
        assert_eq!(
            reg.get_priority_level(cmd),
            20,
            "{cmd} must fall back to default level 20"
        );
    }
}

#[test]
fn guard_priority_registry_entry_count() {
    let reg = CommandRegistry::new();

    let all_expected: Vec<(&str, u32)> = vec![
        ("/stop", 0),
        ("/daemon status", 10),
        ("/daemon restart", 10),
        ("/daemon reload-config", 10),
        ("/daemon version", 10),
        ("/daemon logs", 10),
        ("/daemon approve", 10),
        ("/status", 10),
        ("/restart", 10),
        ("/reload-config", 10),
        ("/reload_config", 10),
        ("/version", 10),
        ("/logs", 10),
        ("/approve", 10),
    ];
    for (cmd, expected_level) in &all_expected {
        assert_eq!(
            reg.get_priority_level(cmd),
            *expected_level,
            "priority mismatch for {cmd}"
        );
    }
    assert_eq!(
        all_expected.len(),
        14,
        "expected exactly 14 priority registry entries (1 critical + 13 daemon)"
    );
}
