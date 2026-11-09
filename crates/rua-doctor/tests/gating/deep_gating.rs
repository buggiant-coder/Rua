use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks, run_all_checks_filtered, CheckContext};

#[test]
fn deep_false_skips_network_checks() {
    let paths = Paths::new(Some("/tmp/rua-doctor-deep-test"));
    let ctx = CheckContext { paths: &paths };

    let (results, skipped) = run_all_checks_filtered(&ctx, false);

    assert!(
        !results.iter().any(|r| r.name == "connectivity"),
        "connectivity check must be skipped when deep=false"
    );
    assert!(skipped > 0, "at least one network check should be skipped");
}

#[test]
fn deep_true_includes_network_checks() {
    let paths = Paths::new(Some("/tmp/rua-doctor-deep-test"));
    let ctx = CheckContext { paths: &paths };

    let (results, skipped) = run_all_checks_filtered(&ctx, true);

    assert!(
        results.iter().any(|r| r.name == "connectivity"),
        "connectivity check must run when deep=true"
    );
    assert_eq!(skipped, 0, "no checks should be skipped when deep=true");
}

#[test]
fn run_all_checks_includes_everything() {
    let paths = Paths::new(Some("/tmp/rua-doctor-deep-test"));
    let ctx = CheckContext { paths: &paths };

    let results = run_all_checks(&ctx);

    assert!(
        results.iter().any(|r| r.name == "connectivity"),
        "run_all_checks (unfiltered) must include connectivity"
    );
}
