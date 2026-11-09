use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct CronArgs {
    #[command(subcommand)]
    command: CronCommands,
}

#[derive(Debug, Args)]
struct CreateJobArgs {
    #[arg(short = 'f', long = "file")]
    file: Option<PathBuf>,

    #[arg(long = "type")]
    task_type: Option<String>,

    #[arg(long)]
    name: Option<String>,

    #[arg(long)]
    schedule: Option<String>,

    #[arg(long = "schedule-type")]
    schedule_type: Option<String>,

    #[arg(long = "run-at")]
    run_at: Option<String>,

    #[arg(long, default_value = "UTC")]
    timezone: String,

    #[arg(long = "repeat-every-days")]
    repeat_every_days: Option<u32>,

    #[arg(long = "repeat-end-type")]
    repeat_end_type: Option<String>,

    #[arg(long = "repeat-until")]
    repeat_until: Option<String>,

    #[arg(long = "repeat-count")]
    repeat_count: Option<u32>,

    #[arg(long)]
    text: Option<String>,

    #[arg(long, default_value = "console")]
    channel: String,

    #[arg(long)]
    target_user: Option<String>,

    #[arg(long)]
    target_session: Option<String>,

    #[arg(long, default_value = "final")]
    mode: String,

    #[arg(long = "save-result-to-inbox")]
    save_result_to_inbox: Option<bool>,

    #[arg(long = "share-session")]
    share_session: Option<bool>,

    #[arg(long = "tool-safety")]
    tool_safety: Option<bool>,

    #[arg(long = "max-concurrency")]
    max_concurrency: Option<u32>,

    #[arg(long = "timeout-seconds")]
    timeout_seconds: Option<u32>,

    #[arg(long = "misfire-grace-seconds")]
    misfire_grace_seconds: Option<u32>,
}

#[derive(Debug, Subcommand)]
enum CronCommands {
    List,

    Get {
        job_id: String,
    },

    Create(Box<CreateJobArgs>),

    Update {
        job_id: String,
        #[command(flatten)]
        args: Box<CreateJobArgs>,
    },

    Pause {
        job_id: String,
    },

    Resume {
        job_id: String,
    },

    Delete {
        job_id: String,
    },

    Run {
        job_id: String,
    },

    State {
        job_id: String,
    },
}

