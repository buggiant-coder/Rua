use chrono::NaiveTime;
use tempfile::TempDir;

use rua_config::types::agents::{ActiveHoursConfig, HeartbeatConfig};
use rua_cron::heartbeat::{
    build_heartbeat_job_spec, interval_to_cron, load_heartbeat_md, strip_frontmatter,
};
use rua_cron::{parse_active_hours, parse_interval};

#[test]
fn interval_parser_accepts_qwenpaw_style_human_durations() {
    assert_eq!(parse_interval("30m"), Some(30 * 60));
    assert_eq!(parse_interval("1h"), Some(60 * 60));
    assert_eq!(parse_interval("2h30m"), Some(2 * 60 * 60 + 30 * 60));
    assert_eq!(parse_interval("1h30m45s"), Some(60 * 60 + 30 * 60 + 45));
    assert_eq!(parse_interval("90s"), Some(90));
}

#[test]
fn interval_parser_rejects_ambiguous_or_empty_values() {
    assert_eq!(parse_interval(""), None);
    assert_eq!(parse_interval("15"), None);
    assert_eq!(parse_interval("m"), None);
    assert_eq!(parse_interval("0m"), None);
    assert_eq!(parse_interval("1d"), None);
    assert_eq!(parse_interval("1h30"), None);
}

#[test]
fn interval_to_cron_uses_rust_normalized_scheduler_shape() {
    assert_eq!(interval_to_cron(30), "* * * * *");
    assert_eq!(interval_to_cron(15 * 60), "*/15 * * * *");
    assert_eq!(interval_to_cron(60 * 60), "0 */1 * * *");
    assert_eq!(interval_to_cron(2 * 60 * 60), "0 */2 * * *");
    assert_eq!(interval_to_cron(2 * 60 * 60 + 30 * 60), "30 */2 * * *");
}

#[test]
fn active_hours_parse_and_match_normal_and_cross_midnight_windows() {
    let daytime = parse_active_hours("09:00-17:30").expect("daytime window");
    assert!(daytime.contains(NaiveTime::from_hms_opt(9, 0, 0).unwrap()));
    assert!(daytime.contains(NaiveTime::from_hms_opt(12, 30, 0).unwrap()));
    assert!(!daytime.contains(NaiveTime::from_hms_opt(17, 30, 0).unwrap()));
    assert!(!daytime.contains(NaiveTime::from_hms_opt(8, 59, 0).unwrap()));

    let overnight = parse_active_hours("22:00-06:00").expect("overnight window");
    assert!(overnight.contains(NaiveTime::from_hms_opt(22, 0, 0).unwrap()));
    assert!(overnight.contains(NaiveTime::from_hms_opt(23, 59, 0).unwrap()));
    assert!(overnight.contains(NaiveTime::from_hms_opt(5, 59, 0).unwrap()));
    assert!(!overnight.contains(NaiveTime::from_hms_opt(6, 0, 0).unwrap()));
    assert!(!overnight.contains(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
}

#[test]
fn active_hours_rejects_invalid_shapes() {
    assert!(parse_active_hours("").is_none());
    assert!(parse_active_hours("09:00").is_none());
    assert!(parse_active_hours("09:00-17:00-18:00").is_none());
    assert!(parse_active_hours("24:00-01:00").is_none());
    assert!(parse_active_hours("09:xx-17:00").is_none());
}

#[test]
fn heartbeat_md_loader_strips_frontmatter_for_execution_prompt() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("HEARTBEAT.md"),
        "---\ntitle: ignored\n---\n\nRun status check\n",
    )
    .expect("write heartbeat");

    let prompt = load_heartbeat_md(tmp.path()).expect("heartbeat prompt");
    assert_eq!(prompt, "Run status check\n");
}

#[test]
fn heartbeat_md_loader_skips_missing_empty_and_comment_only_files() {
    let tmp = TempDir::new().expect("tempdir");
    assert!(load_heartbeat_md(tmp.path()).is_none());

    std::fs::write(tmp.path().join("HEARTBEAT.md"), "\n  \n").expect("write empty");
    assert!(load_heartbeat_md(tmp.path()).is_none());

    std::fs::write(tmp.path().join("HEARTBEAT.md"), "# only a comment\n\n")
        .expect("write comments");
    assert!(load_heartbeat_md(tmp.path()).is_none());
}

#[test]
fn strip_frontmatter_leaves_unclosed_frontmatter_unchanged() {
    let raw = "---\ntitle: missing end\nbody";
    assert_eq!(strip_frontmatter(raw), raw);
}

#[test]
fn heartbeat_job_spec_uses_persisted_config_and_last_dispatch() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("HEARTBEAT.md"), "Run status check\n").expect("write heartbeat");

    let cfg = HeartbeatConfig {
        enabled: true,
        every: "30m".into(),
        target: "last".into(),
        active_hours: Some(ActiveHoursConfig {
            start: "09:00".into(),
            end: "17:00".into(),
            extra: Default::default(),
        }),
        extra: Default::default(),
    };
    let last = serde_json::json!({
        "channel": "dingtalk",
        "user_id": "u1",
        "session_id": "s1"
    });

    let job = build_heartbeat_job_spec("agent-a", &cfg, tmp.path(), "Asia/Shanghai", Some(&last))
        .expect("heartbeat job");

    assert_eq!(job.id, "_heartbeat:agent-a");
    assert_eq!(job.agent_id.as_deref(), Some("agent-a"));
    assert_eq!(job.schedule.cron, "*/30 * * * *");
    assert_eq!(job.schedule.timezone, "Asia/Shanghai");
    assert_eq!(job.prompt.as_deref(), Some("Run status check\n"));
    let dispatch = job.dispatch.expect("dispatch");
    assert_eq!(dispatch.channel, "dingtalk");
    let target = dispatch.target.expect("target");
    assert_eq!(target.user_id, "u1");
    assert_eq!(target.session_id, "s1");
    assert_eq!(job.meta["internal"], "heartbeat");
    assert_eq!(job.meta["active_hours"]["start"], "09:00");
}

#[test]
fn heartbeat_job_spec_skips_disabled_or_missing_prompt() {
    let tmp = TempDir::new().expect("tempdir");
    let mut cfg = HeartbeatConfig {
        enabled: false,
        every: "30m".into(),
        target: "main".into(),
        active_hours: None,
        extra: Default::default(),
    };
    assert!(build_heartbeat_job_spec("agent-a", &cfg, tmp.path(), "UTC", None).is_none());

    cfg.enabled = true;
    assert!(build_heartbeat_job_spec("agent-a", &cfg, tmp.path(), "UTC", None).is_none());
}
