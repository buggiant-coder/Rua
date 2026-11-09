use async_trait::async_trait;
use futures::StreamExt;
use rua_config::types::cron::CronJobSpec;
use rua_core::event::RuntimeEvent;
use rua_core::RuaError;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronJobStatus {
    Running,
    Success,
    Error,
    Skipped,
    Cancelled,
}

impl CronJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Error => "error",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobExecutionResult {
    pub job_id: String,

    pub success: bool,

    pub error: Option<String>,

    pub duration_ms: u64,
}

#[async_trait]
pub trait CronChannelSender: Send + Sync {
    async fn send_text(
        &self,
        agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        text: &str,
    ) -> Result<(), RuaError>;

    async fn send_event(
        &self,
        agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        event: &RuntimeEvent,
    ) -> Result<(), RuaError>;

    async fn save_result_to_inbox(
        &self,
        _agent_id: &str,
        _channel: &str,
        _user_id: &str,
        _session_id: &str,
        _job_name: &str,
        _text: &str,
    ) -> Result<(), RuaError> {
        Ok(())
    }
}

pub type RuntimeEventStream =
    Pin<Box<dyn futures::Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>;

#[derive(Debug, Clone, Copy)]
pub struct CronAgentRunOptions {
    pub tool_safety: bool,
}

#[async_trait]
pub trait CronAgentRunner: Send + Sync {
    async fn run_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<String, RuaError>;

    async fn stream_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        let text = self.run_query(agent_id, user_id, session_id, input).await?;
        let events: Vec<Result<RuntimeEvent, RuaError>> = vec![Ok(RuntimeEvent::TextDelta {
            delta: text,
            message_id: None,
        })];
        Ok(Box::pin(futures::stream::iter(events)))
    }

    async fn stream_query_with_options(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
        _options: CronAgentRunOptions,
    ) -> Result<RuntimeEventStream, RuaError> {
        self.stream_query(agent_id, user_id, session_id, input)
            .await
    }
}

pub struct CronExecutor {
    channel_sender: Arc<dyn CronChannelSender>,

    agent_runner: Arc<dyn CronAgentRunner>,

    _max_concurrency: u32,

    running_counts: dashmap::DashMap<String, Arc<AtomicUsize>>,
}

impl CronExecutor {
    pub fn new(
        channel_sender: Arc<dyn CronChannelSender>,
        agent_runner: Arc<dyn CronAgentRunner>,
    ) -> Self {
        Self {
            channel_sender,
            agent_runner,
            _max_concurrency: 1,
            running_counts: dashmap::DashMap::new(),
        }
    }

    pub async fn execute(&self, job: &CronJobSpec) -> JobExecutionResult {
        let job_id = job.id.clone();
        let start = std::time::Instant::now();

        let max_concurrency = job.runtime.as_ref().map(|r| r.max_concurrency).unwrap_or(1);

        let counter = {
            let entry = self
                .running_counts
                .entry(job_id.clone())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
            Arc::clone(entry.value())
        };

        let running = counter.fetch_add(1, Ordering::SeqCst);

        if running >= max_concurrency as usize {
            counter.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!(
                job_id = %job_id,
                running = running,
                max = max_concurrency,
                "Job execution skipped: max concurrency reached"
            );
            return JobExecutionResult {
                job_id,
                success: false,
                error: Some("max concurrency reached".to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }

        let timeout_secs = job
            .runtime
            .as_ref()
            .map(|r| r.timeout_seconds)
            .unwrap_or(120);

        let result = tokio::time::timeout(
            Duration::from_secs(u64::from(timeout_secs)),
            self.execute_inner(job),
        )
        .await;

        counter.fetch_sub(1, Ordering::SeqCst);

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(())) => JobExecutionResult {
                job_id,
                success: true,
                error: None,
                duration_ms,
            },
            Ok(Err(e)) => JobExecutionResult {
                job_id,
                success: false,
                error: Some(e.to_string()),
                duration_ms,
            },
            Err(_) => {
                tracing::error!(
                    job_id = %job_id,
                    timeout_secs = timeout_secs,
                    "Job execution timed out"
                );
                JobExecutionResult {
                    job_id,
                    success: false,
                    error: Some(format!("execution timed out after {timeout_secs}s")),
                    duration_ms,
                }
            }
        }
    }

    async fn execute_inner(&self, job: &CronJobSpec) -> Result<(), RuaError> {
        let (channel, user_id, session_id) = extract_dispatch(job);

        match job.task_type.as_str() {
            "text" => {
                self.execute_text(job, &channel, &user_id, &session_id)
                    .await
            }
            "agent" => {
                self.execute_agent(job, &channel, &user_id, &session_id)
                    .await
            }
            other => {
                tracing::warn!(
                    job_id = %job.id,
                    task_type = other,
                    "Unknown task_type, skipping"
                );
                Err(RuaError::Cron {
                    job_id: job.id.clone(),
                    message: format!("unknown task_type: {other}"),
                })
            }
        }
    }

