use std::sync::Arc;

use async_trait::async_trait;
use cell_store::FileSystemCellStore;
use chrono::{DateTime, Utc};
use rua_config::types::cron::{CronJobSpec, ScheduleSpec};
use rua_contract::cron::{CronJob, SystemScheduler};
use rua_core::Result;
use rua_cron::CronCells;
use std::sync::Mutex;
use tempfile::TempDir;

#[derive(Default)]
struct RecordingScheduler {
    registered: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
}

#[async_trait]
impl SystemScheduler for RecordingScheduler {
    async fn register_next_run(&self, job: &CronJob, _next_run_at: DateTime<Utc>) -> Result<()> {
        self.registered.lock().unwrap().push(job.id.clone());
        Ok(())
    }
    async fn cancel(&self, job_id: &str) -> Result<()> {
        self.cancelled.lock().unwrap().push(job_id.to_string());
        Ok(())
    }
}

impl RecordingScheduler {
    fn registered_count(&self, id: &str) -> usize {
        self.registered
            .lock()
            .unwrap()
            .iter()
            .filter(|x| *x == id)
            .count()
    }
    fn cancelled_count(&self, id: &str) -> usize {
        self.cancelled
            .lock()
            .unwrap()
            .iter()
            .filter(|x| *x == id)
            .count()
    }
}

fn fixture() -> (TempDir, Arc<CronCells>, Arc<RecordingScheduler>) {
    let tmp = TempDir::new().expect("tempdir");
    let store = Arc::new(FileSystemCellStore::new());
    let scheduler = Arc::new(RecordingScheduler::default());
    let cells = Arc::new(CronCells::new(
        Arc::clone(&store),
        Arc::clone(&scheduler) as Arc<dyn SystemScheduler>,
        tmp.path().join("jobs.json"),
    ));
    (tmp, cells, scheduler)
}

fn make_spec(id: &str, paused: bool) -> CronJobSpec {
    CronJobSpec {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        paused,
        schedule: ScheduleSpec {
            r#type: "cron".to_string(),
            cron: "0 * * * *".to_string(),
            run_at: None,
            timezone: "UTC".to_string(),
            repeat_every_days: None,
            repeat_end_type: None,
            repeat_until: None,
            repeat_count: None,
            extra: Default::default(),
        },
        task_type: "agent".to_string(),
        text: Some("hello".to_string()),
        request: None,
        dispatch: None,
        save_result_to_inbox: None,
        runtime: None,
        meta: Default::default(),
        cron_expr: None,
        agent_id: Some("default".to_string()),
        prompt: None,
        missed_policy: None,
        max_retries: None,
        retry_delay_secs: None,
        extra: Default::default(),
    }
}

#[tokio::test]
async fn cell_read_returns_none_for_missing_job() {
    let (_tmp, cells, _scheduler) = fixture();
    let result = cells.cell("ghost").read().await.expect("read ok");
    assert!(result.is_none(), "missing job must read as None");
}

#[tokio::test]
async fn cell_write_persists_to_correct_json_path() {
    let (tmp, cells, _scheduler) = fixture();
    let spec = make_spec("alpha", false);
    cells
        .cell("alpha")
        .write(spec.clone())
        .await
        .expect("write ok");

    let raw = std::fs::read_to_string(tmp.path().join("jobs.json")).expect("read file");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
    assert!(
        parsed.get("jobs").and_then(|j| j.get("alpha")).is_some(),
        "spec lives under $.jobs.alpha; got {raw}"
    );
    let on_disk_id = parsed["jobs"]["alpha"]["id"]
        .as_str()
        .expect("id field present");
    assert_eq!(on_disk_id, "alpha");
}

#[tokio::test]
async fn cell_write_with_paused_false_registers_timer() {
    let (_tmp, cells, scheduler) = fixture();
    let spec = make_spec("active", false);
    cells.cell("active").write(spec).await.expect("write ok");
    assert_eq!(
        scheduler.registered_count("active"),
        1,
        "paused=false enabled=true must register exactly one timer"
    );
    assert_eq!(scheduler.cancelled_count("active"), 0);
}

#[tokio::test]
async fn cell_write_with_paused_true_cancels_timer() {
    let (_tmp, cells, scheduler) = fixture();
    let spec = make_spec("muted", true);
    cells.cell("muted").write(spec).await.expect("write ok");
    assert_eq!(
        scheduler.cancelled_count("muted"),
        1,
        "paused=true must cancel the timer (no register)"
    );
    assert_eq!(scheduler.registered_count("muted"), 0);
}

#[tokio::test]
async fn cell_update_pause_unpause_cycle() {
    let (_tmp, cells, scheduler) = fixture();

    cells
        .cell("loop")
        .write(make_spec("loop", false))
        .await
        .expect("initial write");
    assert_eq!(scheduler.registered_count("loop"), 1);

    let paused = cells
        .cell("loop")
        .update(|existing| {
            let mut spec = existing.expect("existing spec");
            spec.paused = true;
            spec
        })
        .await
        .expect("pause update");
    assert!(paused.paused);
    assert_eq!(scheduler.cancelled_count("loop"), 1);

    let resumed = cells
        .cell("loop")
        .update(|existing| {
            let mut spec = existing.expect("existing spec");
            spec.paused = false;
            spec
        })
        .await
        .expect("resume update");
    assert!(!resumed.paused);
    assert_eq!(scheduler.registered_count("loop"), 2, "resume re-registers");
}

#[tokio::test]
async fn list_returns_all_jobs_in_file() {
    let (_tmp, cells, _scheduler) = fixture();
    cells.cell("a").write(make_spec("a", false)).await.unwrap();
    cells.cell("b").write(make_spec("b", true)).await.unwrap();
    cells.cell("c").write(make_spec("c", false)).await.unwrap();

    let mut listed: Vec<String> = cells
        .list()
        .await
        .expect("list ok")
        .into_iter()
        .map(|s| s.id)
        .collect();
    listed.sort();
    assert_eq!(listed, vec!["a", "b", "c"]);
}
