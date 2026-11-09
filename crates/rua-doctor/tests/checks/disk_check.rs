use rua_config::dirs::Paths;
use rua_doctor::disk::status_from_free;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn disk_check_registered_and_runs() {
    let paths = Paths::new(Some("/tmp/rua-doctor-disk-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let disk = results.iter().find(|r| r.name == "disk");
    assert!(disk.is_some(), "disk check must appear in run_all_checks");
    assert!(
        disk.unwrap().message.contains("disk free"),
        "disk check message must contain 'disk free'"
    );
}

#[test]
fn secret_dir_same_volume_no_duplicate() {
    let paths = Paths::new(Some("/tmp/rua-doctor-disk-same-vol"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let disk = results.iter().find(|r| r.name == "disk").unwrap();
    let count = disk.message.matches("disk free").count();
    assert_eq!(
        count, 1,
        "same-volume working_dir and secret_dir must produce only one entry"
    );
}

#[test]
fn mock_5gib_pass() {
    assert_eq!(status_from_free(5 * 1024 * 1024 * 1024), CheckStatus::Pass);
}

#[test]
fn mock_300mib_warn() {
    assert_eq!(status_from_free(300 * 1024 * 1024), CheckStatus::Warn);
}

#[test]
fn mock_50mib_fail() {
    assert_eq!(status_from_free(50 * 1024 * 1024), CheckStatus::Fail);
}

#[test]
fn threshold_boundary_500mib_is_pass() {
    assert_eq!(status_from_free(500 * 1024 * 1024), CheckStatus::Pass);
}

#[test]
fn threshold_boundary_100mib_is_warn() {
    assert_eq!(status_from_free(100 * 1024 * 1024), CheckStatus::Warn);
}
