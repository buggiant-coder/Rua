use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iroh_docs::AuthorId;
use serde::{Deserialize, Serialize};

const MAX_RECENT_SAMPLES: usize = 64;
const MAX_PENDING_ENTRIES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncSourceTiming {
    pub commit_observed_at_ms: u64,
    pub publication_started_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStageStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncFailureStage {
    Publication,
    Transport,
    InboundInbox,
    Content,
    DecryptRead,
    DomainApply,
    CacheRefresh,
    CanonicalVisibility,
    RepairScan,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncPublicationStageSample {
    pub correlation_id: String,
    pub commit_observed_at_ms: u64,
    pub publication_started_at_ms: u64,
    pub publication_completed_at_ms: u64,
    pub commit_to_publication_ms: u64,
    pub publication_ms: u64,
    pub status: SyncStageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_cause: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncInboundStageSample {
    pub correlation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_commit_observed_at_ms: Option<u64>,
    pub transport_arrival_at_ms: u64,
    pub inbox_recorded_at_ms: u64,
    pub content_ready_at_ms: u64,
    pub decrypt_read_completed_at_ms: u64,
    pub domain_apply_completed_at_ms: u64,
    pub cache_refresh_completed_at_ms: u64,
    pub canonical_visible_at_ms: u64,
    pub transport_to_inbox_ms: u64,
    pub inbox_wait_ms: u64,
    pub content_wait_ms: u64,
    pub content_read_ms: u64,
    pub decrypt_ms: u64,
    pub domain_apply_ms: u64,
    pub cache_refresh_ms: u64,
    pub canonical_visibility_ms: u64,
    pub receiver_pipeline_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_to_end_wall_ms: Option<u64>,
    pub used_peer_download: bool,
    pub status: SyncStageStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncStageFailure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_fingerprint: Option<String>,
    pub stage: SyncFailureStage,
    pub cause: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncNowLocalCompletionStatus {
    NotStarted,
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncNowStageSample {
    pub completed_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_peer_node_id: Option<String>,
    pub transport_status: SyncStageStatus,
    pub local_completion_status: SyncNowLocalCompletionStatus,
    pub transport_duration_ms: u64,
    pub local_apply_duration_ms: u64,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<SyncFailureStage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncLatencySummary {
    pub samples: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SyncStageDiagnosticsSnapshot {
    pub recent_publications: Vec<SyncPublicationStageSample>,
    pub recent_inbound: Vec<SyncInboundStageSample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_pipeline: Option<SyncLatencySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_to_end_wall: Option<SyncLatencySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<SyncStageFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_now: Option<SyncNowStageSample>,
}

#[derive(Clone)]
struct PendingInboundStage {
    transport_arrival_at_ms: u64,
    transport_arrival: Instant,
    inbox_recorded_at_ms: Option<u64>,
    inbox_recorded: Option<Instant>,
}

#[derive(Clone)]
pub struct InboundStageStart {
    pub transport_arrival_at_ms: u64,
    pub inbox_recorded_at_ms: u64,
    transport_arrival: Instant,
    inbox_recorded: Instant,
}

impl InboundStageStart {
    pub fn transport_to_inbox_ms(&self) -> u64 {
        millis(
            self.inbox_recorded
                .checked_duration_since(self.transport_arrival)
                .unwrap_or_default(),
        )
    }

    pub fn inbox_wait_ms(&self, read_started: Instant) -> u64 {
        millis(
            read_started
                .checked_duration_since(self.inbox_recorded)
                .unwrap_or_default(),
        )
    }

    pub fn receiver_pipeline_ms(&self) -> u64 {
        millis(self.transport_arrival.elapsed())
    }
}

#[derive(Default)]
struct SyncStageState {
    recent_publications: VecDeque<SyncPublicationStageSample>,
    recent_inbound: VecDeque<SyncInboundStageSample>,
    pending_inbound: HashMap<EntryIdentity, PendingInboundStage>,
    last_failure: Option<SyncStageFailure>,
    last_sync_now: Option<SyncNowStageSample>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct EntryIdentity {
    author: [u8; 32],
    key: Vec<u8>,
}

#[derive(Default)]
pub struct SyncStageRecorder {
    state: Mutex<SyncStageState>,
}

impl SyncStageRecorder {
    pub fn record_transport_arrival(&self, author: AuthorId, key: &[u8]) {
        let identity = entry_identity(author, key);
        let mut state = self.state();
        if !state.pending_inbound.contains_key(&identity)
            && state.pending_inbound.len() >= MAX_PENDING_ENTRIES
        {
            if let Some(oldest) = state
                .pending_inbound
                .iter()
                .min_by_key(|(_, timing)| timing.transport_arrival_at_ms)
                .map(|(identity, _)| identity.clone())
            {
                state.pending_inbound.remove(&oldest);
            }
        }
        state.pending_inbound.insert(
            identity,
            PendingInboundStage {
                transport_arrival_at_ms: wall_now_ms(),
                transport_arrival: Instant::now(),
                inbox_recorded_at_ms: None,
                inbox_recorded: None,
            },
        );
    }

    pub fn record_inbox_recorded(&self, author: AuthorId, key: &[u8]) {
        let identity = entry_identity(author, key);
        let now_ms = wall_now_ms();
        let now = Instant::now();
        let mut state = self.state();
        let pending = state
            .pending_inbound
            .entry(identity)
            .or_insert(PendingInboundStage {
                transport_arrival_at_ms: now_ms,
                transport_arrival: now,
                inbox_recorded_at_ms: None,
                inbox_recorded: None,
            });
        pending.inbox_recorded_at_ms = Some(now_ms);
        pending.inbox_recorded = Some(now);
    }

    pub fn inbound_stage_start(
        &self,
        author: AuthorId,
        key: &[u8],
        fallback_seen_at_ms: u64,
    ) -> InboundStageStart {
        let state = self.state();
        if let Some(pending) = state.pending_inbound.get(&entry_identity(author, key)) {
            let inbox_recorded_at_ms = pending
                .inbox_recorded_at_ms
                .unwrap_or(pending.transport_arrival_at_ms);
            let inbox_recorded = pending.inbox_recorded.unwrap_or(pending.transport_arrival);
            return InboundStageStart {
                transport_arrival_at_ms: pending.transport_arrival_at_ms,
                inbox_recorded_at_ms,
                transport_arrival: pending.transport_arrival,
                inbox_recorded,
            };
        }
        let now = Instant::now();
        InboundStageStart {
            transport_arrival_at_ms: fallback_seen_at_ms,
            inbox_recorded_at_ms: fallback_seen_at_ms,
            transport_arrival: now,
            inbox_recorded: now,
        }
    }

    pub fn clear_inbound_entry(&self, author: AuthorId, key: &[u8]) {
        self.state()
            .pending_inbound
            .remove(&entry_identity(author, key));
    }

    pub fn record_publication(&self, sample: SyncPublicationStageSample) {
        let mut state = self.state();
        push_bounded(&mut state.recent_publications, sample);
    }

    pub fn record_inbound(&self, sample: SyncInboundStageSample) {
        let mut state = self.state();
        push_bounded(&mut state.recent_inbound, sample);
    }

    pub fn record_failure(&self, failure: SyncStageFailure) {
        self.state().last_failure = Some(failure);
    }

    pub fn record_sync_now(&self, sample: SyncNowStageSample) {
        self.state().last_sync_now = Some(sample);
    }

    pub fn snapshot(&self) -> SyncStageDiagnosticsSnapshot {
        let state = self.state();
        let recent_publications = state.recent_publications.iter().rev().cloned().collect();
        let recent_inbound = state
            .recent_inbound
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let receiver_pipeline = latency_summary(
            state
                .recent_inbound
                .iter()
                .map(|sample| sample.receiver_pipeline_ms)
                .collect(),
        );
        let end_to_end_wall = latency_summary(
            state
                .recent_inbound
                .iter()
                .filter_map(|sample| sample.end_to_end_wall_ms)
                .collect(),
        );
        SyncStageDiagnosticsSnapshot {
            recent_publications,
            recent_inbound,
            receiver_pipeline,
            end_to_end_wall,
            last_failure: state.last_failure.clone(),
            last_sync_now: state.last_sync_now.clone(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SyncStageState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub fn entry_fingerprint(author: AuthorId, key: &[u8]) -> String {
    let mut input = Vec::with_capacity(author.as_bytes().len() + key.len());
    input.extend_from_slice(author.as_bytes());
    input.extend_from_slice(key);
    blake3::hash(&input).to_hex()[..16].to_string()
}

pub fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn entry_identity(author: AuthorId, key: &[u8]) -> EntryIdentity {
    EntryIdentity {
        author: *author.as_bytes(),
        key: key.to_vec(),
    }
}

fn push_bounded<T>(samples: &mut VecDeque<T>, sample: T) {
    if samples.len() == MAX_RECENT_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn latency_summary(mut values: Vec<u64>) -> Option<SyncLatencySummary> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(SyncLatencySummary {
        samples: values.len(),
        p50_ms: percentile(&values, 50),
        p95_ms: percentile(&values, 95),
        max_ms: values.last().copied().unwrap_or_default(),
    })
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let rank = values.len().saturating_mul(percentile).saturating_add(99) / 100;
    values[rank.saturating_sub(1).min(values.len() - 1)]
}
