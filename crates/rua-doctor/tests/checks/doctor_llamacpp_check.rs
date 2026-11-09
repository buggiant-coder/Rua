use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn llamacpp_installation_check_registered_and_runs() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llamacpp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let check = results
        .iter()
        .find(|r| r.name == "llamacpp_installation")
        .expect("llamacpp_installation check must be registered");

    assert!(
        matches!(check.status, CheckStatus::Pass | CheckStatus::Warn),
        "llamacpp_installation must return Pass or Warn, got {:?}",
        check.status
    );
}

#[test]
fn llamacpp_installation_check_never_fails() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llamacpp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let check = results
        .iter()
        .find(|r| r.name == "llamacpp_installation")
        .expect("llamacpp_installation check must be registered");

    match check.status {
        CheckStatus::Pass => {
            assert!(
                !check.message.is_empty(),
                "Pass message should contain path and version info"
            );
        }
        CheckStatus::Warn => {
            assert!(
                check.message.contains("not found"),
                "Warn message should indicate binary not found"
            );
        }
        CheckStatus::Fail => panic!("llamacpp_installation should never Fail (severity=Warning)"),
    }
}

#[test]
fn llamacpp_server_status_check_registered_and_runs() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llamacpp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let check = results
        .iter()
        .find(|r| r.name == "llamacpp_server_status")
        .expect("llamacpp_server_status check must be registered");

    assert!(
        matches!(check.status, CheckStatus::Pass | CheckStatus::Warn),
        "llamacpp_server_status must return Pass or Warn, got {:?}",
        check.status
    );
}

#[test]
fn llamacpp_server_status_check_never_fails() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llamacpp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let check = results
        .iter()
        .find(|r| r.name == "llamacpp_server_status")
        .expect("llamacpp_server_status check must be registered");

    match check.status {
        CheckStatus::Pass => {
            assert!(
                check.message.contains("running"),
                "Pass message should indicate server is running"
            );
        }
        CheckStatus::Warn => {
            assert!(
                check.message.contains("not running"),
                "Warn message should indicate server not running"
            );
        }
        CheckStatus::Fail => {
            panic!("llamacpp_server_status should never Fail (severity=Warning)")
        }
    }
}

#[test]
fn llamacpp_installation_details_structure() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llamacpp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let check = results
        .iter()
        .find(|r| r.name == "llamacpp_installation")
        .expect("llamacpp_installation check must be registered");

    let details = check.details.as_ref().expect("details should be present");
    assert!(
        details.get("installed").is_some(),
        "details must have 'installed' field"
    );
}
