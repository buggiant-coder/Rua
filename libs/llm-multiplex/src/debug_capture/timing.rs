use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "openai-compatible")]
use agent_types::AgentEvent;
use serde::Serialize;
use serde_json::{Map, Value};

use super::clock::{CaptureClock, ClockSnapshot};
use super::network::NetworkCapture;
#[cfg(feature = "openai-compatible")]
use super::write_provider_artifact;
use super::writer::CaptureWriter;
use super::CaptureTarget;

/// An opt-in, local timing artifact. It observes a call without changing its request or events.
#[derive(Default)]
pub struct CaptureTrace {
    state: Option<Box<TraceState>>,
}

#[derive(Clone)]
struct TraceParent {
    target: CaptureTarget,
    cancel: tokio_util::sync::CancellationToken,
    writer: Arc<CaptureWriter>,
    trace_id: String,
}

tokio::task_local! {
    static CURRENT_TRACE: Option<TraceParent>;
}

struct TraceState {
    target: CaptureTarget,
    capture_cancel: tokio_util::sync::CancellationToken,
    writer: Arc<CaptureWriter>,
    path: PathBuf,
    clock: CaptureClock,
    record: TraceRecord,
    http: Option<std::sync::Arc<crate::transport::timing::HttpTiming>>,
    network: Option<NetworkCapture>,
}

#[derive(Serialize)]
struct TraceRecord {
    schema_version: u32,
    process_id: u32,
    process_clock_id: String,
    started_at_process_ms: f64,
    model_call_id: String,
    trace_id: String,
    stage: &'static str,
    started_at_unix_ms: u128,
    #[serde(flatten)]
    clock: ClockSnapshot,
    outcome: &'static str,
    milestones_ms: BTreeMap<&'static str, f64>,
    milestones_wall_ms: BTreeMap<&'static str, f64>,
    milestones_including_suspend_ms: BTreeMap<&'static str, f64>,
    data: Map<String, Value>,
}

impl CaptureTrace {
    pub fn new(metadata: &Map<String, Value>, stage: &'static str) -> Self {
        let Some((target, capture_cancel, writer)) = CaptureTarget::resolve(metadata) else {
            return Self::default();
        };
        Self::from_target(target, capture_cancel, writer, stage, None)
    }

    fn from_target(
        target: CaptureTarget,
        capture_cancel: tokio_util::sync::CancellationToken,
        writer: Arc<CaptureWriter>,
        stage: &'static str,
        parent_trace_id: Option<String>,
    ) -> Self {
        let write_initial_snapshot =
            parent_trace_id.is_none() && !matches!(stage, "capture" | "provider" | "rate-limit");
        let trace_id = uuid::Uuid::new_v4().to_string();
        let path = target
            .call_dir()
            .join(format!("{stage}-timing-{trace_id}.json"));
        let clock = CaptureClock::new();
        let mut trace = Self {
            state: Some(Box::new(TraceState {
                record: TraceRecord {
                    schema_version: 2,
                    process_id: std::process::id(),
                    process_clock_id: clock.process_clock_id().to_owned(),
                    started_at_process_ms: clock.started_at_process_ms(),
                    model_call_id: target.model_call_id.clone(),
                    trace_id,
                    stage,
                    started_at_unix_ms: clock.started_at_unix_ms(),
                    clock: clock.snapshot(),
                    outcome: "in_progress",
                    milestones_ms: BTreeMap::new(),
                    milestones_wall_ms: BTreeMap::new(),
                    milestones_including_suspend_ms: BTreeMap::new(),
                    data: parent_trace_id
                        .map(|id| ("parent_trace_id".into(), Value::from(id)))
                        .into_iter()
                        .collect(),
                },
                target,
                capture_cancel,
                writer,
                path,
                clock,
                http: None,
                network: None,
            })),
        };
        // Child and provider/capture traces are initialized before their first wait.
        // HTTP send/rate-limit wait or finish/Drop queues the initialized record,
        // avoiding an empty snapshot immediately followed by another write.
        if write_initial_snapshot {
            trace.flush();
        }
        trace
    }

    pub fn child(&self, stage: &'static str) -> Self {
        self.parent().map_or_else(Self::default, |parent| {
            Self::from_target(
                parent.target,
                parent.cancel,
                parent.writer,
                stage,
                Some(parent.trace_id),
            )
        })
    }

    /// Attribute internal Hook work without adding diagnostic fields to durable Messages.
    /// Spawned tasks do not inherit this scope; their caller must pass an explicit child trace.
    /// Fast local work only records marks; a pending phase queues a snapshot after
    /// 100 ms so a stalled Hook/save remains inspectable without per-Hook writes.
    pub async fn measure_local<F: Future>(
        &mut self,
        started: &'static str,
        finished: &'static str,
        future: F,
    ) -> F::Output {
        self.mark(started);
        let future = CURRENT_TRACE.scope(self.parent(), future);
        let output = if self.is_enabled() {
            tokio::pin!(future);
            tokio::select! {
                biased;
                output = &mut future => {
                    if let Some(state) = self.state.as_mut() {
                        let count = state.record.data.get("coalesced_trace_checkpoints")
                            .and_then(Value::as_u64).unwrap_or(0);
                        state.record.data.insert("coalesced_trace_checkpoints".into(), (count + 1).into());
                    }
                    output
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    self.flush();
                    future.await
                }
            }
        } else {
            future.await
        };
        self.mark(finished);
        output
    }

