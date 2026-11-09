use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use rua_config::types::cron::{
    CronJobSpec, CronJobState, DispatchSpec, DispatchTarget, JobRuntimeSpec,
};
use rua_core::event::RuntimeEvent;
use rua_core::RuaError;
use rua_cron::executor::{
    CronAgentRunner, CronChannelSender, CronExecutor, FailNTimesAgentRunner, MockAgentRunner,
    MockChannelSender, RuntimeEventStream,
};
use rua_cron::{CronRunner, RunJobStatus};

fn make_agent_job(
    id: &str,
    max_retries: Option<u32>,
    retry_delay_secs: Option<u64>,
) -> CronJobSpec {
    CronJobSpec {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        paused: false,
        schedule: Default::default(),
        task_type: "agent".to_string(),
        text: Some("test prompt".to_string()),
        request: None,
        dispatch: None,
        save_result_to_inbox: None,
        runtime: None,
        meta: Default::default(),
        cron_expr: None,
        agent_id: None,
        prompt: None,
        missed_policy: None,
        max_retries,
        retry_delay_secs,
        extra: Default::default(),
    }
}

fn make_streaming_agent_job(id: &str) -> CronJobSpec {
    let mut job = make_agent_job(id, None, None);
    job.text = None;
    job.request = Some(serde_json::json!({
        "input": "What time is it?"
    }));
    job.dispatch = Some(DispatchSpec {
        r#type: "channel".into(),
        channel: "console".into(),
        target: Some(DispatchTarget {
            user_id: "cron-agent-user".into(),
            session_id: "console:cron-agent-session".into(),
            extra: Default::default(),
        }),
        mode: "stream".into(),
        meta: Default::default(),
        extra: Default::default(),
    });
    job.agent_id = Some("cron-agent".into());
    job
}

fn make_streaming_agent_job_with_input(
    id: &str,
    input: serde_json::Value,
    agent_id: &str,
) -> CronJobSpec {
    let mut job = make_streaming_agent_job(id);
    job.request = Some(serde_json::json!({ "input": input }));
    job.agent_id = Some(agent_id.into());
    job
}

