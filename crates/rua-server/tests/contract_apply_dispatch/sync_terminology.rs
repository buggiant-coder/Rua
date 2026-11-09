#[test]
fn current_sync_code_uses_network_secret_terms_not_legacy_identity_terms() {
    let current_sources = [
        include_str!("../../src/endpoints/network_secret_backup.rs"),
        include_str!("../../../rua-sync/src/export.rs"),
        include_str!("../../../rua-sync/src/device_list.rs"),
    ];

    let forbidden_terms = [
        ["Master", " Identity"].concat(),
        ["Master", "Identity"].concat(),
        ["master", " identity"].concat(),
        ["Recipients", " List"].concat(),
        ["recipients", "-list"].concat(),
        ["age", " multi-recipient"].concat(),
    ];

    for src in current_sources {
        for forbidden in &forbidden_terms {
            assert!(
                !src.contains(forbidden),
                "current sync code/docs must not use legacy sync term: {forbidden}"
            );
        }
    }
}

#[test]
fn device_list_no_longer_exposes_recipient_list_helper() {
    let src = include_str!("../../../rua-sync/src/device_list.rs");

    assert!(
        !src.contains("fn recipients(") && !src.contains("pub fn recipients("),
        "Device List is UI/peer metadata, not recipient-list authority"
    );
}