    async fn execute_text(
        &self,
        job: &CronJobSpec,
        channel: &str,
        user_id: &str,
        session_id: &str,
    ) -> Result<(), RuaError> {
        let text = job
            .text
            .as_deref()
            .or(job.prompt.as_deref())
            .unwrap_or("")
            .trim();

        if text.is_empty() {
            tracing::warn!(job_id = %job.id, "Text job has no text/prompt, skipping");
            return Ok(());
        }
        let agent_id = job.agent_id.as_deref().unwrap_or("default");

        tracing::info!(
            job_id = %job.id,
            agent_id = agent_id,
            channel = channel,
            "Executing text cron job"
        );

        self.channel_sender
            .send_text(agent_id, channel, user_id, session_id, text)
            .await?;
        if job.save_result_to_inbox.unwrap_or(false) && channel != "console" {
            self.channel_sender
                .save_result_to_inbox(agent_id, channel, user_id, session_id, &job.name, text)
                .await?;
        }
        Ok(())
    }

    async fn execute_agent(
        &self,
        job: &CronJobSpec,
        channel: &str,
        user_id: &str,
        session_id: &str,
    ) -> Result<(), RuaError> {
        let agent_id = job.agent_id.as_deref().unwrap_or("default");

        let input = if let Some(request) = &job.request {
            if let Some(input_val) = request.get("input") {
                match input_val {
                    serde_json::Value::String(s) => s.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                }
            } else {
                serde_json::to_string(request).unwrap_or_default()
            }
        } else if let Some(prompt) = &job.prompt {
            prompt.clone()
        } else if let Some(text) = &job.text {
            text.clone()
        } else {
            tracing::warn!(job_id = %job.id, "Agent job has no input, skipping");
            return Ok(());
        };

        tracing::info!(
            job_id = %job.id,
            agent_id = agent_id,
            channel = channel,
            "Executing agent cron job (streaming)"
        );

        let runtime = job.runtime.as_ref();
        let run_session_id = if runtime.is_some_and(|r| !r.share_session) {
            format!("{session_id}:cron:{}:{}", job.id, uuid::Uuid::new_v4())
        } else {
            session_id.to_string()
        };
        let options = CronAgentRunOptions {
            tool_safety: runtime.is_some_and(|r| r.tool_safety),
        };

        let mode = job
            .dispatch
            .as_ref()
            .map_or("stream", |dispatch| dispatch.mode.as_str());
        if !matches!(mode, "stream" | "final") {
            return Err(RuaError::Cron {
                job_id: job.id.clone(),
                message: format!("unsupported dispatch mode: {mode}"),
            });
        }
        let mut stream = self
            .agent_runner
            .stream_query_with_options(agent_id, user_id, &run_session_id, &input, options)
            .await?;

        let mut output = String::new();
        while let Some(event_result) = stream.next().await {
            let event = event_result?;
            if let RuntimeEvent::TextDelta { delta, .. } = &event {
                output.push_str(delta);
            }
            if mode == "stream"
                || matches!(
                    event,
                    RuntimeEvent::ToolApprovalPending { .. }
                        | RuntimeEvent::ToolApprovalRequest { .. }
                        | RuntimeEvent::AskUser { .. }
                        | RuntimeEvent::Error { .. }
                )
            {
                self.channel_sender
                    .send_event(agent_id, channel, user_id, session_id, &event)
                    .await?;
            }
            if let RuntimeEvent::Error { error } = event {
                return Err(error);
            }
            if let RuntimeEvent::End {
                finish_reason: Some(reason),
            } = &event
            {
                if matches!(
                    reason.as_str(),
                    "cancelled" | "canceled" | "aborted" | "error"
                ) {
                    return Err(RuaError::Cron {
                        job_id: job.id.clone(),
                        message: format!("agent run ended: {reason}"),
                    });
                }
            }
        }

        if mode == "final" && !output.trim().is_empty() {
            self.channel_sender
                .send_text(agent_id, channel, user_id, session_id, &output)
                .await?;
        }
        if job.save_result_to_inbox.unwrap_or(false)
            && channel != "console"
            && !output.trim().is_empty()
        {
            self.channel_sender
                .save_result_to_inbox(agent_id, channel, user_id, session_id, &job.name, &output)
                .await?;
        }

        Ok(())
    }
}

