use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::AliyunProvider;

#[test]
#[serial_test::serial]
fn unavailable_when_env_vars_unset() {
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");

    let provider = AliyunProvider::new();
    let (avail, reason) = provider.available();

    assert!(!avail);
    let reason = reason.unwrap();
    assert!(
        reason.contains("ALIBABA_CLOUD_ACCESS_KEY_ID"),
        "reason should mention missing key id: {reason}",
    );
    assert!(
        reason.contains("ALIBABA_CLOUD_ACCESS_KEY_SECRET"),
        "reason should mention missing secret: {reason}",
    );
}

#[test]
#[serial_test::serial]
fn available_when_env_vars_set() {
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_ID", "test-id");
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET", "test-secret");

    let provider = AliyunProvider::new();
    let (avail, reason) = provider.available();

    assert!(avail);
    assert!(reason.is_none());

    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");
}

#[test]
#[serial_test::serial]
fn unavailable_when_only_id_set() {
    std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_ID", "test-id");
    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");

    let provider = AliyunProvider::new();
    let (avail, reason) = provider.available();

    assert!(!avail);
    let reason = reason.unwrap();
    assert!(reason.contains("ALIBABA_CLOUD_ACCESS_KEY_SECRET"));
    assert!(!reason.contains("ALIBABA_CLOUD_ACCESS_KEY_ID"));

    std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
}
