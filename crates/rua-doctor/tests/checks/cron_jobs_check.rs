use rua_config::dirs::Paths;
use rua_doctor::cron_jobs::validate_jobs_file;
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};

#[test]
fn cron_jobs_check_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-cron-reg"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    assert!(
        results.iter().any(|r| r.name == "cron_jobs"),
        "cron_jobs check must appear in run_all_checks"
    );
}

#[test]
fn valid_jobs_file_pass() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-valid");
    let _ = std::fs::create_dir_all(&tmp);

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "job-1": {
                "id": "job-1",
                "name": "Morning Report",
                "enabled": true,
                "schedule": {
                    "type": "cron",
                    "cron": "0 9 * * *",
                    "timezone": "Asia/Shanghai"
                },
                "task_type": "text",
                "text": "hello"
            }
        }
    });
    std::fs::write(
        tmp.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let diag = validate_jobs_file("root", &tmp.join("jobs.json"));
    assert!(diag.exists);
    assert!(diag.parseable);
    assert_eq!(diag.jobs.len(), 1);
    assert!(
        diag.jobs[0].errors.is_empty(),
        "valid job should have no errors"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn missing_required_fields_error() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-missing");
    let _ = std::fs::create_dir_all(&tmp);

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "invalid": {
                "id": "",
                "name": "",
                "schedule": {
                    "cron": "",
                    "timezone": ""
                }
            }
        }
    });
    std::fs::write(
        tmp.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let diag = validate_jobs_file("root", &tmp.join("jobs.json"));
    assert!(diag.parseable);
    assert_eq!(diag.jobs.len(), 1);
    let errors = &diag.jobs[0].errors;
    assert!(
        errors.iter().any(|e| e.contains("'id'")),
        "should report missing id"
    );
    assert!(
        errors.iter().any(|e| e.contains("'name'")),
        "should report missing name"
    );
    assert!(
        errors.iter().any(|e| e.contains("'schedule.cron'")),
        "should report missing schedule.cron"
    );
    assert!(
        errors.iter().any(|e| e.contains("'schedule.timezone'")),
        "should report missing schedule.timezone"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn invalid_timezone_error() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-badtz");
    let _ = std::fs::create_dir_all(&tmp);

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "job-tz": {
                "id": "job-tz",
                "name": "Bad TZ",
                "schedule": {
                    "cron": "0 9 * * *",
                    "timezone": "Mars/Olympus"
                }
            }
        }
    });
    std::fs::write(
        tmp.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let diag = validate_jobs_file("root", &tmp.join("jobs.json"));
    assert!(diag.parseable);
    let errors = &diag.jobs[0].errors;
    assert!(
        errors.iter().any(|e| e.contains("invalid timezone")),
        "should report invalid timezone: {:?}",
        errors
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unparseable_json_error() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-badjson");
    let _ = std::fs::create_dir_all(&tmp);

    std::fs::write(tmp.join("jobs.json"), "{ not valid json }").unwrap();

    let diag = validate_jobs_file("root", &tmp.join("jobs.json"));
    assert!(diag.exists);
    assert!(!diag.parseable);
    assert!(diag.parse_error.is_some());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn nonexistent_file_not_exists() {
    let diag = validate_jobs_file("root", &std::path::PathBuf::from("/tmp/no-such-jobs.json"));
    assert!(!diag.exists);
    assert!(!diag.parseable);
}

#[test]
fn full_check_with_jobs_file() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-full");
    let _ = std::fs::create_dir_all(&tmp);

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "j1": {
                "id": "j1",
                "name": "Good Job",
                "schedule": { "cron": "0 9 * * *", "timezone": "UTC" },
                "task_type": "text",
                "text": "hi"
            }
        }
    });
    std::fs::write(
        tmp.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let r = results.iter().find(|r| r.name == "cron_jobs").unwrap();
    assert_eq!(
        r.status,
        CheckStatus::Pass,
        "valid jobs → Pass: {}",
        r.message
    );
    assert!(r.message.contains("valid"), "message: {}", r.message);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn full_check_with_bad_job_fail() {
    let tmp = std::env::temp_dir().join("rua-doctor-cron-full-bad");
    let _ = std::fs::create_dir_all(&tmp);

    let jobs = serde_json::json!({
        "version": 1,
        "jobs": {
            "invalid": {
                "id": "",
                "name": "",
                "schedule": { "cron": "", "timezone": "Invalid/Zone" }
            }
        }
    });
    std::fs::write(
        tmp.join("jobs.json"),
        serde_json::to_string_pretty(&jobs).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    let r = results.iter().find(|r| r.name == "cron_jobs").unwrap();
    assert_eq!(
        r.status,
        CheckStatus::Fail,
        "bad jobs → Fail: {}",
        r.message
    );
    assert!(r.message.contains("error"), "message: {}", r.message);

    let _ = std::fs::remove_dir_all(&tmp);
}
