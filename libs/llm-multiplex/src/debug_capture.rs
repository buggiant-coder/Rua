use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_types::ChatRequest;
use rua_core::ChatAddress;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

pub(crate) mod clock;
mod network;
mod timing;
mod writer;
pub use timing::CaptureTrace;
pub use writer::flush_capture;
use writer::CaptureWriter;

pub const MODEL_CALL_ID_METADATA_KEY: &str = "rua_model_call_id";
pub const CHAT_ADDRESS_METADATA_KEY: &str = "security_chat_address";
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureTarget {
    pub capture_id: String,
    pub capture_path: PathBuf,
    pub model_call_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCapture {
    pub capture_id: String,
    pub capture_path: PathBuf,
}

struct CaptureRegistration {
    capture: ActiveCapture,
    cancel: CancellationToken,
    writer: Arc<CaptureWriter>,
}

fn active_captures() -> &'static RwLock<HashMap<ChatAddress, CaptureRegistration>> {
    static ACTIVE: OnceLock<RwLock<HashMap<ChatAddress, CaptureRegistration>>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn register_capture(address: ChatAddress, capture: ActiveCapture) {
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return;
    }
    if active_captures()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&address)
        .is_some_and(|old| old.capture == capture)
    {
        return;
    }
    let writer = match CaptureWriter::start(capture.capture_path.clone()) {
        Ok(writer) => writer,
        Err(error) => {
            record_result(Err(error));
            return;
        }
    };
    // Starting/recovering a worker may drain a previously closed writer. Never
    // hold the registry lock across that wait: model calls resolve here too.
    let mut active = active_captures().write().unwrap_or_else(|e| e.into_inner());
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return;
    }
    if active
        .get(&address)
        .is_some_and(|old| old.capture == capture)
    {
        return;
    }
    if let Some(old) = active.insert(
        address,
        CaptureRegistration {
            capture,
            cancel: CancellationToken::new(),
            writer,
        },
    ) {
        old.cancel.cancel();
    }
}

pub fn active_capture(address: &ChatAddress) -> Option<ActiveCapture> {
    active_captures()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(address)
        .map(|registered| registered.capture.clone())
}

pub fn unregister_capture(address: &ChatAddress) {
    if let Some(registered) = active_captures()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(address)
    {
        registered.cancel.cancel();
    }
}

/// Cancel diagnostic samplers and drain accepted writes after model producers stop.
/// Samplers finish first; only the disk drain uses the shutdown blocking pool.
pub async fn shutdown_capture_writers(timeout: std::time::Duration) -> io::Result<()> {
    SHUTTING_DOWN.store(true, Ordering::Release);
    let deadline = tokio::time::Instant::now() + timeout;
    let captures =
        std::mem::take(&mut *active_captures().write().unwrap_or_else(|e| e.into_inner()));
    for registered in captures.values() {
        registered.cancel.cancel();
    }
    drop(captures);
    let samplers = writer::stop_samplers(deadline).await;
    let writes = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            writer::shutdown_capture_writers(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
        }),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LLM capture shutdown timed out"))?
    .map_err(io::Error::other)?;
    samplers.and(writes)
}

impl CaptureTarget {
    pub fn from_metadata(metadata: &Map<String, Value>) -> Option<Self> {
        Self::resolve(metadata).map(|(target, _, _)| target)
    }

    fn resolve(
        metadata: &Map<String, Value>,
    ) -> Option<(Self, CancellationToken, Arc<CaptureWriter>)> {
        let address =
            serde_json::from_value::<ChatAddress>(metadata.get(CHAT_ADDRESS_METADATA_KEY)?.clone())
                .ok()?;
        let active = active_captures()
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let registered = active.get(&address)?;
        Some((
            Self {
                capture_id: registered.capture.capture_id.clone(),
                capture_path: registered.capture.capture_path.clone(),
                model_call_id: metadata
                    .get(MODEL_CALL_ID_METADATA_KEY)?
                    .as_str()?
                    .to_owned(),
            },
            registered.cancel.clone(),
            registered.writer.clone(),
        ))
    }

    pub fn call_dir(&self) -> PathBuf {
        self.capture_path.join("calls").join(&self.model_call_id)
    }
}

/// Returns whether the artifact was queued, not whether it has reached disk.
/// Use `flush_capture` at an explicit export/test/stop boundary to wait for durability.
pub fn record_named_json<T: Serialize>(
    metadata: &Map<String, Value>,
    filename: &str,
    value: &T,
) -> bool {
    let Some((target, _, writer)) = CaptureTarget::resolve(metadata) else {
        return false;
    };
    writer.record(target.call_dir().join(filename), value, false)
}

pub fn record_logical_request(request: &ChatRequest) {
    record_named_json(&request.metadata, "logical-request.json", request);
    record_named_json(
        &request.metadata,
        "call.json",
        &serde_json::json!({
            "model_call_id": request.metadata.get(MODEL_CALL_ID_METADATA_KEY),
            "chat_address": request.metadata.get(CHAT_ADDRESS_METADATA_KEY),
        }),
    );
    record_projection_placeholder(request);
}

fn record_projection_placeholder(request: &ChatRequest) {
    let Some((target, _, writer)) = CaptureTarget::resolve(&request.metadata) else {
        return;
    };
    let path = target.call_dir().join("projection.json");
    writer.record(
        path,
        &serde_json::json!({
            "status": "unavailable",
            "reason": "context_projection_evidence_not_recorded",
        }),
        true,
    );
}

pub fn record_provider_request<T: Serialize>(
    metadata: &Map<String, Value>,
    provider: &str,
    endpoint: &str,
    body: &T,
) -> bool {
    record_provider_artifact(metadata, "request", provider, endpoint, body)
}

pub fn record_provider_response<T: Serialize>(
    metadata: &Map<String, Value>,
    provider: &str,
    endpoint: &str,
    body: &T,
) -> bool {
    record_provider_artifact(metadata, "response", provider, endpoint, body)
}

fn record_provider_artifact<T: Serialize>(
    metadata: &Map<String, Value>,
    kind: &str,
    provider: &str,
    endpoint: &str,
    body: &T,
) -> bool {
    let Some((target, _, writer)) = CaptureTarget::resolve(metadata) else {
        return false;
    };
    write_provider_artifact(&target, &writer, kind, provider, endpoint, body).is_some()
}

fn write_provider_artifact<T: Serialize>(
    target: &CaptureTarget,
    writer: &CaptureWriter,
    kind: &str,
    provider: &str,
    endpoint: &str,
    body: &T,
) -> Option<PathBuf> {
    // Borrow the body rather than cloning the complete long request into a Value.
    #[derive(Serialize)]
    struct Payload<'a, T> {
        provider: &'a str,
        endpoint: &'a str,
        body: &'a T,
    }
    // Enqueued files need not exist yet. Use monotonic process-local nanoseconds
    // instead of a filesystem existence check to distinguish concurrent attempts.
    static LAST_ARTIFACT_NS: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let previous = LAST_ARTIFACT_NS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(now.max(last + 1))
        })
        .unwrap();
    let nanos = now.max(previous + 1);
    let path = target.call_dir().join(format!(
        "provider-{kind}-{}.{:09}.json",
        nanos / 1_000_000_000,
        nanos % 1_000_000_000,
    ));
    writer
        .record(
            path.clone(),
            &Payload {
                provider,
                endpoint,
                body,
            },
            false,
        )
        .then_some(path)
}

fn record_result(result: io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "failed to write local LLM debug capture artifact");
            false
        }
    }
}
