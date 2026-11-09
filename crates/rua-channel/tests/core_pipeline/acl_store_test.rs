use rua_channel::core::acl_store::{acl_message, AccessControlStore};
use tempfile::tempdir;

#[test]
fn store_round_trip_persists_pending() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("access_control.json");

    let store = AccessControlStore::new(path.clone());
    store.record_pending("dingtalk", "u1", "hello there", "Alice");

    assert!(path.exists());
    let reopened = AccessControlStore::new(path);
    let pending = reopened.pending("dingtalk");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].user_id, "u1");
    assert_eq!(pending[0].channel, "dingtalk");
    assert_eq!(pending[0].first_message, "hello there");
    assert_eq!(pending[0].username, "Alice");
}

#[test]
fn record_pending_is_deduplicated() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    store.record_pending("onebot", "u9", "first", "");
    store.record_pending("onebot", "u9", "second message ignored", "");

    let pending = store.pending("onebot");
    assert_eq!(pending.len(), 1, "duplicate pending must not be appended");
    assert_eq!(pending[0].first_message, "first");
}

#[test]
fn whitelist_blacklist_default_false_for_unknown() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));
    assert!(!store.is_whitelisted("dingtalk", "nobody"));
    assert!(!store.is_blacklisted("dingtalk", "nobody"));
}

#[test]
fn hot_reload_picks_up_external_writes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("access_control.json");
    let store = AccessControlStore::new(path.clone());
    assert!(!store.is_whitelisted("dingtalk", "vip"));

    let json = r#"{"dingtalk":{"whitelist":{"vip":{"remark":"","username":""}},"blacklist":{},"pending":[]}}"#;

    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&path, json).unwrap();

    assert!(
        store.is_whitelisted("dingtalk", "vip"),
        "store should hot-reload external whitelist change"
    );
}

#[test]
fn whitelist_add_remove_and_blacklist_are_mutually_exclusive() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    store.add_to_blacklist("telegram", "u1", "blocked", "Alice");
    assert!(store.is_blacklisted("telegram", "u1"));
    assert!(!store.is_whitelisted("telegram", "u1"));

    store.add_to_whitelist("telegram", "u1", "trusted", "");
    assert!(store.is_whitelisted("telegram", "u1"));
    assert!(!store.is_blacklisted("telegram", "u1"));

    let acl = store.get_acl("telegram");
    assert_eq!(acl.whitelist["u1"].remark, "trusted");
    assert_eq!(
        acl.whitelist["u1"].username, "Alice",
        "blank username should preserve existing metadata"
    );

    store.remove_from_whitelist("telegram", "u1");
    assert!(!store.is_whitelisted("telegram", "u1"));
}

#[test]
fn pending_approve_carries_remark_username_and_removes_pending() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    store.record_pending("feishu", "u2", "hello", "Bob");
    assert!(store.update_pending_remark("feishu", "u2", "teammate"));

    assert!(store.approve_pending("feishu", "u2", ""));
    assert!(store.pending("feishu").is_empty());
    assert!(store.is_whitelisted("feishu", "u2"));
    assert!(!store.is_blacklisted("feishu", "u2"));

    let acl = store.get_acl("feishu");
    assert_eq!(acl.whitelist["u2"].remark, "teammate");
    assert_eq!(acl.whitelist["u2"].username, "Bob");
}

#[test]
fn pending_deny_override_remark_moves_to_blacklist() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    store.record_pending("wecom", "u3", "hello", "Carol");
    assert!(store.update_pending_remark("wecom", "u3", "old remark"));

    assert!(store.deny_pending("wecom", "u3", "spam"));
    assert!(store.pending("wecom").is_empty());
    assert!(store.is_blacklisted("wecom", "u3"));
    assert!(!store.is_whitelisted("wecom", "u3"));

    let acl = store.get_acl("wecom");
    assert_eq!(acl.blacklist["u3"].remark, "spam");
    assert_eq!(acl.blacklist["u3"].username, "Carol");
}

