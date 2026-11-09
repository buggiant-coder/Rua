use std::fs;
use std::path::{Path, PathBuf};

use rua_desktop::preferences::{
    resolve_configured_rua_root, DesktopLabels, DesktopLocale, DesktopPreferencesStore,
};

#[test]
fn rua_root_and_locale_persist_across_store_instances() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("desktop-preferences.json");
    let first = DesktopPreferencesStore::new(path.clone());

    first
        .set_rua_root(Path::new("/Users/test/Rua Data"))
        .expect("persist Rua Root");
    first
        .set_locale(DesktopLocale::Japanese)
        .expect("persist locale");

    let loaded = DesktopPreferencesStore::new(path)
        .load()
        .expect("reload preferences");
    assert_eq!(loaded.rua_root(), Some(Path::new("/Users/test/Rua Data")));
    assert_eq!(loaded.locale(), DesktopLocale::Japanese);
}

#[test]
fn preference_updates_preserve_unknown_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("desktop-preferences.json");
    fs::write(
        &path,
        r#"{
  "version": 1,
  "workingDir": "/Users/test/.rua",
  "locale": "zh",
  "futureSetting": {
    "enabled": true
  }
}"#,
    )
    .expect("seed preferences");

    DesktopPreferencesStore::new(path.clone())
        .set_locale(DesktopLocale::English)
        .expect("update locale");

    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("read preferences")).expect("valid JSON");
    assert_eq!(value["locale"], "en");
    assert_eq!(value["futureSetting"]["enabled"], true);
}

#[test]
fn default_rua_root_is_under_the_user_home_including_debug_builds() {
    assert_eq!(
        resolve_configured_rua_root(None, None, PathBuf::from("/Users/test")),
        PathBuf::from("/Users/test/.rua")
    );
}

#[test]
fn explicit_rua_root_overrides_persisted_preference() {
    let resolved = resolve_configured_rua_root(
        Some(PathBuf::from("/tmp/explicit")),
        Some(PathBuf::from("/tmp/persisted")),
        PathBuf::from("/tmp/default"),
    );
    assert_eq!(resolved, PathBuf::from("/tmp/explicit"));

    let persisted = resolve_configured_rua_root(
        None,
        Some(PathBuf::from("/tmp/persisted")),
        PathBuf::from("/tmp/default"),
    );
    assert_eq!(persisted, PathBuf::from("/tmp/persisted"));
}

#[test]
fn locale_normalization_drives_native_menu_labels() {
    assert_eq!(
        DesktopLocale::from_tag("zh-CN"),
        DesktopLocale::SimplifiedChinese
    );
    assert_eq!(DesktopLocale::from_tag("ja-JP"), DesktopLocale::Japanese);
    assert_eq!(DesktopLocale::from_tag("ru-RU"), DesktopLocale::Russian);
    assert_eq!(DesktopLocale::from_tag("fr-FR"), DesktopLocale::English);

    let labels = DesktopLabels::for_locale(DesktopLocale::SimplifiedChinese);
    assert_eq!(labels.switch_rua_root, "切换 Rua 根目录并重启 Runtime...");
    assert_eq!(labels.show, "显示 Rua");
    assert_eq!(labels.quit, "退出 Rua");
}

#[test]
fn quit_confirmation_defaults_on_for_new_and_existing_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let store = DesktopPreferencesStore::new(temp.path().join("preferences.json"));
    assert!(store.load().unwrap().confirm_before_quit());
    let old: rua_desktop::preferences::DesktopPreferences =
        serde_json::from_str(r#"{"version":1,"locale":"en","futureSetting":42}"#).unwrap();
    assert!(old.confirm_before_quit());
    assert_eq!(serde_json::to_value(old).unwrap()["futureSetting"], 42);
}

#[test]
fn quit_confirmation_can_be_disabled_and_reenabled_across_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("preferences.json");
    let store = DesktopPreferencesStore::new(path.clone());
    store.set_locale(DesktopLocale::Japanese).unwrap();
    store.set_rua_root(Path::new("/tmp/rua-quit-test")).unwrap();
    store.set_confirm_before_quit(false).unwrap();

    let reloaded = DesktopPreferencesStore::new(path);
    let preferences = reloaded.load().unwrap();
    assert!(!preferences.confirm_before_quit());
    assert_eq!(preferences.locale(), DesktopLocale::Japanese);
    assert_eq!(
        preferences.rua_root(),
        Some(Path::new("/tmp/rua-quit-test"))
    );
    reloaded.set_confirm_before_quit(true).unwrap();
    assert!(store.load().unwrap().confirm_before_quit());
}
