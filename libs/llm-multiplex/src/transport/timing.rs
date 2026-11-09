//! Observe reqwest without replacing its replayable body, pool, TLS, or retry policy.
//!
//! Reqwest does not expose a socket-upload-complete callback. In particular, a
//! Body::poll_frame callback would measure handoff, not upload, and wrapping the
//! body would disable its try_clone/redirect/retry behavior. Do not do that here.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::client::legacy::connect::{Connected, Connection, HttpInfo};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{json, Value};
use tower::{Layer, Service};

use crate::debug_capture::clock::{CaptureClock, ClockSnapshot};

const MAX_ATTEMPTS: usize = 8;
pub(crate) const MAX_TRACKED_CONNECTIONS: usize = 16;

/// Remember only socket identities, including sockets opened before capture was
/// enabled. No counters are read until a captured request starts. Reqwest 0.13
/// drops request extensions before Hyper, so its pool selection is not exposed.
#[derive(Debug, Default)]
pub(crate) struct ConnectionRegistry {
    sockets: Mutex<(Vec<SocketIdentity>, u64)>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub(crate) struct SocketIdentity {
    pub(crate) local_addr: SocketAddr,
    pub(crate) remote_addr: SocketAddr,
}

impl SocketIdentity {
    fn from_connected(connection: &Connected) -> Option<Self> {
        let mut extras = http::Extensions::new();
        connection.get_extras(&mut extras);
        extras.get::<HttpInfo>().map(Self::from_info)
    }

    fn from_info(info: &HttpInfo) -> Self {
        Self {
            local_addr: info.local_addr(),
            remote_addr: info.remote_addr(),
        }
    }
}

impl ConnectionRegistry {
    fn remember(&self, connection: &Connected) {
        let Some(socket) = SocketIdentity::from_connected(connection) else {
            return;
        };
        let mut record = self.sockets.lock().unwrap_or_else(|e| e.into_inner());
        if record.0.contains(&socket) {
            return;
        }
        if record.0.len() == MAX_TRACKED_CONNECTIONS {
            record.0.remove(0);
            record.1 += 1;
        }
        record.0.push(socket);
    }

    pub(crate) fn snapshot(&self) -> (Vec<SocketIdentity>, u64) {
        self.sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn forget_closed(&self, socket: SocketIdentity) {
        self.sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .retain(|known| *known != socket);
    }
}

tokio::task_local! {
    pub(crate) static ACTIVE_HTTP_TIMING: Arc<HttpTiming>;
}

pub(crate) struct HttpTiming {
    clock: CaptureClock,
    pub(crate) connections: Arc<ConnectionRegistry>,
    record: Mutex<Value>,
    pub(crate) changed: tokio::sync::Notify,
}

impl HttpTiming {
    pub(crate) fn new(clock: CaptureClock, connections: Arc<ConnectionRegistry>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            connections,
            changed: tokio::sync::Notify::new(),
            record: Mutex::new(json!({
                "upload_timing": "unavailable",
                "pool_checkout_timing": "unavailable",
                "connection_attempts": 0,
                "dns_attempts": 0,
                "connections": [],
                "dns": [],
                "scheduler_probe_ticks": 0,
                "max_scheduler_lag_ms": 0.0,
                "connection_assignment_timing": "unavailable",
                "pre_response_socket_attribution": "transport_candidates_only",
            })),
        })
    }

