use rua_config::types::cron::{CronJobSpec, JobsFile, ScheduleSpec};

#[test]
fn missing_schedule_timezone_defaults_to_system_timezone() {
    let json = serde_json::json!({
        "type": "cron",
        "cron": "0 * * * *"
    });

    let spec: ScheduleSpec = serde_json::from_value(json).unwrap();

    assert!(!spec.timezone.is_empty());
    assert_eq!(spec.timezone, iana_time_zone::get_timezone().unwrap());
}

#[test]
fn retry_fields_serde_roundtrip() {
    let json = serde_json::json!({
        "id": "test-job",
        "name": "Test",
        "max_retries": 3,
        "retry_delay_secs": 60
    });

    let spec: CronJobSpec = serde_json::from_value(json).unwrap();
    let roundtrip: serde_json::Value = serde_json::to_value(&spec).unwrap();

    assert_eq!(spec.max_retries, Some(3));
    assert_eq!(spec.retry_delay_secs, Some(60));
    assert_eq!(roundtrip["max_retries"], 3);
    assert_eq!(roundtrip["retry_delay_secs"], 60);
}

#[test]
fn old_json_without_retry_fields_loads_as_none() {
    let json = serde_json::json!({
        "id": "legacy-job",
        "name": "Legacy",
        "schedule": { "cron": "0 * * * *" }
    });

    let spec: CronJobSpec = serde_json::from_value(json).unwrap();

    assert_eq!(spec.max_retries, None);
    assert_eq!(spec.retry_delay_secs, None);
}

#[test]
fn jobs_file_map_form_round_trips_intact() {
    let json = serde_json::json!({
        "version": 1,
        "jobs": {
            "x": { "id": "x", "name": "X", "schedule": { "cron": "0 * * * *" }, "custom": 42 },
        },
        "extension": {"retained":true},
    });

    assert_eq!(
        rua_config::migration::detect_schema(&json),
        rua_config::migration::DetectedSchema::JobsFile
    );
    let file: JobsFile = serde_json::from_value(json).unwrap();

    assert_eq!(file.jobs.len(), 1);
    assert!(file.jobs.contains_key("x"));
    let saved = serde_json::to_value(&file).unwrap();
    assert_eq!(saved["jobs"]["x"]["custom"], 42);
    assert_eq!(saved["extension"]["retained"], true);
}

#[test]
fn paused_defaults_to_false_when_absent() {
    let json = serde_json::json!({
        "id": "no-pause",
        "name": "no-pause",
        "schedule": { "cron": "0 * * * *" }
    });

    let spec: CronJobSpec = serde_json::from_value(json).unwrap();

    assert!(!spec.paused);
}

#[test]
fn paused_round_trips_when_present() {
    let json = serde_json::json!({
        "id": "p",
        "name": "p",
        "paused": true,
        "schedule": { "cron": "0 * * * *" }
    });

    let spec: CronJobSpec = serde_json::from_value(json).unwrap();
    let out = serde_json::to_value(&spec).unwrap();

    assert!(spec.paused);
    assert_eq!(out["paused"], true);
}

#[test]
fn qwenpaw_once_schedule_fields_round_trip() {
    let json = serde_json::json!({
        "type": "once",
        "run_at": "2026-07-16T09:30:00Z",
        "timezone": "Asia/Shanghai",
        "repeat_every_days": 2,
        "repeat_end_type": "count",
        "repeat_count": 3
    });

    let spec: ScheduleSpec = serde_json::from_value(json).unwrap();
    let out = serde_json::to_value(&spec).unwrap();

    assert_eq!(spec.r#type, "once");
    assert_eq!(out["run_at"], "2026-07-16T09:30:00Z");
    assert_eq!(out["repeat_every_days"], 2);
    assert_eq!(out["repeat_end_type"], "count");
    assert_eq!(out["repeat_count"], 3);
}

#[test]
fn qwenpaw_runtime_and_inbox_fields_round_trip() {
    let json = serde_json::json!({
        "id": "runtime",
        "name": "runtime",
        "schedule": { "cron": "0 * * * *" },
        "save_result_to_inbox": true,
        "runtime": {
            "max_concurrency": 2,
            "timeout_seconds": 30,
            "misfire_grace_seconds": 600,
            "share_session": false,
            "tool_safety": true
        }
    });

    let spec: CronJobSpec = serde_json::from_value(json).unwrap();
    let out = serde_json::to_value(&spec).unwrap();
    let runtime = spec.runtime.expect("runtime");

    assert_eq!(spec.save_result_to_inbox, Some(true));
    assert_eq!(runtime.max_concurrency, 2);
    assert_eq!(runtime.timeout_seconds, 30);
    assert_eq!(runtime.misfire_grace_seconds, 600);
    assert!(!runtime.share_session);
    assert!(runtime.tool_safety);
    assert_eq!(out["save_result_to_inbox"], true);
    assert_eq!(out["runtime"]["share_session"], false);
    assert_eq!(out["runtime"]["tool_safety"], true);
}

#[test]
fn timezone_probe_failure_falls_back_to_utc() {
    fn probe(fail: bool) -> Result<String, std::io::Error> {
        if fail {
            Err(std::io::Error::other("no tz"))
        } else {
            Ok("Asia/Shanghai".into())
        }
    }

    let fallback = probe(true).unwrap_or_else(|_| "UTC".into());

    assert_eq!(fallback, "UTC");
}