pub fn run(
    args: CronArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: CronArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        CronCommands::List => {
            let resp = client.get(&cron_jobs_endpoint()).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    let jobs = resp.as_array().cloned().unwrap_or_else(|| {
                        resp.get("jobs")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if jobs.is_empty() {
                        println!("No cron jobs found.");
                    } else {
                        println!("Cron Jobs:");
                        for j in &jobs {
                            let spec = j.get("spec").unwrap_or(j);
                            let id = spec.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                            let name = spec
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unnamed");
                            let enabled = spec
                                .get("enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let cron_expr = spec
                                .get("schedule")
                                .and_then(|s| s.get("cron"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let status = if enabled { "active" } else { "paused" };
                            println!("  {id}: {name} ({cron_expr}) [{status}]");
                        }
                    }
                }
            }
        }
        CronCommands::Get { job_id } => {
            let resp = client.get(&cron_job_endpoint(&job_id)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    let spec = resp.get("spec").unwrap_or(&resp);
                    let id = spec.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let name = spec
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unnamed");
                    let enabled = spec
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let cron_expr = spec
                        .get("schedule")
                        .and_then(|s| s.get("cron"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let timezone = spec
                        .get("schedule")
                        .and_then(|s| s.get("timezone"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let task_type = spec
                        .get("task_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let status = if enabled { "active" } else { "paused" };
                    println!("Cron Job: {id}");
                    println!("  Name:      {name}");
                    println!("  Schedule:  {cron_expr} ({timezone})");
                    println!("  Type:      {task_type}");
                    println!("  Status:    {status}");
                }
            }
        }
        CronCommands::Create(args) => {
            let CreateJobArgs {
                file,
                task_type,
                name,
                schedule,
                schedule_type,
                run_at,
                timezone,
                repeat_every_days,
                repeat_end_type,
                repeat_until,
                repeat_count,
                text,
                channel,
                target_user,
                target_session,
                mode,
                save_result_to_inbox,
                share_session,
                tool_safety,
                max_concurrency,
                timeout_seconds,
                misfire_grace_seconds,
            } = *args;

            let body: serde_json::Value = if let Some(path) = file {
                let content = std::fs::read_to_string(&path)?;
                serde_json::from_str(&content)?
            } else {
                let task_type = task_type.unwrap_or_else(|| "text".to_string());
                let name = name.ok_or_else(|| {
                    anyhow::anyhow!("--name is required when not using -f/--file")
                })?;
                let schedule_type = schedule_type.unwrap_or_else(|| "cron".to_string());
                let sched = match schedule_type.as_str() {
                    "cron" => {
                        let schedule = schedule.ok_or_else(|| {
                            anyhow::anyhow!("--schedule is required for --schedule-type cron")
                        })?;
                        serde_json::json!({
                            "type": "cron",
                            "cron": schedule,
                            "timezone": timezone,
                        })
                    }
                    "once" => {
                        let run_at = run_at.ok_or_else(|| {
                            anyhow::anyhow!("--run-at is required for --schedule-type once")
                        })?;
                        let mut sched = serde_json::json!({
                            "type": "once",
                            "run_at": run_at,
                            "timezone": timezone,
                        });
                        if let Some(days) = repeat_every_days {
                            sched["repeat_every_days"] = serde_json::json!(days);
                            sched["repeat_end_type"] = serde_json::json!(
                                repeat_end_type.unwrap_or_else(|| "never".into())
                            );
                            if let Some(until) = repeat_until {
                                sched["repeat_until"] = serde_json::json!(until);
                            }
                            if let Some(count) = repeat_count {
                                sched["repeat_count"] = serde_json::json!(count);
                            }
                        }
                        sched
                    }
                    other => anyhow::bail!("Unsupported schedule type: {other}"),
                };

                let dispatch = serde_json::json!({
                    "type": "channel",
                    "channel": channel,
                    "target": {
                        "user_id": target_user.as_deref().unwrap_or(""),
                        "session_id": target_session.as_deref().unwrap_or(""),
                    },
                    "mode": mode,
                    "meta": {},
                });
                let runtime = serde_json::json!({
                    "max_concurrency": max_concurrency.unwrap_or(1),
                    "timeout_seconds": timeout_seconds.unwrap_or(120),
                    "misfire_grace_seconds": misfire_grace_seconds.unwrap_or(600),
                    "share_session": share_session.unwrap_or(true),
                    "tool_safety": tool_safety.unwrap_or(false),
                });

                match task_type.as_str() {
                    "text" => {
                        serde_json::json!({
                            "name": name,
                            "enabled": true,
                            "schedule": sched,
                            "task_type": "text",
                            "text": text.unwrap_or_default(),
                            "save_result_to_inbox": save_result_to_inbox,
                            "dispatch": dispatch,
                            "runtime": runtime,
                            "meta": {},
                        })
                    }
                    "agent" => {
                        let prompt = text.ok_or_else(|| {
                            anyhow::anyhow!("--text is required when task type is 'agent'")
                        })?;
                        serde_json::json!({
                            "name": name,
                            "enabled": true,
                            "schedule": sched,
                            "task_type": "agent",
                            "request": {
                                "input": [
                                    {
                                        "role": "user",
                                        "type": "message",
                                        "content": [{"type": "text", "text": prompt}],
                                    }
                                ],
                                "session_id": target_session.as_deref().unwrap_or(""),
                                "user_id": "cron",
                            },
                            "save_result_to_inbox": save_result_to_inbox,
                            "dispatch": dispatch,
                            "runtime": runtime,
                            "meta": {},
                        })
                    }
                    other => anyhow::bail!("Unsupported task type: {other}"),
                }
            };
            let resp = client.post(&cron_jobs_endpoint(), &body).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    let id = resp
                        .get("spec")
                        .and_then(|s| s.get("id"))
                        .or_else(|| resp.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("Created cron job: {id}");
                }
            }
        }
        CronCommands::Update { job_id, args } => {
            let CreateJobArgs {
                file,
                task_type,
                name,
                schedule,
                schedule_type,
                run_at,
                timezone,
                repeat_every_days,
                repeat_end_type,
                repeat_until,
                repeat_count,
                text,
                channel,
                target_user,
                target_session,
                mode,
                save_result_to_inbox,
                share_session,
                tool_safety,
                max_concurrency,
                timeout_seconds,
                misfire_grace_seconds,
            } = *args;

            let mut body: serde_json::Value = if let Some(path) = file {
                let content = std::fs::read_to_string(&path)?;
                serde_json::from_str(&content)?
            } else {
                client.get(&cron_job_endpoint(&job_id)).await?
            };

            if let Some(name) = name {
                body["name"] = serde_json::json!(name);
            }
            if let Some(task_type) = task_type {
                body["task_type"] = serde_json::json!(task_type);
            }
            if let Some(text) = text {
                if body.get("task_type").and_then(|value| value.as_str()) == Some("agent") {
                    body["request"] = serde_json::json!({
                        "input": [
                            {
                                "role": "user",
                                "type": "message",
                                "content": [{"type": "text", "text": text}],
                            }
                        ],
                        "session_id": target_session.as_deref().unwrap_or(""),
                        "user_id": target_user.as_deref().unwrap_or("cron"),
                    });
                } else {
                    body["text"] = serde_json::json!(text);
                }
            }
            if schedule_type.is_some() || schedule.is_some() || run_at.is_some() {
                let kind = schedule_type
                    .as_deref()
                    .or_else(|| body.pointer("/schedule/type").and_then(|v| v.as_str()))
                    .unwrap_or("cron");
                body["schedule"] = match kind {
                    "cron" => serde_json::json!({
                        "type": "cron",
                        "cron": schedule.or_else(|| body.pointer("/schedule/cron").and_then(|v| v.as_str()).map(str::to_string)).unwrap_or_default(),
                        "timezone": timezone,
                    }),
                    "once" => {
                        let mut sched = serde_json::json!({
                            "type": "once",
                            "run_at": run_at.or_else(|| body.pointer("/schedule/run_at").and_then(|v| v.as_str()).map(str::to_string)).unwrap_or_default(),
                            "timezone": timezone,
                        });
                        if let Some(days) = repeat_every_days.or_else(|| {
                            body.pointer("/schedule/repeat_every_days")
                                .and_then(|v| v.as_u64())
                                .and_then(|v| u32::try_from(v).ok())
                        }) {
                            sched["repeat_every_days"] = serde_json::json!(days);
                            sched["repeat_end_type"] = serde_json::json!(repeat_end_type
                                .or_else(|| body
                                    .pointer("/schedule/repeat_end_type")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string))
                                .unwrap_or_else(|| "never".into()));
                            if let Some(until) = repeat_until.or_else(|| {
                                body.pointer("/schedule/repeat_until")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                            }) {
                                sched["repeat_until"] = serde_json::json!(until);
                            }
                            if let Some(count) = repeat_count.or_else(|| {
                                body.pointer("/schedule/repeat_count")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|v| u32::try_from(v).ok())
                            }) {
                                sched["repeat_count"] = serde_json::json!(count);
                            }
                        }
                        sched
                    }
                    other => anyhow::bail!("Unsupported schedule type: {other}"),
                };
            }
            if target_user.is_some()
                || target_session.is_some()
                || channel != "console"
                || mode != "final"
            {
                body["dispatch"] = serde_json::json!({
                    "type": "channel",
                    "channel": channel,
                    "target": {
                        "user_id": target_user.or_else(|| body.pointer("/dispatch/target/user_id").and_then(|v| v.as_str()).map(str::to_string)).unwrap_or_default(),
                        "session_id": target_session.or_else(|| body.pointer("/dispatch/target/session_id").and_then(|v| v.as_str()).map(str::to_string)).unwrap_or_default(),
                    },
                    "mode": mode,
                    "meta": body.pointer("/dispatch/meta").cloned().unwrap_or_else(|| serde_json::json!({})),
                });
            }
            if let Some(save) = save_result_to_inbox {
                body["save_result_to_inbox"] = serde_json::json!(save);
            }
            if max_concurrency.is_some()
                || timeout_seconds.is_some()
                || misfire_grace_seconds.is_some()
                || share_session.is_some()
                || tool_safety.is_some()
            {
                let mut runtime = body
                    .get("runtime")
                    .and_then(|value| value.as_object())
                    .cloned()
                    .unwrap_or_default();
                if let Some(value) = max_concurrency {
                    runtime.insert("max_concurrency".into(), serde_json::json!(value));
                }
                if let Some(value) = timeout_seconds {
                    runtime.insert("timeout_seconds".into(), serde_json::json!(value));
                }
                if let Some(value) = misfire_grace_seconds {
                    runtime.insert("misfire_grace_seconds".into(), serde_json::json!(value));
                }
                if let Some(value) = share_session {
                    runtime.insert("share_session".into(), serde_json::json!(value));
                }
                if let Some(value) = tool_safety {
                    runtime.insert("tool_safety".into(), serde_json::json!(value));
                }
                body["runtime"] = serde_json::Value::Object(runtime);
            }
            body["id"] = serde_json::json!(job_id);

            let resp = client.put(&cron_job_endpoint(&job_id), &body).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Updated cron job: {job_id}"),
            }
        }
        CronCommands::Pause { job_id } => {
            let body = serde_json::json!({"enabled": false});
            let resp = client.put(&cron_job_endpoint(&job_id), &body).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Cron job '{job_id}' paused."),
            }
        }
        CronCommands::Resume { job_id } => {
            let body = serde_json::json!({"enabled": true});
            let resp = client.put(&cron_job_endpoint(&job_id), &body).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Cron job '{job_id}' resumed."),
            }
        }
        CronCommands::Delete { job_id } => {
            let resp = client.delete(&cron_job_endpoint(&job_id)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Cron job '{job_id}' deleted."),
            }
        }
        CronCommands::Run { job_id } => {
            let body = serde_json::json!({});
            let resp = client.post(&cron_job_run_endpoint(&job_id), &body).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => println!("Cron job '{job_id}' triggered."),
            }
        }
        CronCommands::State { job_id } => {
            let resp = client.get(&cron_job_state_endpoint(&job_id)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    crate::output::print_json(&resp);
                }
            }
        }
    }

    Ok(())
}

pub fn cron_jobs_endpoint() -> String {
    "/api/cron/jobs".to_string()
}

pub fn cron_job_endpoint(job_id: &str) -> String {
    format!("/api/cron/jobs/{job_id}")
}

pub fn cron_job_run_endpoint(job_id: &str) -> String {
    format!("/api/cron/jobs/{job_id}/run")
}

pub fn cron_job_state_endpoint(job_id: &str) -> String {
    format!("/api/cron/jobs/{job_id}/state")
}