#[test]
fn dismiss_pending_removes_without_listing_user() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    store.record_pending("qq", "u4", "hello", "Dana");

    assert!(store.dismiss_pending("qq", "u4"));
    assert!(!store.dismiss_pending("qq", "u4"));
    assert!(store.pending("qq").is_empty());
    assert!(!store.is_whitelisted("qq", "u4"));
    assert!(!store.is_blacklisted("qq", "u4"));
}

#[test]
fn all_pending_is_sorted_newest_first_and_all_acls_snapshot_roundtrips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("access_control.json");
    let store = AccessControlStore::new(path.clone());

    store.record_pending("dingtalk", "old", "first", "Old");
    std::thread::sleep(std::time::Duration::from_millis(5));
    store.record_pending("telegram", "new", "second", "New");
    store.add_to_whitelist("telegram", "vip", "trusted", "Victor");

    let pending = store.get_all_pending();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].user_id, "new");
    assert_eq!(pending[1].user_id, "old");

    let all = store.get_all_acls();
    assert!(all.contains_key("dingtalk"));
    assert_eq!(all["telegram"].whitelist["vip"].username, "Victor");

    let reopened = AccessControlStore::new(path);
    assert_eq!(reopened.get_all_pending().len(), 2);
    assert_eq!(
        reopened.get_acl("telegram").whitelist["vip"].remark,
        "trusted"
    );
}

#[test]
fn update_remark_and_username_report_missing_entries() {
    let dir = tempdir().unwrap();
    let store = AccessControlStore::new(dir.path().join("access_control.json"));

    assert!(!store.update_remark("telegram", "missing", "remark"));
    assert!(!store.update_username("telegram", "missing", "Name"));
    assert!(!store.update_pending_remark("telegram", "missing", "remark"));

    store.add_to_whitelist("telegram", "u1", "", "");
    assert!(store.update_remark("telegram", "u1", "trusted"));
    assert!(store.update_username("telegram", "u1", "Alice"));

    let acl = store.get_acl("telegram");
    assert_eq!(acl.whitelist["u1"].remark, "trusted");
    assert_eq!(acl.whitelist["u1"].username, "Alice");
}

#[test]
fn i18n_blocked_covers_six_languages() {
    assert_eq!(
        acl_message("blocked", "zh", "x"),
        "您已被禁止访问此智能体。"
    );
    assert_eq!(
        acl_message("blocked", "en", "x"),
        "You have been blocked from this agent."
    );
    assert_eq!(
        acl_message("blocked", "ja", "x"),
        "このエージェントへのアクセスがブロックされています。"
    );
    assert_eq!(
        acl_message("blocked", "ru", "x"),
        "Вам заблокирован доступ к этому агенту."
    );
    assert_eq!(
        acl_message("blocked", "pt-BR", "x"),
        "Você foi bloqueado deste agente."
    );
    assert_eq!(
        acl_message("blocked", "id", "x"),
        "Anda telah diblokir dari agen ini."
    );
}

#[test]
fn i18n_pending_substitutes_sender_id() {
    let msg = acl_message("pending", "en", "user-42");
    assert!(msg.contains("user-42"));
    assert!(!msg.contains("{sender_id}"));

    let ja = acl_message("pending", "ja", "user-99");
    assert!(ja.contains("user-99"));
    assert!(!ja.contains("{sender_id}"));
}

#[test]
fn i18n_language_fallback() {
    assert_eq!(
        acl_message("blocked", "zh-Hant", "x"),
        "您已被禁止访问此智能体。"
    );

    assert_eq!(
        acl_message("blocked", "fr", "x"),
        "You have been blocked from this agent."
    );

    assert_eq!(
        acl_message("blocked", "", "x"),
        "You have been blocked from this agent."
    );
}

#[test]
fn i18n_unknown_key_is_empty() {
    assert_eq!(acl_message("nope", "en", "x"), "");
}