    pub fn current(stage: &'static str) -> Self {
        CURRENT_TRACE
            .try_with(Clone::clone)
            .ok()
            .flatten()
            .map_or_else(Self::default, |parent| {
                Self::from_target(
                    parent.target,
                    parent.cancel,
                    parent.writer,
                    stage,
                    Some(parent.trace_id),
                )
            })
    }

    fn parent(&self) -> Option<TraceParent> {
        self.state.as_ref().map(|state| TraceParent {
            target: state.target.clone(),
            cancel: state.capture_cancel.clone(),
            writer: state.writer.clone(),
            trace_id: state.record.trace_id.clone(),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn observe_http(
        &mut self,
        connections: std::sync::Arc<crate::transport::timing::ConnectionRegistry>,
    ) -> Option<std::sync::Arc<crate::transport::timing::HttpTiming>> {
        let state = self.state.as_mut()?;
        let timing = crate::transport::timing::HttpTiming::new(state.clock.clone(), connections);
        state.http = Some(timing.clone());
        state.record.data.insert(
            "network_trace_artifact".into(),
            Value::from(format!("network-timing-{}.json", state.record.trace_id)),
        );
        state.network = Some(NetworkCapture::start(
            &state.target,
            &state.record.trace_id,
            state.clock.clone(),
            timing.clone(),
            &state.capture_cancel,
            state.writer.clone(),
        ));
        self.flush();
        Some(timing)
    }

    pub fn set(&mut self, name: &str, value: impl Into<Value>) {
        if let Some(state) = self.state.as_mut() {
            state.record.data.insert(name.into(), value.into());
        }
    }

    pub fn mark(&mut self, name: &'static str) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        state.record.mark(name, &state.clock.snapshot());
    }

    /// Queue only first-occurrence wait boundaries, never every token or packet.
    pub fn checkpoint(&mut self, name: &'static str) {
        if self
            .state
            .as_ref()
            .is_some_and(|state| !state.record.milestones_ms.contains_key(name))
        {
            self.mark(name);
            if name == "response_headers" {
                if let Some(network) = self.state.as_ref().and_then(|state| state.network.as_ref())
                {
                    network.sample_now("response_headers");
                }
            }
            self.flush();
        }
    }

    pub(crate) fn checkpoint_at(&mut self, name: &'static str, clock: ClockSnapshot) {
        if let Some(state) = self.state.as_mut() {
            if state.record.mark(name, &clock) {
                if name == "new_connection_established" {
                    if let Some(network) = &state.network {
                        network.sample_now(name);
                    }
                }
                self.flush();
            }
        }
    }

    pub fn add_duration(&mut self, total: &str, maximum: &str, duration: Duration) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let elapsed = millis(duration);
        let data = &mut state.record.data;
        let sum = data.get(total).and_then(Value::as_f64).unwrap_or(0.0) + elapsed;
        let max = data
            .get(maximum)
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .max(elapsed);
        data.insert(total.into(), Value::from(sum));
        data.insert(maximum.into(), Value::from(max));
    }

    pub fn finish(&mut self, outcome: &'static str) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if state.record.outcome != "in_progress" {
            return;
        }
        state.record.outcome = outcome;
        if let Some(network) = &state.network {
            network.finish(outcome);
        }
        self.flush();
    }

    #[cfg(feature = "openai-compatible")]
    pub(crate) fn record_request(&mut self, provider: &str, endpoint: &str, body: &Value) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let started = Instant::now();
        let path = write_provider_artifact(
            &state.target,
            &state.writer,
            "request",
            provider,
            endpoint,
            body,
        );
        self.set("request_capture_enqueue_ms", millis(started.elapsed()));
        self.set("request_capture_queued", path.is_some());
        self.set("provider", provider);
        self.set("endpoint", endpoint);
        self.set("model", body.get("model").cloned().unwrap_or(Value::Null));
        if let Some(path) = path {
            self.set(
                "request_artifact",
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
            self.mark("request_capture_enqueued");
        }
    }

    pub(crate) fn response_headers(&mut self, status: u16, headers: &reqwest::header::HeaderMap) {
        if !self.is_enabled() {
            return;
        }
        let safe_headers = [
            "x-request-id",
            "request-id",
            "x-dashscope-request-id",
            "x-amzn-requestid",
            "server-timing",
            "x-envoy-upstream-service-time",
            "retry-after",
            "content-type",
        ]
        .into_iter()
        .filter_map(|name| {
            Some((
                name.to_owned(),
                Value::from(headers.get(name)?.to_str().ok()?),
            ))
        })
        .collect::<Map<_, _>>();
        self.set("http_status", status);
        self.set("response_headers", safe_headers);
        self.checkpoint("response_headers");
    }

    #[cfg(feature = "openai-compatible")]
    pub(crate) fn received_bytes(&mut self, bytes: usize) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        for (name, amount) in [("response_bytes", bytes as u64), ("response_chunks", 1)] {
            let count = state
                .record
                .data
                .get(name)
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + amount;
            state.record.data.insert(name.into(), Value::from(count));
        }
        state.record.data.insert(
            "last_response_byte_ms".into(),
            Value::from(millis(state.clock.started.elapsed())),
        );
        self.checkpoint("first_response_byte");
    }

