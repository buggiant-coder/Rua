//! Opt-in, per-attempt socket samples. No processes, packet capture, or request changes.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rua_http::diagnostics::{SocketReading, SocketSampler, SOCKET_COUNTERS_SUPPORTED};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::clock::{CaptureClock, ClockSnapshot};
use super::writer::CaptureWriter;
use super::CaptureTarget;
use crate::transport::timing::{HttpTiming, SocketIdentity, MAX_TRACKED_CONNECTIONS};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const WRITE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SAMPLES: usize = 600;
const KEEP_INITIAL: usize = 32;

pub(super) struct NetworkCapture {
    state: Arc<Mutex<State>>,
    cancel: CancellationToken,
}

struct State {
    path: PathBuf,
    writer: Arc<CaptureWriter>,
    writer_failures_at_start: u64,
    clock: CaptureClock,
    timing: Arc<HttpTiming>,
    samplers: HashMap<SocketIdentity, SocketSampler>,
    last_write: Instant,
    written_clock_delta_ms: f64,
    record: Record,
}

#[derive(Serialize)]
struct Record {
    schema_version: u32,
    process_id: u32,
    model_call_id: String,
    trace_id: String,
    stage: &'static str,
    started_at_unix_ms: u128,
    #[serde(flatten)]
    clock: ClockSnapshot,
    outcome: &'static str,
    availability: &'static str,
    counter_scope: &'static str,
    sample_interval_ms: u128,
    sample_limit: usize,
    initial_samples_retained: usize,
    candidate_connection_limit: usize,
    candidate_connections_omitted: u64,
    confirmed_connection: Option<SocketIdentity>,
    sample_count: u64,
    samples_omitted: u64,
    probe_total_ms: f64,
    max_probe_ms: f64,
    enqueue_ms_before_snapshot: f64,
    capture_write_mode: &'static str,
    samples: VecDeque<Sample>,
}

#[derive(Serialize)]
struct Sample {
    sequence: u64,
    reason: &'static str,
    attribution: &'static str,
    #[serde(flatten)]
    clock: ClockSnapshot,
    local_addr: Option<String>,
    remote_addr: Option<String>,
    probe_ms: f64,
    #[serde(flatten)]
    reading: SocketReading,
}

impl NetworkCapture {
    pub(super) fn start(
        target: &CaptureTarget,
        trace_id: &str,
        clock: CaptureClock,
        timing: Arc<HttpTiming>,
        parent_cancel: &CancellationToken,
        writer: Arc<CaptureWriter>,
    ) -> Self {
        let cancel = parent_cancel.child_token();
        let state = Arc::new(Mutex::new(State {
            path: target
                .call_dir()
                .join(format!("network-timing-{trace_id}.json")),
            writer_failures_at_start: writer.status().failed_artifacts,
            writer: writer.clone(),
            record: Record {
                schema_version: 2,
                process_id: std::process::id(),
                model_call_id: target.model_call_id.clone(),
                trace_id: trace_id.to_owned(),
                stage: "network",
                started_at_unix_ms: clock.started_at_unix_ms(),
                clock: clock.snapshot(),
                outcome: "in_progress",
                availability: if SOCKET_COUNTERS_SUPPORTED {
                    "supported"
                } else {
                    "unsupported_platform"
                },
                counter_scope: "socket_lifetime_not_http_request",
                sample_interval_ms: SAMPLE_INTERVAL.as_millis(),
                sample_limit: MAX_SAMPLES,
                initial_samples_retained: KEEP_INITIAL,
                candidate_connection_limit: MAX_TRACKED_CONNECTIONS,
                candidate_connections_omitted: 0,
                confirmed_connection: None,
                sample_count: 0,
                samples_omitted: 0,
                probe_total_ms: 0.0,
                max_probe_ms: 0.0,
                enqueue_ms_before_snapshot: 0.0,
                capture_write_mode: "background_bounded",
                samples: VecDeque::new(),
            },
            clock,
            timing,
            samplers: HashMap::new(),
            last_write: Instant::now(),
            written_clock_delta_ms: 0.0,
        }));
        let capture = Self {
            state: state.clone(),
            cancel: cancel.clone(),
        };
        if cancel.is_cancelled() {
            capture.finish("capture_disabled");
            return capture;
        }
        capture.sample_now("http_send_started");
        // Unsupported counters still need a cancellation task to finish the record.
        writer.samplers.spawn(async move {
            let mut ticker = tokio::time::interval_at(
                tokio::time::Instant::now() + SAMPLE_INTERVAL,
                SAMPLE_INTERVAL,
            );
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        state.lock().unwrap_or_else(|e| e.into_inner()).finish("capture_disabled");
                        break;
                    }
                    _ = ticker.tick(), if SOCKET_COUNTERS_SUPPORTED => {
                        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                        if cancel.is_cancelled() { continue; }
                        state.sample("interval");
                        let clock_shift = (state.record.clock.wall_monotonic_delta_ms
                            - state.written_clock_delta_ms).abs() >= 1000.0;
                        if (state.last_write.elapsed() >= WRITE_INTERVAL || clock_shift)
                            && !state.write()
                        {
                            cancel.cancel();
                        }
                    }
                }
            }
        });
        capture
    }

    /// Connection/header boundaries also sample, so a sub-second call has evidence.
    pub(super) fn sample_now(&self, reason: &'static str) {
        if self.cancel.is_cancelled() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.record.outcome != "in_progress" {
            return;
        }
        state.sample(reason);
        if !state.write() {
            self.cancel.cancel();
        }
    }

    pub(super) fn finish(&self, outcome: &'static str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancel.is_cancelled() {
            state.finish("capture_disabled");
        } else {
            state.finish(outcome);
        }
        self.cancel.cancel();
    }

    pub(super) fn outcome(&self) -> &'static str {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.writer.status().failed_artifacts > state.writer_failures_at_start {
            "capture_write_failed"
        } else {
            state.record.outcome
        }
    }
}

