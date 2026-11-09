use chrono::{NaiveTime, Utc};
use rua_config::types::agents::HeartbeatConfig as AgentHeartbeatConfig;
use rua_config::types::cron::{
    CronJobSpec, DispatchSpec, DispatchTarget, JobRuntimeSpec, ScheduleSpec,
};
use std::path::Path;
use tracing;

use crate::normalize::normalize_cron_expr;

#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    pub cron_expr: String,

    pub active_hours: Option<ActiveHoursRange>,

    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct ActiveHoursRange {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl ActiveHoursRange {
    pub fn contains(&self, time: NaiveTime) -> bool {
        if self.start <= self.end {
            time >= self.start && time < self.end
        } else {
            time >= self.start || time < self.end
        }
    }
}

pub fn parse_interval(input: &str) -> Option<u64> {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return None;
    }

    let mut total_seconds: u64 = 0;
    let mut current_num = String::new();
    let mut found_any = false;

    for ch in input.chars() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
        } else {
            match ch {
                'h' => {
                    let n: u64 = current_num.parse().ok()?;
                    total_seconds += n * 3600;
                    current_num.clear();
                    found_any = true;
                }
                'm' => {
                    let n: u64 = current_num.parse().ok()?;
                    total_seconds += n * 60;
                    current_num.clear();
                    found_any = true;
                }
                's' => {
                    let n: u64 = current_num.parse().ok()?;
                    total_seconds += n;
                    current_num.clear();
                    found_any = true;
                }
                _ => return None,
            }
        }
    }

    if !current_num.is_empty() {
        return None;
    }

    if found_any && total_seconds > 0 {
        Some(total_seconds)
    } else {
        None
    }
}

pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 1800;

pub const HEARTBEAT_JOB_ID_PREFIX: &str = "_heartbeat:";

pub fn heartbeat_job_id(agent_id: &str) -> String {
    format!("{HEARTBEAT_JOB_ID_PREFIX}{agent_id}")
}

pub fn interval_to_cron(seconds: u64) -> String {
    if seconds < 60 {
        "* * * * *".to_string()
    } else if seconds < 3600 {
        let minutes = seconds / 60;
        format!("*/{minutes} * * * *")
    } else {
        let hours = seconds / 3600;
        let remaining_minutes = (seconds % 3600) / 60;
        if remaining_minutes == 0 {
            format!("0 */{hours} * * *")
        } else {
            format!("{remaining_minutes} */{hours} * * *")
        }
    }
}

pub fn parse_active_hours(s: &str) -> Option<ActiveHoursRange> {
    let s = s.trim();
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start = parse_time(parts[0].trim())?;
    let end = parse_time(parts[1].trim())?;

    Some(ActiveHoursRange { start, end })
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: u32 = parts[0].parse().ok()?;
    let min: u32 = parts[1].parse().ok()?;
    NaiveTime::from_hms_opt(hour, min, 0)
}

pub fn load_heartbeat_md(workspace_dir: &Path) -> Option<String> {
    let path = workspace_dir.join("HEARTBEAT.md");
    let content = std::fs::read_to_string(&path).ok()?;

    let content = strip_frontmatter(&content);

    let meaningful_lines: Vec<&str> = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .collect();

    if meaningful_lines.is_empty() {
        tracing::debug!(
            "HEARTBEAT.md at {:?} is empty or comments-only, skipping",
            path
        );
        return None;
    }

    Some(content.to_string())
}

pub fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }

    if let Some(end_idx) = trimmed[3..].find("---") {
        let after_frontmatter = &trimmed[3 + end_idx + 3..];
        after_frontmatter
            .trim_start_matches('\n')
            .trim_start_matches('\r')
    } else {
        content
    }
}

pub fn is_within_active_hours(active_hours: Option<&ActiveHoursRange>, timezone: &str) -> bool {
    let range = match active_hours {
        Some(r) => r,
        None => return true,
    };

    let now = if let Ok(tz) = timezone.parse::<chrono_tz::Tz>() {
        Utc::now().with_timezone(&tz).time()
    } else {
        tracing::warn!("Invalid timezone '{}', using UTC", timezone);
        Utc::now().time()
    };

    range.contains(now)
}