    #[cfg(feature = "openai-compatible")]
    pub(crate) fn response_metadata(&mut self, value: &Value) {
        if !self.is_enabled() {
            return;
        }
        for (source, destination) in [
            ("id", "response_id"),
            ("model", "response_model"),
            ("usage", "raw_usage"),
        ] {
            if let Some(value) = value.get(source).filter(|value| !value.is_null()) {
                self.set(destination, value.clone());
            }
        }
    }

    #[cfg(feature = "openai-compatible")]
    pub(crate) fn error(&mut self, error: &crate::LlmError, stage: &'static str) {
        use crate::LlmError;
        let category = match error {
            LlmError::Auth { .. } => "auth",
            LlmError::RateLimit { .. } => "rate_limit",
            LlmError::Server { .. } => "server",
            LlmError::Timeout { .. } => "timeout",
            LlmError::RetryDeadlineExceeded { .. } => "retry_deadline",
            LlmError::Connection { .. } => "connection",
            LlmError::InvalidResponse { .. } => "invalid_response",
            LlmError::ModelNotFound { .. } => "model_not_found",
            LlmError::Cancelled => "cancelled",
            LlmError::Config { .. } => "config",
            LlmError::Unknown { .. } => "unknown",
        };
        // Error messages may contain URLs/credentials; retain only classification.
        self.set("error_kind", stage);
        self.set("error_category", category);
        self.set("retryable", error.is_retryable());
        self.finish("error");
    }

    #[cfg(feature = "openai-compatible")]
    pub(crate) fn event(&mut self, event: &AgentEvent) {
        if !self.is_enabled() {
            return;
        }
        let milestone = match event {
            AgentEvent::ThinkingDelta { delta, .. } if !delta.is_empty() => "first_thinking_delta",
            AgentEvent::TextDelta { delta, .. } if !delta.is_empty() => "first_text_delta",
            AgentEvent::ToolCallDelta {
                name,
                arguments_delta,
                ..
            } if name.as_ref().is_some_and(|name| !name.is_empty())
                || !arguments_delta.is_empty() =>
            {
                "first_tool_call_delta"
            }
            AgentEvent::Usage { usage } => {
                self.set("usage", serde_json::to_value(usage).unwrap_or(Value::Null));
                "first_usage"
            }
            AgentEvent::Error { .. } => {
                self.set("error_kind", "provider_stream_event");
                self.finish("error");
                return;
            }
            _ => return,
        };
        if event.is_content() {
            if let Some(state) = self.state.as_mut() {
                state.record.data.insert(
                    "last_output_ms".into(),
                    Value::from(millis(state.clock.started.elapsed())),
                );
            }
        }
        self.checkpoint(milestone);
    }

    fn flush(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        state.record.clock = state.clock.snapshot();
        if let Some(http) = &state.http {
            state
                .record
                .data
                .insert("http_transport".into(), http.snapshot());
        }
        if let Some(network) = &state.network {
            state
                .record
                .data
                .insert("network_capture_outcome".into(), network.outcome().into());
        }
        state
            .record
            .data
            .insert("capture_write_mode".into(), "background_bounded".into());
        state.record.data.insert(
            "capture_writer_status_before_enqueue".into(),
            serde_json::to_value(state.writer.status()).unwrap_or(Value::Null),
        );
        let started = Instant::now();
        state
            .writer
            .record(state.path.clone(), &state.record, false);
        self.add_duration(
            "trace_enqueue_ms_before_last_snapshot",
            "max_trace_enqueue_ms_before_last_snapshot",
            started.elapsed(),
        );
    }
}

impl TraceRecord {
    fn mark(&mut self, name: &'static str, clock: &ClockSnapshot) -> bool {
        if self.milestones_ms.contains_key(name) {
            return false;
        }
        self.milestones_ms.insert(name, clock.elapsed_ms);
        self.milestones_wall_ms.insert(name, clock.wall_elapsed_ms);
        if let Some(elapsed) = clock.elapsed_including_suspend_ms {
            self.milestones_including_suspend_ms.insert(name, elapsed);
        }
        true
    }
}

impl Drop for CaptureTrace {
    fn drop(&mut self) {
        // Dropping a future/stream is not evidence of success (or necessarily a user Stop).
        self.finish("dropped");
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
