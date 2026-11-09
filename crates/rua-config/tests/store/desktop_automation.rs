use rua_config::desktop_automation::{preferences_path, DesktopAutomationPreferences};

#[test]
fn browser_preference_is_local_and_preserves_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("device-cache");
    let path = preferences_path(&cache);
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "browser_path":"/Applications/旧 浏览器", "future_setting":{"enabled":true}
        }),
    )
    .unwrap();
    let mut preferences = DesktopAutomationPreferences::load(&path).unwrap();
    preferences.browser_path = Some("/Applications/Chrome 测试/Google Chrome".into());
    preferences.save(&path).unwrap();
    let loaded = DesktopAutomationPreferences::load(&path).unwrap();
    assert_eq!(loaded, preferences);
    assert_eq!(loaded.extra["future_setting"]["enabled"], true);
    assert!(!temp.path().join("workspaces").exists());
    preferences.browser_path = None;
    preferences.save(&path).unwrap();
    assert!(DesktopAutomationPreferences::load(&path)
        .unwrap()
        .browser_path
        .is_none());
}

#[test]
fn invalid_preference_json_is_reported_without_resetting_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = preferences_path(temp.path());
    rua_config::atomic_write(&path, b"{invalid").unwrap();
    assert!(DesktopAutomationPreferences::load(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{invalid");
}