fn make_text_job(id: &str, text: Option<&str>) -> CronJobSpec {
    CronJobSpec {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        paused: false,
        schedule: Default::default(),
        task_type: "text".to_string(),
        text: text.map(str::to_string),
        request: None,
        dispatch: Some(DispatchSpec {
            r#type: "channel".into(),
            channel: "console".into(),
            target: Some(DispatchTarget {
                user_id: "cron-user".into(),
                session_id: "console:cron-session".into(),
                extra: Default::default(),
            }),
            mode: "stream".into(),
            meta: Default::default(),
            extra: Default::default(),
        }),
        save_result_to_inbox: None,
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

fn runtime(share_session: bool) -> JobRuntimeSpec {
    JobRuntimeSpec {
        max_concurrency: 1,
        timeout_seconds: 120,
        misfire_grace_seconds: 600,
        share_session,
        tool_safety: false,
        extra: Default::default(),
    }
}

fn make_runner(job: Option<CronJobSpec>, agent: Arc<FailNTimesAgentRunner>) -> CronRunner {
    let sender: Arc<dyn CronChannelSender> = Arc::new(MockChannelSender::new());
    let executor = Arc::new(CronExecutor::new(sender, agent));
    let jobs = Arc::new(DashMap::new());
    if let Some(job) = job {
        jobs.insert(job.id.clone(), job);
    }
    CronRunner::new(Some(executor), jobs, Arc::new(DashMap::new()))
}

fn make_executor_with_sender() -> (CronExecutor, Arc<MockChannelSender>) {
    let sender = Arc::new(MockChannelSender::new());
    let agent = Arc::new(FailNTimesAgentRunner::new(0, "unused"));
    let executor = CronExecutor::new(sender.clone(), agent);
    (executor, sender)
}

fn assert_single_text_delta(events: Vec<RuntimeEvent>, expected: &str) {
    assert_eq!(events.len(), 1, "expected one runtime event: {events:?}");
    match &events[0] {
        RuntimeEvent::TextDelta { delta, message_id } => {
            assert_eq!(delta, expected);
            assert!(message_id.is_none());
        }
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

struct FailingChannelSender;

#[async_trait]
impl CronChannelSender for FailingChannelSender {
    async fn send_text(
        &self,
        _agent_id: &str,
        channel: &str,
        _user_id: &str,
        _session_id: &str,
        _text: &str,
    ) -> Result<(), RuaError> {
        Err(RuaError::Runtime {
            message: format!("channel delivery failed: {channel}"),
        })
    }

    async fn send_event(
        &self,
        _agent_id: &str,
        channel: &str,
        _user_id: &str,
        _session_id: &str,
        _event: &RuntimeEvent,
    ) -> Result<(), RuaError> {
        Err(RuaError::Runtime {
            message: format!("event delivery failed: {channel}"),
        })
    }
}

struct ErroringStreamAgentRunner;

struct StreamingOnlyAgentRunner;
#[async_trait]
impl CronAgentRunner for StreamingOnlyAgentRunner {
    async fn run_query(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, RuaError> {
        panic!("executor must consume the stream");
    }
    async fn stream_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        Ok(Box::pin(futures::stream::iter(["part-1", "part-2"].map(
            |text| {
                Ok(RuntimeEvent::TextDelta {
                    delta: text.into(),
                    message_id: None,
                })
            },
        ))))
    }
}

#[tokio::test]
async fn final_mode_sends_one_complete_result_and_console_does_not_duplicate_inbox() {
    let sender = Arc::new(MockChannelSender::new());
    let executor = CronExecutor::new(sender.clone(), Arc::new(StreamingOnlyAgentRunner));
    let mut job = make_streaming_agent_job("final");
    job.dispatch.as_mut().unwrap().mode = "final".into();
    job.save_result_to_inbox = Some(true);
    assert!(executor.execute(&job).await.success);
    assert!(sender.sent_events().await.is_empty());
    let messages = sender.sent_messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].3, "part-1part-2");
    assert!(sender.saved_inbox_messages().await.is_empty());
}

struct TerminalRunner {
    error: bool,
}

#[tokio::test]
async fn final_mode_empty_output_does_not_send_and_console_text_is_not_saved_twice() {
    let sender = Arc::new(MockChannelSender::new());
    let executor = CronExecutor::new(sender.clone(), Arc::new(MockAgentRunner::new("")));
    let mut job = make_streaming_agent_job("empty-final");
    job.dispatch.as_mut().unwrap().mode = "final".into();
    assert!(executor.execute(&job).await.success);
    assert!(sender.sent_messages().await.is_empty());
    let mut text = make_text_job("console-once", Some("notification"));
    text.save_result_to_inbox = Some(true);
    assert!(executor.execute(&text).await.success);
    assert_eq!(sender.sent_messages().await.len(), 1);
    assert!(sender.saved_inbox_messages().await.is_empty());
}
#[async_trait]
impl CronAgentRunner for TerminalRunner {
    async fn run_query(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, RuaError> {
        unreachable!()
    }
    async fn stream_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        let terminal = if self.error {
            RuntimeEvent::Error {
                error: RuaError::Runtime {
                    message: "runtime failure".into(),
                },
            }
        } else {
            RuntimeEvent::End {
                finish_reason: Some("cancelled".into()),
            }
        };
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::TextDelta {
                delta: "partial".into(),
                message_id: None,
            }),
            Ok(terminal),
        ])))
    }
}

#[tokio::test]
async fn final_mode_never_delivers_partial_output_after_error_or_cancel() {
    for error in [false, true] {
        let sender = Arc::new(MockChannelSender::new());
        let executor = CronExecutor::new(sender.clone(), Arc::new(TerminalRunner { error }));
        let mut job = make_streaming_agent_job("terminal");
        job.dispatch.as_mut().unwrap().mode = "final".into();
        assert!(!executor.execute(&job).await.success);
        assert!(sender.sent_messages().await.is_empty());
        assert_eq!(sender.sent_events().await.len(), usize::from(error));
    }
}

#[tokio::test]
async fn final_send_failure_and_unknown_mode_are_reported() {
    let executor = CronExecutor::new(
        Arc::new(FailingChannelSender),
        Arc::new(StreamingOnlyAgentRunner),
    );
    let mut job = make_streaming_agent_job("send-failed");
    job.dispatch.as_mut().unwrap().mode = "final".into();
    assert!(!executor.execute(&job).await.success);
    job.dispatch.as_mut().unwrap().mode = "invalid".into();
    assert!(executor
        .execute(&job)
        .await
        .error
        .unwrap()
        .contains("unsupported dispatch mode"));
}

#[async_trait]
impl CronAgentRunner for ErroringStreamAgentRunner {
    async fn run_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<String, RuaError> {
        Ok("unused".into())
    }

