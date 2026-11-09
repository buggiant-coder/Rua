use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rua_config::types::cron::{CronJobSpec, DispatchSpec, DispatchTarget, ScheduleSpec};
use rua_contract::cron::SystemScheduler;
use rua_cron::executor::MockChannelSender;
use rua_cron::{
    compute_next_run, spec_to_cron_job, CronExecutor, JsonCronJobRepository, TokioScheduler,
};
use tempfile::TempDir;

struct NoOpAgentRunner;

#[async_trait::async_trait]
impl rua_cron::CronAgentRunner for NoOpAgentRunner {
    async fn run_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<String, rua_core::RuaError> {
        Ok("noop".to_string())
    }
}

fn make_text_job(id: &str, text: &str) -> CronJobSpec {
    CronJobSpec {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        paused: false,
        schedule: ScheduleSpec {
            r#type: "cron".to_string(),
            cron: "* * * * *".to_string(),
            run_at: None,
            timezone: "UTC".to_string(),
            repeat_every_days: None,
            repeat_end_type: None,
            repeat_until: None,
            repeat_count: None,
            extra: Default::default(),
        },
        task_type: "text".to_string(),
        text: Some(text.to_string()),
        request: None,
        save_result_to_inbox: None,
        dispatch: Some(DispatchSpec {
            r#type: "channel".to_string(),
            channel: "test-channel".to_string(),
            target: Some(DispatchTarget {
                user_id: "test-user".to_string(),
                session_id: "test-session".to_string(),
                extra: Default::default(),
            }),
            mode: "text".to_string(),
            meta: Default::default(),
            extra: Default::default(),
        }),
        runtime: None,
        meta: Default::default(),
        cron_expr: None,
        agent_id: None,
        prompt: None,
        missed_policy: None,
        max_retries: None,
        retry_delay_secs: None,
        extra: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_scheduler_picks_up_job_and_sends_text() {
    let tmp = TempDir::new().expect("tempdir");
    let ws_dir = tmp.path().join("workspaces").join("test_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let jobs_path = ws_dir.join("jobs.json");
    let repo = JsonCronJobRepository::new(&jobs_path);
    let job = make_text_job("job-1", "Hello from cron!");
    repo.save_jobs(vec![job.clone()]).unwrap();

    let sender = Arc::new(MockChannelSender::new());
    let runner: Arc<dyn rua_cron::CronAgentRunner> = Arc::new(NoOpAgentRunner);
    let executor = Arc::new(CronExecutor::new(sender.clone(), runner));

    let (fire_tx, mut fire_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let scheduler = Arc::new(TokioScheduler::new(Arc::new(move |job_id: String| {
        let _ = fire_tx.send(job_id);
    })));

    let cron_job = spec_to_cron_job(&job);
    scheduler
        .register_next_run(&cron_job, Utc::now())
        .await
        .expect("register ok");

    let fired_id = tokio::time::timeout(Duration::from_secs(5), fire_rx.recv())
        .await
        .expect("fire timeout")
        .expect("fire channel closed");
    assert_eq!(fired_id, "job-1");

    let result = executor.execute(&job).await;
    assert!(result.success, "Job execution failed: {:?}", result.error);

    let messages = sender.sent_messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, "test-channel");
    assert_eq!(messages[0].1, "test-user");
    assert_eq!(messages[0].2, "test-session");
    assert_eq!(messages[0].3, "Hello from cron!");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_reload_detects_new_and_deleted_jobs() {
    let tmp = TempDir::new().expect("tempdir");
    let ws_dir = tmp.path().join("workspaces").join("test_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let jobs_path = ws_dir.join("jobs.json");
    let repo = JsonCronJobRepository::new(&jobs_path);

    repo.save_jobs(vec![]).unwrap();
    let initial = repo.load_jobs().unwrap();
    assert!(initial.is_empty());

    let job_a = make_text_job("job-a", "A");
    let job_b = make_text_job("job-b", "B");
    repo.save_jobs(vec![job_a, job_b]).unwrap();

    let loaded = repo.load_jobs().unwrap();
    assert_eq!(loaded.len(), 2);

    let job_a_only = make_text_job("job-a", "A");
    repo.save_jobs(vec![job_a_only]).unwrap();

    let after_delete = repo.load_jobs().unwrap();
    assert_eq!(after_delete.len(), 1);
    assert_eq!(after_delete[0].id, "job-a");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_file_watcher_triggers_on_jobs_json_change() {
    use notify::{RecursiveMode, Watcher};

    let tmp = TempDir::new().expect("tempdir");
    let ws_dir = tmp.path().join("workspaces").join("test_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(4);

    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let is_json = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "json"));
                if is_json {
                    let _ = tx.try_send(());
                }
            }
        })
        .expect("watcher");

    watcher
        .watch(
            tmp.path().join("workspaces").as_ref(),
            RecursiveMode::Recursive,
        )
        .expect("watch");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let jobs_path = ws_dir.join("jobs.json");
    let repo = JsonCronJobRepository::new(&jobs_path);
    let job = make_text_job("watcher-job", "Watch me!");
    repo.save_jobs(vec![job]).unwrap();

    let got_event = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(
        got_event.is_ok(),
        "File watcher should detect jobs.json change within 5s"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_compute_next_run_returns_future_time() {
    let job = make_text_job("schedule-test", "Test");
    let next = compute_next_run(&job.schedule.cron, &job.schedule.timezone);
    assert!(
        next.is_some(),
        "compute_next_run should return Some for '* * * * *'"
    );
    let next_time = next.unwrap();
    assert!(
        next_time > Utc::now() - chrono::Duration::seconds(2),
        "Next run should be close to now"
    );
}
