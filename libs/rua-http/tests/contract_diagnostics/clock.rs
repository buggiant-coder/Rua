use rua_http::diagnostics::{suspend_inclusive_time, SOCKET_COUNTERS_SUPPORTED};

#[test]
fn native_clock_availability_is_explicit() {
    assert_eq!(SOCKET_COUNTERS_SUPPORTED, cfg!(target_os = "macos"));
    if cfg!(target_os = "macos") {
        let before = suspend_inclusive_time().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(suspend_inclusive_time().unwrap() - before >= std::time::Duration::from_millis(10));
    } else {
        assert_eq!(suspend_inclusive_time(), None);
        let mut sampler = rua_http::diagnostics::SocketSampler::default();
        let reading = sampler.sample(
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
        );
        assert_eq!(reading.status, "unsupported_platform");
        assert!(reading.counters.is_none());
    }
}