    async fn stream_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        let events = vec![
            Ok(RuntimeEvent::TextDelta {
                delta: "partial cron response".into(),
                message_id: None,
            }),
            Err(RuaError::Runtime {
                message: "agent stream failed".into(),
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

async fn wait_done(runner: &CronRunner, job_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(state) = runner.get_state(job_id) {
            if state.last_status.as_deref() != Some("running") {
                return;
            }
        }
        assert!(Instant::now() < deadline, "job did not complete in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn text_job_execution_sends_text_to_dispatch_target() {
    let (executor, sender) = make_executor_with_sender();
    let job = make_text_job("text-success", Some("Hello from cron"));

    let result = executor.execute(&job).await;

    assert!(result.success, "text cron job should execute successfully");
    assert_eq!(result.job_id, "text-success");
    assert_eq!(result.error, None);
    assert_eq!(
        sender.sent_messages().await,
        vec![(
            "console".to_string(),
            "cron-user".to_string(),
            "console:cron-session".to_string(),
            "Hello from cron".to_string()
        )]
    );
}

#[tokio::test]
async fn text_job_save_result_to_inbox_records_visible_result() {
    let (executor, sender) = make_executor_with_sender();
    let mut job = make_text_job("text-inbox", Some("persist me"));
    job.dispatch.as_mut().unwrap().channel = "telegram".into();
    job.save_result_to_inbox = Some(true);

    let result = executor.execute(&job).await;

    assert!(result.success);
    assert_eq!(
        sender.saved_inbox_messages().await,
        vec![(
            "telegram".to_string(),
            "cron-user".to_string(),
            "console:cron-session".to_string(),
            "text-inbox".to_string(),
            "persist me".to_string(),
        )]
    );
}

#[tokio::test]
async fn empty_text_job_is_successful_noop_without_channel_send() {
    let (executor, sender) = make_executor_with_sender();
    let job = make_text_job("text-empty", Some("   "));

    let result = executor.execute(&job).await;

    assert!(
        result.success,
        "empty text jobs are skipped successfully at the executor boundary"
    );
    assert!(sender.sent_messages().await.is_empty());
}

#[tokio::test]
async fn agent_job_execution_streams_events_to_dispatch_target() {
    let sender = Arc::new(MockChannelSender::new());
    let agent = Arc::new(MockAgentRunner::new("Mock heartbeat response"));
    let executor = CronExecutor::new(sender.clone(), agent.clone());
    let job = make_streaming_agent_job("agent-stream-success");

    let result = executor.execute(&job).await;

    assert!(result.success, "agent cron job should execute successfully");
    assert_eq!(
        agent.recorded_calls().await,
        vec![(
            "cron-agent".into(),
            "cron-agent-user".into(),
            "console:cron-agent-session".into(),
            "What time is it?".into()
        )]
    );
    assert_single_text_delta(sender.sent_events().await, "Mock heartbeat response");
}

#[tokio::test]
async fn agent_job_can_use_isolated_run_session_and_save_result_to_inbox() {
    let sender = Arc::new(MockChannelSender::new());
    let agent = Arc::new(MockAgentRunner::new("Saved response"));
    let executor = CronExecutor::new(sender.clone(), agent.clone());
    let mut job = make_streaming_agent_job("agent-isolated-session");
    job.dispatch.as_mut().unwrap().channel = "telegram".into();
    job.runtime = Some(runtime(false));
    job.save_result_to_inbox = Some(true);

    let result = executor.execute(&job).await;

    assert!(result.success);
    let calls = agent.recorded_calls().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "cron-agent");
    assert_eq!(calls[0].1, "cron-agent-user");
    assert!(
        calls[0]
            .2
            .starts_with("console:cron-agent-session:cron:agent-isolated-session:"),
        "agent run should use isolated cron session, got {}",
        calls[0].2
    );
    assert_single_text_delta(sender.sent_events().await, "Saved response");
    assert_eq!(
        sender.saved_inbox_messages().await,
        vec![(
            "telegram".to_string(),
            "cron-agent-user".to_string(),
            "console:cron-agent-session".to_string(),
            "agent-isolated-session".to_string(),
            "Saved response".to_string(),
        )]
    );
}

#[tokio::test]
async fn agent_job_preserves_structured_request_input_as_json_projection() {
    let sender = Arc::new(MockChannelSender::new());
    let agent = Arc::new(MockAgentRunner::new("Structured response"));
    let executor = CronExecutor::new(sender.clone(), agent.clone());
    let input = serde_json::json!([
        {
            "role": "user",
            "type": "message",
            "content": [{ "type": "text", "text": "What time is it?" }]
        }
    ]);
    let job = make_streaming_agent_job_with_input(
        "agent-structured-input",
        input.clone(),
        "structured-agent",
    );

    let result = executor.execute(&job).await;

    assert!(result.success);
    let calls = agent.recorded_calls().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "structured-agent");
    let projected: serde_json::Value =
        serde_json::from_str(&calls[0].3).expect("structured input JSON projection");
    assert_eq!(projected, input);
    assert_single_text_delta(sender.sent_events().await, "Structured response");
}

#[tokio::test]
async fn text_delivery_failure_becomes_error_execution_result() {
    let sender: Arc<dyn CronChannelSender> = Arc::new(FailingChannelSender);
    let agent = Arc::new(FailNTimesAgentRunner::new(0, "unused"));
    let executor = CronExecutor::new(sender, agent);
    let job = make_text_job("text-delivery-error", Some("deliver me"));

    let result = executor.execute(&job).await;

    assert!(!result.success);
    let error = result.error.expect("delivery error");
    assert!(
        error.contains("channel delivery failed: console"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn agent_stream_error_is_reported_after_delivered_prefix_events() {
    let sender = Arc::new(MockChannelSender::new());
    let agent = Arc::new(ErroringStreamAgentRunner);
    let executor = CronExecutor::new(sender.clone(), agent);
    let job = make_streaming_agent_job("agent-stream-error");

    let result = executor.execute(&job).await;

    assert!(!result.success);
    let error = result.error.expect("agent stream error");
    assert!(
        error.contains("agent stream failed"),
        "unexpected error: {error}"
    );
    assert_single_text_delta(sender.sent_events().await, "partial cron response");
}

#[tokio::test]
async fn run_job_without_executor_reports_no_executor_without_mutating_state() {
    let jobs = Arc::new(DashMap::new());
    jobs.insert("no-exec".into(), make_agent_job("no-exec", None, None));
    let runner = CronRunner::new(None, jobs, Arc::new(DashMap::<String, CronJobState>::new()));

    assert_eq!(runner.run_job("no-exec"), RunJobStatus::NoExecutor);
    assert!(runner.get_state("no-exec").is_none());
}

#[tokio::test]
async fn run_job_missing_job_reports_not_found_without_creating_state() {
    let agent = Arc::new(FailNTimesAgentRunner::new(0, "ok"));
    let runner = make_runner(None, agent);

    assert_eq!(runner.run_job("missing"), RunJobStatus::NotFound);
    assert!(runner.get_state("missing").is_none());
}

#[tokio::test]
async fn spawned_job_is_marked_running_before_background_completion() {
    let agent = Arc::new(FailNTimesAgentRunner::new(0, "ok"));
    let runner = make_runner(Some(make_agent_job("spawned", None, None)), agent);

    assert_eq!(runner.run_job("spawned"), RunJobStatus::Spawned);
    let state = runner.get_state("spawned").expect("running state");
    assert_eq!(state.last_status.as_deref(), Some("running"));

    wait_done(&runner, "spawned").await;
    let state = runner.get_state("spawned").expect("final state");
    assert_eq!(state.last_status.as_deref(), Some("success"));
}

#[tokio::test]
async fn retry_3_always_fail_runs_4_times() {
    let agent = Arc::new(FailNTimesAgentRunner::new(100, "ok"));
    let calls = agent.calls.clone();
    let runner = make_runner(Some(make_agent_job("r3", Some(3), Some(0))), agent);

    assert_eq!(runner.run_job("r3"), RunJobStatus::Spawned);
    wait_done(&runner, "r3").await;

    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let state = runner.get_state("r3").unwrap();
    assert_eq!(state.last_status.as_deref(), Some("error"));
    assert!(state.last_error.is_some());
}

#[tokio::test]
async fn no_retry_when_max_retries_none() {
    let agent = Arc::new(FailNTimesAgentRunner::new(100, "ok"));
    let calls = agent.calls.clone();
    let runner = make_runner(Some(make_agent_job("no-retry", None, None)), agent);

    assert_eq!(runner.run_job("no-retry"), RunJobStatus::Spawned);
    wait_done(&runner, "no-retry").await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let state = runner.get_state("no-retry").unwrap();
    assert_eq!(state.last_status.as_deref(), Some("error"));
}

#[tokio::test]
async fn retry_delay_respected() {
    let agent = Arc::new(FailNTimesAgentRunner::new(100, "ok"));
    let runner = make_runner(Some(make_agent_job("delay", Some(1), Some(1))), agent);

    let start = Instant::now();
    assert_eq!(runner.run_job("delay"), RunJobStatus::Spawned);
    wait_done(&runner, "delay").await;

    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "retry delay not respected: elapsed {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn retry_succeeds_after_failures() {
    let agent = Arc::new(FailNTimesAgentRunner::new(2, "success"));
    let calls = agent.calls.clone();
    let runner = make_runner(Some(make_agent_job("recover", Some(3), Some(0))), agent);

    assert_eq!(runner.run_job("recover"), RunJobStatus::Spawned);
    wait_done(&runner, "recover").await;

    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let state = runner.get_state("recover").unwrap();
    assert_eq!(state.last_status.as_deref(), Some("success"));
    assert!(state.last_error.is_none());
}