pub fn extract_dispatch(job: &CronJobSpec) -> (String, String, String) {
    if let Some(dispatch) = &job.dispatch {
        let channel = if dispatch.channel.is_empty() {
            "console".to_string()
        } else {
            dispatch.channel.clone()
        };

        let (user_id, session_id) = if let Some(target) = &dispatch.target {
            (target.user_id.clone(), target.session_id.clone())
        } else {
            ("default".to_string(), "default".to_string())
        };

        (channel, user_id, session_id)
    } else {
        (
            "console".to_string(),
            "default".to_string(),
            "default".to_string(),
        )
    }
}

pub type MockMessage = (String, String, String, String);
pub type MockInboxMessage = (String, String, String, String, String);

pub struct MockChannelSender {
    pub messages: Arc<tokio::sync::Mutex<Vec<MockMessage>>>,
    pub events: Arc<tokio::sync::Mutex<Vec<RuntimeEvent>>>,
    pub inbox_messages: Arc<tokio::sync::Mutex<Vec<MockInboxMessage>>>,
}

impl MockChannelSender {
    pub fn new() -> Self {
        Self {
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            inbox_messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub async fn sent_messages(&self) -> Vec<MockMessage> {
        self.messages.lock().await.clone()
    }

    pub async fn sent_events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().await.clone()
    }

    pub async fn saved_inbox_messages(&self) -> Vec<MockInboxMessage> {
        self.inbox_messages.lock().await.clone()
    }
}

impl Default for MockChannelSender {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CronChannelSender for MockChannelSender {
    async fn send_text(
        &self,
        _agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        text: &str,
    ) -> Result<(), RuaError> {
        self.messages.lock().await.push((
            channel.to_string(),
            user_id.to_string(),
            session_id.to_string(),
            text.to_string(),
        ));
        Ok(())
    }

    async fn send_event(
        &self,
        _agent_id: &str,
        _channel: &str,
        _user_id: &str,
        _session_id: &str,
        event: &RuntimeEvent,
    ) -> Result<(), RuaError> {
        self.events.lock().await.push(event.clone());
        Ok(())
    }

    async fn save_result_to_inbox(
        &self,
        _agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        job_name: &str,
        text: &str,
    ) -> Result<(), RuaError> {
        self.inbox_messages.lock().await.push((
            channel.to_string(),
            user_id.to_string(),
            session_id.to_string(),
            job_name.to_string(),
            text.to_string(),
        ));
        Ok(())
    }
}

pub type MockAgentCall = (String, String, String, String);

pub struct MockAgentRunner {
    pub response: String,
    pub calls: Arc<tokio::sync::Mutex<Vec<MockAgentCall>>>,
}

impl MockAgentRunner {
    pub fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
            calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub async fn recorded_calls(&self) -> Vec<MockAgentCall> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl CronAgentRunner for MockAgentRunner {
    async fn run_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<String, RuaError> {
        self.calls.lock().await.push((
            agent_id.to_string(),
            user_id.to_string(),
            session_id.to_string(),
            input.to_string(),
        ));
        Ok(self.response.clone())
    }

    async fn stream_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        self.calls.lock().await.push((
            agent_id.to_string(),
            user_id.to_string(),
            session_id.to_string(),
            input.to_string(),
        ));
        let events: Vec<Result<RuntimeEvent, RuaError>> = vec![Ok(RuntimeEvent::TextDelta {
            delta: self.response.clone(),
            message_id: None,
        })];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

pub struct SlowAgentRunner {
    pub delay: Duration,
}

#[async_trait]
impl CronAgentRunner for SlowAgentRunner {
    async fn run_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<String, RuaError> {
        tokio::time::sleep(self.delay).await;
        Ok("delayed response".to_string())
    }

    async fn stream_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        tokio::time::sleep(self.delay).await;
        let events: Vec<Result<RuntimeEvent, RuaError>> = vec![Ok(RuntimeEvent::TextDelta {
            delta: "delayed response".to_string(),
            message_id: None,
        })];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

pub struct FailNTimesAgentRunner {
    pub fail_count: u32,
    pub calls: Arc<AtomicUsize>,
    pub success_response: String,
}

impl FailNTimesAgentRunner {
    pub fn new(fail_count: u32, success_response: &str) -> Self {
        Self {
            fail_count,
            calls: Arc::new(AtomicUsize::new(0)),
            success_response: success_response.to_string(),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CronAgentRunner for FailNTimesAgentRunner {
    async fn run_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<String, RuaError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count as usize {
            Err(RuaError::Runtime {
                message: format!("intentional failure #{}", n + 1),
            })
        } else {
            Ok(self.success_response.clone())
        }
    }
}

pub struct ErrorAgentRunner;

#[async_trait]
impl CronAgentRunner for ErrorAgentRunner {
    async fn run_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<String, RuaError> {
        Err(RuaError::Runtime {
            message: "agent error".to_string(),
        })
    }

    async fn stream_query(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _session_id: &str,
        _input: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        Err(RuaError::Runtime {
            message: "agent error".to_string(),
        })
    }
}
