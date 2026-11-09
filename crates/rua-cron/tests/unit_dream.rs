use rua_config::types::agents::AgentsRunningConfig;

#[test]
fn dream_job_spec_uses_running_memory_summary_cron() {
    let mut running = AgentsRunningConfig::default();
    running.memory_summary.memory_summary_enabled = true;
    running.memory_summary.dream_cron = "0 23 * * *".into();

    let job =
        rua_cron::build_dream_job_spec("agent-a", &running, "Asia/Shanghai").expect("dream job");

    assert_eq!(job.id, "_dream:agent-a");
    assert_eq!(job.name, "Dream Memory");
    assert_eq!(job.task_type, "dream");
    assert_eq!(job.agent_id.as_deref(), Some("agent-a"));
    assert_eq!(job.schedule.cron, "0 23 * * *");
    assert_eq!(job.schedule.timezone, "Asia/Shanghai");
    assert_eq!(job.meta["internal"], "dream");
}

#[test]
fn dream_job_spec_respects_disabled_and_empty_cron() {
    let mut running = AgentsRunningConfig::default();
    running.memory_summary.memory_summary_enabled = false;
    assert!(rua_cron::build_dream_job_spec("agent-a", &running, "UTC").is_none());

    running.memory_summary.memory_summary_enabled = true;
    running.memory_summary.dream_cron.clear();
    assert!(rua_cron::build_dream_job_spec("agent-a", &running, "UTC").is_none());
}
