use rua_config::types::cron::ScheduleSpec;

#[test]
fn schedule_spec_defaults_to_system_timezone() {
    let json = serde_json::json!({
        "type": "cron",
        "cron": "*/5 * * * *"
    });
    let spec: ScheduleSpec = serde_json::from_value(json).unwrap();
    let expected = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    assert_eq!(spec.timezone, expected);
}

#[test]
fn schedule_spec_explicit_timezone_preserved() {
    let json = serde_json::json!({
        "type": "cron",
        "cron": "0 9 * * *",
        "timezone": "America/New_York"
    });
    let spec: ScheduleSpec = serde_json::from_value(json).unwrap();
    assert_eq!(spec.timezone, "America/New_York");
}