impl Drop for NetworkCapture {
    fn drop(&mut self) {
        self.finish("dropped");
    }
}

impl State {
    fn sample(&mut self, reason: &'static str) {
        if self.record.outcome != "in_progress" || !SOCKET_COUNTERS_SUPPORTED {
            return;
        }
        let started = Instant::now();
        let (candidates, omitted) = self.timing.connections.snapshot();
        self.record.candidate_connections_omitted = omitted;
        let confirmed = self.timing.confirmed_socket();
        let sockets = confirmed.map(|socket| vec![socket]).unwrap_or(candidates);
        self.samplers.retain(|socket, _| sockets.contains(socket));
        if sockets.is_empty() {
            self.push_sample(
                reason,
                "unavailable",
                None,
                SocketReading {
                    status: "awaiting_connection",
                    counters: None,
                },
                0.0,
            );
        }
        for socket in sockets {
            let probe_started = Instant::now();
            let reading = self
                .samplers
                .entry(socket)
                .or_default()
                .sample(socket.local_addr, socket.remote_addr);
            if reading.status == "socket_not_found" {
                // Closed pooled candidates must not cause an fd scan every second.
                self.timing.connections.forget_closed(socket);
            }
            let probe_ms = probe_started.elapsed().as_secs_f64() * 1000.0;
            self.push_sample(
                reason,
                if confirmed.is_some() {
                    "response_confirmed"
                } else {
                    "transport_candidate"
                },
                Some(socket),
                reading,
                probe_ms,
            );
        }
        let probe_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.record.probe_total_ms += probe_ms;
        self.record.max_probe_ms = self.record.max_probe_ms.max(probe_ms);
    }

    fn push_sample(
        &mut self,
        reason: &'static str,
        attribution: &'static str,
        socket: Option<SocketIdentity>,
        reading: SocketReading,
        probe_ms: f64,
    ) {
        let clock = self.clock.snapshot();
        self.record.clock = clock.clone();
        self.record.sample_count += 1;
        if self.record.samples.len() == MAX_SAMPLES {
            // Preserve upload startup plus the recent tail, with an explicit gap count.
            self.record.samples.remove(KEEP_INITIAL);
            self.record.samples_omitted += 1;
        }
        self.record.samples.push_back(Sample {
            sequence: self.record.sample_count,
            reason,
            attribution,
            clock,
            local_addr: socket.map(|socket| socket.local_addr.to_string()),
            remote_addr: socket.map(|socket| socket.remote_addr.to_string()),
            probe_ms,
            reading,
        });
    }

    fn write(&mut self) -> bool {
        if self.writer.status().failed_artifacts > self.writer_failures_at_start {
            self.record.outcome = "capture_write_failed";
            return false;
        }
        self.record.clock = self.clock.snapshot();
        self.record.confirmed_connection = self.timing.confirmed_socket();
        let start = Instant::now();
        let queued = self.writer.record(self.path.clone(), &self.record, false);
        self.record.enqueue_ms_before_snapshot += start.elapsed().as_secs_f64() * 1000.0;
        self.last_write = Instant::now();
        self.written_clock_delta_ms = self.record.clock.wall_monotonic_delta_ms;
        if !queued {
            self.record.outcome = "enqueue_failed";
        }
        queued
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.record.outcome != "in_progress" {
            return;
        }
        if outcome != "capture_disabled" {
            self.sample(outcome);
        }
        self.record.outcome = outcome;
        self.write();
    }
}
