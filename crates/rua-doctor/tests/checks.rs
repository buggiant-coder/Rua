#[path = "checks/agent_json_schema.rs"]
mod agent_json_schema;
#[path = "checks/browser_check.rs"]
mod browser_check;
#[path = "checks/cron_jobs_check.rs"]
mod cron_jobs_check;
#[path = "checks/disk_check.rs"]
mod disk_check;
#[path = "checks/doctor_llamacpp_check.rs"]
mod doctor_llamacpp_check;
#[path = "checks/doctor_log_writable.rs"]
mod doctor_log_writable;
#[path = "checks/doctor_provider_http_probe.rs"]
mod doctor_provider_http_probe;
#[path = "checks/doctor_workspace_writable.rs"]
mod doctor_workspace_writable;
#[path = "checks/llm_connectivity_check.rs"]
mod llm_connectivity_check;
#[path = "checks/voice_connectivity.rs"]
mod voice_connectivity;
