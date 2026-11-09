use rua_channel::core::allowlist::ChannelAcl;
use rua_channel::core::config::BaseChannelConfig;

fn cfg_with_acl(
    dm_policy: &str,
    group_policy: &str,
    allow: &[&str],
    deny: &[&str],
) -> BaseChannelConfig {
    BaseChannelConfig {
        dm_policy: dm_policy.into(),
        group_policy: group_policy.into(),
        allow_from: allow.iter().map(|s| s.to_string()).collect(),
        deny_from: deny.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn default_config_allows_all() {
    let acl = ChannelAcl::from_config(&BaseChannelConfig::default());
    assert!(acl.is_allowed("anyone", false));
    assert!(acl.is_allowed("anyone", true));
}

#[test]
fn dm_allowlist_blocks_unlisted() {
    let cfg = cfg_with_acl("allowlist", "allow_all", &["alice"], &[]);
    let acl = ChannelAcl::from_config(&cfg);

    assert!(acl.is_allowed("alice", false));
    assert!(!acl.is_allowed("bob", false));

    assert!(acl.is_allowed("bob", true));
}

#[test]
fn group_denylist_blocks_listed() {
    let cfg = cfg_with_acl("allow_all", "denylist", &[], &["spammer"]);
    let acl = ChannelAcl::from_config(&cfg);

    assert!(acl.is_allowed("spammer", false));
    assert!(!acl.is_allowed("spammer", true));
    assert!(acl.is_allowed("legit", true));
}

#[test]
fn both_policies_allowlist() {
    let cfg = cfg_with_acl("allowlist", "allowlist", &["vip"], &[]);
    let acl = ChannelAcl::from_config(&cfg);

    assert!(acl.is_allowed("vip", false));
    assert!(acl.is_allowed("vip", true));
    assert!(!acl.is_allowed("rando", false));
    assert!(!acl.is_allowed("rando", true));
}

#[test]
fn empty_allowlist_blocks_all() {
    let cfg = cfg_with_acl("allowlist", "allowlist", &[], &[]);
    let acl = ChannelAcl::from_config(&cfg);

    assert!(!acl.is_allowed("anyone", false));
    assert!(!acl.is_allowed("anyone", true));
}

#[test]
fn unknown_policy_string_defaults_to_allow_all() {
    let cfg = cfg_with_acl("something_invalid", "also_invalid", &[], &[]);
    let acl = ChannelAcl::from_config(&cfg);

    assert!(acl.is_allowed("anyone", false));
    assert!(acl.is_allowed("anyone", true));
}