    pub(crate) fn snapshot(&self) -> Value {
        self.record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn confirmed_socket(&self) -> Option<SocketIdentity> {
        let record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        Some(SocketIdentity {
            local_addr: record["local_addr"].as_str()?.parse().ok()?,
            remote_addr: record["remote_addr"].as_str()?.parse().ok()?,
        })
    }

    fn begin(self: &Arc<Self>, kind: &'static str) -> Attempt {
        let mut record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        let counter = if kind == "dns" {
            "dns_attempts"
        } else {
            "connection_attempts"
        };
        record[counter] = json!(record[counter].as_u64().unwrap_or(0) + 1);
        let attempts = record[kind].as_array_mut().expect("timing attempt array");
        let index = (attempts.len() < MAX_ATTEMPTS).then(|| {
            let clock = self.clock.snapshot();
            attempts.push(json!({
                "started_ms": clock.elapsed_ms,
                "started_clock": clock,
                "outcome": "in_progress",
            }));
            attempts.len() - 1
        });
        self.changed.notify_one();
        Attempt {
            timing: self.clone(),
            kind,
            index,
            finished: false,
        }
    }

    pub(crate) fn checkpoints(&self) -> Vec<(&'static str, ClockSnapshot)> {
        let record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        [
            (
                "new_connection_started",
                &record["connections"][0]["started_clock"],
            ),
            (
                "new_connection_established",
                &record["connections"][0]["established_clock"],
            ),
            ("dns_started", &record["dns"][0]["started_clock"]),
            ("dns_finished", &record["dns"][0]["finished_clock"]),
        ]
        .into_iter()
        .filter_map(|(name, value)| {
            serde_json::from_value(value.clone())
                .ok()
                .map(|clock| (name, clock))
        })
        .collect()
    }

    pub(crate) fn response(&self, response: &reqwest::Response) {
        let mut record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        record["http_version"] = json!(format!("{:?}", response.version()));
        record["response_remote_addr"] = json!(response.remote_addr().map(|a| a.to_string()));
        if let Some(info) = response.extensions().get::<HttpInfo>() {
            let local = info.local_addr().to_string();
            let remote = info.remote_addr().to_string();
            let created_here = record["connections"].as_array().is_some_and(|attempts| {
                attempts.iter().any(|attempt| {
                    attempt["local_addr"] == local && attempt["remote_addr"] == remote
                })
            });
            let origin = if created_here {
                "created_for_request"
            } else if record["connection_attempts"].as_u64().unwrap_or(0) > MAX_ATTEMPTS as u64 {
                "unknown"
            } else {
                "pooled_or_shared"
            };
            record["connection_origin"] = json!(origin);
            record["local_addr"] = json!(local);
            record["remote_addr"] = json!(remote);
        }
    }

    pub(crate) fn scheduler_tick(&self, lag: Duration) {
        let mut record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        record["scheduler_probe_ticks"] =
            json!(record["scheduler_probe_ticks"].as_u64().unwrap_or(0) + 1);
        record["max_scheduler_lag_ms"] = json!(record["max_scheduler_lag_ms"]
            .as_f64()
            .unwrap_or(0.0)
            .max(lag.as_secs_f64() * 1000.0));
    }
}

struct Attempt {
    timing: Arc<HttpTiming>,
    kind: &'static str,
    index: Option<usize>,
    finished: bool,
}

impl Attempt {
    fn finish(&mut self, outcome: &str, connection: Option<Connected>) {
        self.finished = true;
        let Some(index) = self.index else { return };
        let mut record = self.timing.record.lock().unwrap_or_else(|e| e.into_inner());
        let attempt = &mut record[self.kind][index];
        let clock = self.timing.clock.snapshot();
        attempt["finished_ms"] = json!(clock.elapsed_ms);
        attempt["finished_clock"] = json!(clock);
        attempt["outcome"] = json!(outcome);
        if let Some(connection) = connection {
            attempt["established_ms"] = json!(clock.elapsed_ms);
            attempt["established_clock"] = json!(clock);
            attempt["negotiated_h2"] = json!(connection.is_negotiated_h2());
            // True only for a plaintext forward proxy, not every CONNECT tunnel.
            attempt["http_forward_proxy"] = json!(connection.is_proxied());
            let mut extras = http::Extensions::new();
            connection.get_extras(&mut extras);
            if let Some(info) = extras.get::<HttpInfo>() {
                attempt["local_addr"] = json!(info.local_addr().to_string());
                attempt["remote_addr"] = json!(info.remote_addr().to_string());
            }
        }
        self.timing.changed.notify_one();
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("dropped", None);
        }
    }
}

#[derive(Clone)]
pub(crate) struct ConnectionTimingLayer(pub(crate) Arc<ConnectionRegistry>);

impl<S> Layer<S> for ConnectionTimingLayer {
    type Service = TimedConnector<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TimedConnector {
            inner,
            connections: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TimedConnector<S> {
    inner: S,
    connections: Arc<ConnectionRegistry>,
}

impl<S, R> Service<R> for TimedConnector<S>
where
    S: Service<R>,
    S::Future: Send + 'static,
    S::Response: Connection + Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let timing = ACTIVE_HTTP_TIMING.try_with(Arc::clone).ok();
        let mut attempt = timing.as_ref().map(|timing| timing.begin("connections"));
        let future = self.inner.call(request);
        let connections = self.connections.clone();
        let future = async move {
            let result = future.await;
            let connected = result.as_ref().ok().map(Connection::connected);
            if let Some(connected) = &connected {
                connections.remember(connected);
            }
            if let Some(attempt) = &mut attempt {
                attempt.finish(
                    if result.is_ok() { "completed" } else { "error" },
                    connected,
                );
            }
            result
        };
        match timing {
            Some(timing) => Box::pin(ACTIVE_HTTP_TIMING.scope(timing, future)),
            None => Box::pin(future),
        }
    }
}

/// Delegate to the same GAI resolver reqwest uses by default, with no cache or
/// address-order changes. DNS remains inside the connector's timeout.
pub(crate) struct TimedResolver;

impl Resolve for TimedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let mut attempt = ACTIVE_HTTP_TIMING
            .try_with(|timing| timing.begin("dns"))
            .ok();
        Box::pin(async move {
            let mut resolver = hyper_util::client::legacy::connect::dns::GaiResolver::new();
            let result = match name.as_str().parse() {
                Ok(name) => resolver
                    .call(name)
                    .await
                    .map(|addresses| Box::new(addresses) as Addrs)
                    .map_err(Into::into),
                Err(error) => Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            };
            if let Some(attempt) = &mut attempt {
                attempt.finish(if result.is_ok() { "completed" } else { "error" }, None);
            }
            result
        })
    }
}
