use rua_channel::core::AllowlistPolicy;

#[test]
fn allow_all_permits_any_sender() {
    let policy = AllowlistPolicy::allow_all();

    assert!(policy.is_allowed("user1"));
    assert!(policy.is_allowed("unknown"));
}

#[test]
fn allowlist_only_permits_listed_senders() {
    let policy = AllowlistPolicy::allowlist(vec!["alice".to_string(), "bob".to_string()]);

    assert!(policy.is_allowed("alice"));
    assert!(policy.is_allowed("bob"));
    assert!(!policy.is_allowed("charlie"));
}

#[test]
fn denylist_blocks_only_listed_senders() {
    let policy = AllowlistPolicy::denylist(vec!["spammer".to_string()]);

    assert!(!policy.is_allowed("spammer"));
    assert!(policy.is_allowed("legit_user"));
}

#[test]
fn empty_allowlist_blocks_all_senders() {
    let policy = AllowlistPolicy::allowlist(Vec::<String>::new());

    assert!(!policy.is_allowed("anyone"));
}