pub fn heartbeat_cron_expr(every: &str) -> String {
    if let Some(seconds) = parse_interval(every) {
        return interval_to_cron(seconds);
    }

    normalize_cron_expr(every).unwrap_or_else(|_| interval_to_cron(DEFAULT_HEARTBEAT_INTERVAL_SECS))
}

pub fn build_heartbeat_job_spec(
    agent_id: &str,
    config: &AgentHeartbeatConfig,
    workspace_dir: &Path,
    timezone: &str,
    last_dispatch: Option<&serde_json::Value>,
) -> Option<CronJobSpec> {
    if !config.enabled {
        return None;
    }

    let prompt = load_heartbeat_md(workspace_dir)?;
    let cron = heartbeat_cron_expr(&config.every);
    let (channel, user_id, session_id) = heartbeat_dispatch(&config.target, last_dispatch);
    let mut meta = serde_json::Map::new();
    meta.insert(
        "internal".into(),
        serde_json::Value::String("heartbeat".into()),
    );
    if let Some(active_hours) = &config.active_hours {
        meta.insert(
            "active_hours".into(),
            serde_json::json!({
                "start": active_hours.start,
                "end": active_hours.end,
            }),
        );
    }

    Some(CronJobSpec {
        id: heartbeat_job_id(agent_id),
        name: "Heartbeat".to_string(),
        enabled: true,
        paused: false,
        schedule: ScheduleSpec {
            r#type: "cron".to_string(),
            cron,
            run_at: None,
            timezone: timezone.to_string(),
            repeat_every_days: None,
            repeat_end_type: None,
            repeat_until: None,
            repeat_count: None,
            extra: Default::default(),
        },
        task_type: "agent".to_string(),
        text: None,
        request: None,
        save_result_to_inbox: Some(true),
        dispatch: Some(DispatchSpec {
            r#type: "channel".to_string(),
            channel,
            target: Some(DispatchTarget {
                user_id,
                session_id,
                extra: Default::default(),
            }),
            mode: "stream".to_string(),
            meta: Default::default(),
            extra: Default::default(),
        }),
        runtime: Some(JobRuntimeSpec {
            max_concurrency: 1,
            timeout_seconds: 300,
            misfire_grace_seconds: 3600,
            share_session: true,
            tool_safety: false,
            extra: Default::default(),
        }),
        meta,
        cron_expr: None,
        agent_id: Some(agent_id.to_string()),
        prompt: Some(prompt),
        missed_policy: Some("run_once".to_string()),
        max_retries: None,
        retry_delay_secs: None,
        extra: Default::default(),
    })
}

pub fn should_run_heartbeat_job(job: &CronJobSpec) -> bool {
    if job.meta.get("internal").and_then(|v| v.as_str()) != Some("heartbeat") {
        return true;
    }
    let active_hours = job
        .meta
        .get("active_hours")
        .and_then(active_hours_from_value);
    is_within_active_hours(active_hours.as_ref(), &job.schedule.timezone)
}

fn active_hours_from_value(value: &serde_json::Value) -> Option<ActiveHoursRange> {
    let start = value.get("start")?.as_str()?;
    let end = value.get("end")?.as_str()?;
    parse_active_hours(&format!("{start}-{end}"))
}

fn heartbeat_dispatch(
    target: &str,
    last_dispatch: Option<&serde_json::Value>,
) -> (String, String, String) {
    let target = target.trim();
    if target.eq_ignore_ascii_case("last") {
        if let Some(dispatch) = dispatch_from_last(last_dispatch) {
            return dispatch;
        }
    }

    if target.is_empty() || target.eq_ignore_ascii_case("main") {
        return ("console".into(), "main".into(), "main".into());
    }

    (target.to_string(), "main".into(), "main".into())
}

fn dispatch_from_last(value: Option<&serde_json::Value>) -> Option<(String, String, String)> {
    let value = value?;
    let channel = value.get("channel")?.as_str()?.trim();
    if channel.is_empty() {
        return None;
    }
    let user_id = value
        .get("user_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("main");
    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("main");
    Some((
        channel.to_string(),
        user_id.to_string(),
        session_id.to_string(),
    ))
}
