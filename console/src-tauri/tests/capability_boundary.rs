#[test]
fn desktop_capabilities_are_scoped_to_main_loopback_and_named_commands() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json"))
            .expect("capability JSON");

    assert_eq!(manifest["windows"], serde_json::json!(["main"]));
    assert_eq!(
        manifest["remote"]["urls"],
        serde_json::json!(["http://127.0.0.1:*", "http://localhost:*"])
    );
    let permissions = manifest["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    assert!(permissions.contains(&"desktop-runtime"));
    assert!(permissions.contains(&"notification:allow-is-permission-granted"));
    assert!(permissions.contains(&"notification:allow-request-permission"));
    assert!(
        permissions
            .iter()
            .all(|permission| !permission.starts_with("shell:")),
        "Console JavaScript must not receive shell plugin permissions"
    );
    assert!(
        permissions
            .iter()
            .all(|permission| !permission.starts_with("fs:")),
        "Console JavaScript must not receive unrestricted filesystem permissions"
    );

    let native_permission = include_str!("../permissions/desktop-native.toml");
    for command in [
        "save_desktop_file",
        "open_external_link",
        "open_llm_debug_capture_directory",
        "open_project_resource_directory",
        "show_user_notification",
    ] {
        assert!(
            native_permission.contains(&format!("\"{command}\"")),
            "missing named Desktop Native command permission: {command}"
        );
    }
    let commands = include_str!("../src/lib.rs");
    for command in [
        "desktop_install_status",
        "install_command_line_tool",
        "migrate_legacy_runtime",
        "desktop_update_status",
        "check_for_desktop_update",
        "install_desktop_update",
        "desktop_computer_recheck",
        "desktop_computer_restart",
        "desktop_computer_request_permissions",
        "desktop_computer_open_permission_settings",
    ] {
        assert!(
            !native_permission.contains(command),
            "Runtime capability in Shell ACL: {command}"
        );
        assert!(
            !commands.contains(command),
            "Runtime capability in Tauri invoke handler: {command}"
        );
    }
    assert!(!native_permission.contains("shell:"));
    assert!(!native_permission.contains("fs:"));

    let runtime_permission = include_str!("../permissions/desktop-runtime.toml");
    for command in [
        "desktop_bootstrap_context",
        "select_desktop_rua_root",
        "set_desktop_locale",
    ] {
        assert!(
            runtime_permission.contains(&format!("\"{command}\"")),
            "missing named Desktop Runtime command permission: {command}"
        );
    }
}
