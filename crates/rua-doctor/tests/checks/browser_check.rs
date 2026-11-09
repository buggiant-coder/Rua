use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn browser_check_registered_and_runs() {
    let paths = Paths::new(Some("/tmp/rua-doctor-browser-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let browser = results
        .iter()
        .find(|r| r.name == "browser")
        .expect("browser check must be registered");

    assert!(
        matches!(browser.status, CheckStatus::Pass | CheckStatus::Warn),
        "browser check must return Pass or Warn, got {:?}",
        browser.status
    );
}

#[test]
fn browser_check_with_path_containing_chromium() {
    let paths = Paths::new(Some("/tmp/rua-doctor-browser-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    let browser = results
        .iter()
        .find(|r| r.name == "browser")
        .expect("browser check must be registered");

    match browser.status {
        CheckStatus::Pass => {
            assert!(
                browser.message.starts_with("found:"),
                "Pass message should list found browsers"
            );
        }
        CheckStatus::Warn => {
            assert_eq!(browser.message, "no browser found in PATH");
        }
        CheckStatus::Fail => panic!("browser check should never Fail"),
    }
}
