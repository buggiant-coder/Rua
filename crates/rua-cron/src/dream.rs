use rua_config::types::agents::AgentsRunningConfig;
use rua_config::types::cron::{CronJobSpec, JobRuntimeSpec, ScheduleSpec};

use crate::normalize::normalize_cron_expr;

pub const DREAM_JOB_ID_PREFIX: &str = "_dream:";
const DEFAULT_DREAM_CRON: &str = "0 23 * * *";

pub fn dream_job_id(agent_id: &str) -> String {
    format!("{DREAM_JOB_ID_PREFIX}{agent_id}")
}

pub fn build_dream_job_spec(
    agent_id: &str,
    running: &AgentsRunningConfig,
    timezone: &str,
) -> Option<CronJobSpec> {
    let cfg = &running.memory_summary;
    if !cfg.memory_summary_enabled || cfg.dream_cron.trim().is_empty() {
        return None;
    }

    let cron = normalize_cron_expr(&cfg.dream_cron)
        .or_else(|_| normalize_cron_expr(DEFAULT_DREAM_CRON))
        .ok()?;
    let mut meta = serde_json::Map::new();
    meta.insert("internal".into(), serde_json::Value::String("dream".into()));

    Some(CronJobSpec {
        id: dream_job_id(agent_id),
        name: "Dream Memory".to_string(),
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
        task_type: "dream".to_string(),
        text: None,
        request: None,
        dispatch: None,
        save_result_to_inbox: Some(false),
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
        prompt: None,
        missed_policy: Some("run_once".to_string()),
        max_retries: None,
        retry_delay_secs: None,
        extra: Default::default(),
    })
}
