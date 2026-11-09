use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use rua_core::{ChatAddress, ConversationActivityPosition, RunControl};
use rua_device_network::{
    DeviceSessionError, DeviceSessionFrameStream, DeviceSessionLiveActivityHandler,
    DeviceSessionManager,
};
use serde::{Deserialize, Serialize};

use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::foreground_operation::{ForegroundOperationKind, ForegroundOperationRegistry};

const MAX_COMPLETED_REPLAY_FRAMES: usize = 512;
const CONVERSATION_REPLAY_MAX_FRAMES: usize = 4_096;
const CONVERSATION_REPLAY_MAX_BYTES: usize = 4 * 1024 * 1024;
const CONVERSATION_CHECKPOINT_MAX_BYTES: usize = 8 * 1024 * 1024;
const REPLAY_SNAPSHOT_CHUNK_BYTES: usize = 256 * 1024;
const ACTIVE_STREAM_BROADCAST_RECONNECT_ATTEMPTS: usize = 3;
const ACTIVE_STREAM_BROADCAST_RECONNECT_BACKOFF: Duration = Duration::from_millis(100);
const VIEWER_LEASE_TOUCH_INTERVAL: Duration = Duration::from_secs(60);
type ActiveStreamBroadcastFrameTx = tokio::sync::mpsc::UnboundedSender<ActiveStreamBroadcastFrame>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActiveStreamOwner {
    Chat { address: ChatAddress },
    Terminal { session_id: String },
}

impl ActiveStreamOwner {
    pub fn chat(address: ChatAddress) -> Self {
        Self::Chat { address }
    }

    pub fn terminal(session_id: impl Into<String>) -> Self {
        Self::Terminal {
            session_id: session_id.into(),
        }
    }

    pub fn as_chat_address(&self) -> Option<&ChatAddress> {
        match self {
            Self::Chat { address } => Some(address),
            Self::Terminal { .. } => None,
        }
    }

    pub fn terminal_session_id(&self) -> Option<&str> {
        match self {
            Self::Chat { .. } => None,
            Self::Terminal { session_id } => Some(session_id),
        }
    }
}

impl fmt::Display for ActiveStreamOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chat { address } => write!(formatter, "{address}"),
            Self::Terminal { session_id } => write!(formatter, "terminal:{session_id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveStreamBroadcastViewer {
    pub source_device_id: String,
    pub viewer_id: String,
    pub kind: ForegroundOperationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveStreamBroadcastSubscribeRequest {
    #[serde(default)]
    pub device_delivery_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ActiveStreamOwner>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub after_seq_by_stream: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_activity_position: Option<ConversationActivityPosition>,
    #[serde(default)]
    pub collapse_replay: bool,
    pub initial_conversation_control: bool,
    pub authority_epoch: Option<ConversationAuthorityEpoch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<ActiveStreamBroadcastViewer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActiveStreamBroadcastFrame {
    Started {
        device_delivery_id: String,
        owner: ActiveStreamOwner,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority_epoch: Option<ConversationAuthorityEpoch>,
        seq: u64,
    },
    Event {
        device_delivery_id: String,
        owner: ActiveStreamOwner,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority_epoch: Option<ConversationAuthorityEpoch>,
        seq: u64,
        payload: String,
    },
    Done {
        device_delivery_id: String,
        owner: ActiveStreamOwner,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority_epoch: Option<ConversationAuthorityEpoch>,
        seq: u64,
    },
    Error {
        device_delivery_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<ActiveStreamOwner>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority_epoch: Option<ConversationAuthorityEpoch>,
        seq: u64,
        detail: String,
    },
    AuthorityRedirect {
        device_delivery_id: String,
        address: ChatAddress,
        placement: ConversationAuthorityPlacement,
        seq: u64,
    },
    Gap {
        device_delivery_id: String,
        requested_after_seq: u64,
        first_available_seq: u64,
    },
}

impl ActiveStreamBroadcastFrame {
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Started { seq, .. }
            | Self::Event { seq, .. }
            | Self::Done { seq, .. }
            | Self::Error { seq, .. }
            | Self::AuthorityRedirect { seq, .. } => Some(*seq),
            Self::Gap { .. } => None,
        }
    }

    pub fn stream_id(&self) -> &str {
        match self {
            Self::Started {
                device_delivery_id, ..
            }
            | Self::Event {
                device_delivery_id, ..
            }
            | Self::Done {
                device_delivery_id, ..
            }
            | Self::Error {
                device_delivery_id, ..
            }
            | Self::AuthorityRedirect {
                device_delivery_id, ..
            }
            | Self::Gap {
                device_delivery_id, ..
            } => device_delivery_id,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done { .. }
                | Self::Error { .. }
                | Self::AuthorityRedirect { .. }
                | Self::Gap { .. }
        )
    }

    pub fn is_authority_redirect(&self) -> bool {
        matches!(self, Self::AuthorityRedirect { .. })
    }
}

fn replay_event_value(payload: String) -> serde_json::Value {
    serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload))
}

fn with_conversation_activity_metadata(
    payload: String,
    generation: &str,
    sequence: Option<u64>,
    snapshot: bool,
) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&payload) else {
        return payload;
    };
    let Some(object) = value.as_object_mut() else {
        return payload;
    };
    object.insert(
        "activity_generation".to_string(),
        serde_json::Value::String(generation.to_string()),
    );
    if let Some(sequence) = sequence {
        object.insert(
            "activity_sequence".to_string(),
            serde_json::Value::Number(sequence.into()),
        );
    }
    if snapshot {
        object.insert(
            "activity_snapshot".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    value.to_string()
}

fn conversation_activity_position(payload: &str) -> Option<(String, u64)> {
    let value = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    Some((
        value.get("activity_generation")?.as_str()?.to_string(),
        value.get("activity_sequence")?.as_u64()?,
    ))
}

fn conversation_activity_generation(payload: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("activity_generation")?
        .as_str()
        .map(str::to_string)
}

fn conversation_replay_sort_key(frame: &ActiveStreamBroadcastFrame) -> (u8, u64, u64) {
    match frame {
        ActiveStreamBroadcastFrame::Gap { .. } => (0, 0, 0),
        ActiveStreamBroadcastFrame::Started { seq, .. } => (0, 0, *seq),
        ActiveStreamBroadcastFrame::Event { payload, seq, .. } => {
            match conversation_activity_position(payload) {
                Some((_, activity_sequence)) => (1, activity_sequence, *seq),
                None => (2, *seq, *seq),
            }
        }
        ActiveStreamBroadcastFrame::Done { seq, .. }
        | ActiveStreamBroadcastFrame::Error { seq, .. }
        | ActiveStreamBroadcastFrame::AuthorityRedirect { seq, .. } => (3, *seq, *seq),
    }
}

fn replay_snapshot_payload(events: Vec<String>, complete: bool) -> String {
    serde_json::json!({
        "type": "stream_replay",
        "events": events.into_iter().map(replay_event_value).collect::<Vec<_>>(),
        "complete": complete,
    })
    .to_string()
}

pub(crate) fn stream_replay_payloads(events: Vec<String>) -> Vec<String> {
    if events.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::<Vec<String>>::new();
    let mut current = Vec::new();
    let mut current_bytes = 0usize;
    for event in events {
        if !current.is_empty()
            && current_bytes.saturating_add(event.len()) > REPLAY_SNAPSHOT_CHUNK_BYTES
        {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(event.len());
        current.push(event);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, events)| replay_snapshot_payload(events, index + 1 == chunk_count))
        .collect()
}

pub(crate) fn collapse_replay_frames(
    frames: Vec<ActiveStreamBroadcastFrame>,
) -> Vec<ActiveStreamBroadcastFrame> {
    let mut collapsed = Vec::new();
    let mut events = Vec::new();
    for frame in frames {
        if matches!(frame, ActiveStreamBroadcastFrame::Event { .. }) {
            events.push(frame);
            continue;
        }
        collapsed.extend(collapse_replay_event_frames(std::mem::take(&mut events)));
        collapsed.push(frame);
    }
    collapsed.extend(collapse_replay_event_frames(events));
    collapsed
}

fn collapse_replay_event_frames(
    frames: Vec<ActiveStreamBroadcastFrame>,
) -> Vec<ActiveStreamBroadcastFrame> {
    let mut groups = Vec::<Vec<ActiveStreamBroadcastFrame>>::new();
    let mut current = Vec::<ActiveStreamBroadcastFrame>::new();
    let mut current_bytes = 0usize;
    for frame in frames {
        let (stream_id, payload_bytes) = match &frame {
            ActiveStreamBroadcastFrame::Event {
                device_delivery_id,
                payload,
                ..
            } => (device_delivery_id.as_str(), payload.len()),
            _ => unreachable!("non-event replay frames returned before grouping"),
        };
        let stream_changed = current
            .first()
            .is_some_and(|current_frame| current_frame.stream_id() != stream_id);
        if !current.is_empty()
            && (stream_changed
                || current_bytes.saturating_add(payload_bytes) > REPLAY_SNAPSHOT_CHUNK_BYTES)
        {
            groups.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(payload_bytes);
        current.push(frame);
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let group_count = groups.len();
    groups
        .into_iter()
        .enumerate()
        .filter_map(|(index, group)| {
            let last = group.last()?.clone();
            let activity_position = match &last {
                ActiveStreamBroadcastFrame::Event { payload, .. } => {
                    conversation_activity_position(payload)
                }
                _ => None,
            };
            let events = group
                .into_iter()
                .filter_map(|frame| match frame {
                    ActiveStreamBroadcastFrame::Event { payload, .. } => Some(payload),
                    _ => None,
                })
                .collect();
            let ActiveStreamBroadcastFrame::Event {
                device_delivery_id,
                owner,
                authority_epoch,
                seq,
                ..
            } = last
            else {
                return None;
            };
            let payload = replay_snapshot_payload(events, index + 1 == group_count);
            let payload = match activity_position {
                Some((generation, sequence)) => {
                    with_conversation_activity_metadata(payload, &generation, Some(sequence), false)
                }
                None => payload,
            };
            Some(ActiveStreamBroadcastFrame::Event {
                device_delivery_id,
                owner,
                authority_epoch,
                seq,
                payload,
            })
        })
        .collect()
}

pub struct ActiveStreamBroadcastSubscription {
    pub replay: Vec<ActiveStreamBroadcastFrame>,
    pub rx: Option<tokio::sync::mpsc::UnboundedReceiver<ActiveStreamBroadcastFrame>>,
}

struct ActiveStreamBroadcastEntry {
    owner: Option<ActiveStreamOwner>,
    authority_epoch: Option<ConversationAuthorityEpoch>,
    next_seq: u64,
    first_available_seq: u64,
    completed: bool,
    replay_bytes: usize,
    replay_lines: usize,
    frame_replay_lines: VecDeque<usize>,
    frames: VecDeque<ActiveStreamBroadcastFrame>,
    subscribers: Vec<tokio::sync::mpsc::UnboundedSender<ActiveStreamBroadcastFrame>>,
}

#[derive(Clone, Copy, Default)]
struct ActiveReplayPolicy {
    max_bytes: Option<usize>,
    frame_lines: usize,
    max_lines: Option<usize>,
}

impl ActiveStreamBroadcastEntry {
    fn new() -> Self {
        Self {
            owner: None,
            authority_epoch: None,
            next_seq: 1,
            first_available_seq: 1,
            completed: false,
            replay_bytes: 0,
            replay_lines: 0,
            frame_replay_lines: VecDeque::new(),
            frames: VecDeque::new(),
            subscribers: Vec::new(),
        }
    }
}

pub struct ActiveStreamBroadcastHub {
    navigation_activity:
        std::sync::RwLock<Option<Arc<super::chat_activity_wake::ChatActivityWakeHub>>>,
    entries: tokio::sync::RwLock<HashMap<String, ActiveStreamBroadcastEntry>>,
    owner_watchers:
        tokio::sync::RwLock<HashMap<ActiveStreamOwner, Vec<ActiveStreamBroadcastFrameTx>>>,
    agent_watchers: tokio::sync::RwLock<HashMap<String, Vec<ActiveStreamBroadcastFrameTx>>>,
    global_watchers: tokio::sync::RwLock<Vec<ActiveStreamBroadcastFrameTx>>,
    activity_generation: String,
    conversation_timelines: tokio::sync::RwLock<
        HashMap<ChatAddress, Arc<tokio::sync::Mutex<ConversationActivityTimeline>>>,
    >,
}

#[derive(Debug)]
struct ConversationActivityTimeline {
    next_sequence: u64,
    first_available_sequence: u64,
    replay_bytes: usize,
    frames: VecDeque<ActiveStreamBroadcastFrame>,
    checkpoint_frames: VecDeque<ActiveStreamBroadcastFrame>,
    checkpoint_bytes: usize,
    checkpoint_available: bool,
}

fn conversation_event_type(frame: &ActiveStreamBroadcastFrame) -> Option<String> {
    let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
        return None;
    };
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value.get("type")?.as_str().map(str::to_owned)
}

fn checkpoint_frame(frame: &ActiveStreamBroadcastFrame) -> ActiveStreamBroadcastFrame {
    let ActiveStreamBroadcastFrame::Event {
        device_delivery_id,
        owner,
        authority_epoch,
        seq,
        payload,
    } = frame
    else {
        return frame.clone();
    };
    let payload = serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|mut value| {
            value.as_object_mut()?.insert(
                "activity_checkpoint".to_string(),
                serde_json::Value::Bool(true),
            );
            serde_json::to_string(&value).ok()
        })
        .unwrap_or_else(|| payload.clone());
    ActiveStreamBroadcastFrame::Event {
        device_delivery_id: device_delivery_id.clone(),
        owner: owner.clone(),
        authority_epoch: *authority_epoch,
        seq: *seq,
        payload,
    }
}

fn requested_conversation_activity_sequence(
    timeline: &ConversationActivityTimeline,
    after_seq_by_stream: &BTreeMap<String, u64>,
) -> u64 {
    timeline
        .frames
        .iter()
        .filter_map(|frame| {
            let stream_sequence = frame.seq()?;
            if stream_sequence
                > after_seq_by_stream
                    .get(frame.stream_id())
                    .copied()
                    .unwrap_or(0)
            {
                return None;
            }
            let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
                return None;
            };
            conversation_activity_position(payload).map(|(_, sequence)| sequence)
        })
        .max()
        .unwrap_or(0)
}

fn push_conversation_timeline_frame(
    timeline: &mut ConversationActivityTimeline,
    frame: ActiveStreamBroadcastFrame,
) {
    timeline.replay_bytes = timeline
        .replay_bytes
        .saturating_add(frame_replay_bytes(&frame));
    timeline.frames.push_back(frame);
    while (timeline.frames.len() > CONVERSATION_REPLAY_MAX_FRAMES
        || timeline.replay_bytes > CONVERSATION_REPLAY_MAX_BYTES)
        && timeline.frames.len() > 1
    {
        let Some(removed) = timeline.frames.pop_front() else {
            break;
        };
        timeline.replay_bytes = timeline
            .replay_bytes
            .saturating_sub(frame_replay_bytes(&removed));
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = removed {
            if let Some((_, sequence)) = conversation_activity_position(&payload) {
                timeline.first_available_sequence = timeline
                    .first_available_sequence
                    .max(sequence.saturating_add(1));
            }
        }
    }
}

impl Default for ActiveStreamBroadcastHub {
    fn default() -> Self {
        Self {
            navigation_activity: std::sync::RwLock::new(None),
            entries: tokio::sync::RwLock::new(HashMap::new()),
            owner_watchers: tokio::sync::RwLock::new(HashMap::new()),
            agent_watchers: tokio::sync::RwLock::new(HashMap::new()),
            global_watchers: tokio::sync::RwLock::new(Vec::new()),
            activity_generation: uuid::Uuid::new_v4().to_string(),
            conversation_timelines: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl ActiveStreamBroadcastHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_navigation_activity_hub(
        &self,
        hub: Arc<super::chat_activity_wake::ChatActivityWakeHub>,
    ) {
        *self
            .navigation_activity
            .write()
            .expect("navigation activity hub lock poisoned") = Some(hub);
    }

    pub fn activity_generation(&self) -> &str {
        &self.activity_generation
    }

    pub fn conversation_activity_snapshot(&self, payload: String) -> String {
        with_conversation_activity_metadata(payload, &self.activity_generation, None, true)
    }

    async fn conversation_timeline(
        &self,
        address: &ChatAddress,
    ) -> Arc<tokio::sync::Mutex<ConversationActivityTimeline>> {
        if let Some(timeline) = self.conversation_timelines.read().await.get(address) {
            return Arc::clone(timeline);
        }
        let mut timelines = self.conversation_timelines.write().await;
        Arc::clone(timelines.entry(address.clone()).or_insert_with(|| {
            Arc::new(tokio::sync::Mutex::new(ConversationActivityTimeline {
                next_sequence: 1,
                first_available_sequence: 1,
                replay_bytes: 0,
                frames: VecDeque::new(),
                checkpoint_frames: VecDeque::new(),
                checkpoint_bytes: 0,
                checkpoint_available: true,
            }))
        }))
    }

    pub async fn subscribe(
        &self,
        device_delivery_id: &str,
        after_seq: u64,
    ) -> ActiveStreamBroadcastSubscription {
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(device_delivery_id.to_string())
            .or_insert_with(ActiveStreamBroadcastEntry::new);
        subscribe_entry(device_delivery_id, entry, after_seq)
    }

    pub async fn subscribe_chat(
        &self,
        address: &ChatAddress,
        after_seq_by_stream: &BTreeMap<String, u64>,
    ) -> ActiveStreamBroadcastSubscription {
        self.subscribe_owner(
            &ActiveStreamOwner::chat(address.clone()),
            after_seq_by_stream,
            None,
        )
        .await
    }

    pub async fn subscribe_chat_from_activity_position(
        &self,
        address: &ChatAddress,
        position: &ConversationActivityPosition,
    ) -> ActiveStreamBroadcastSubscription {
        self.subscribe_owner(
            &ActiveStreamOwner::chat(address.clone()),
            &BTreeMap::new(),
            Some(position),
        )
        .await
    }

    pub async fn subscribe_terminal(
        &self,
        session_id: &str,
        after_seq_by_stream: &BTreeMap<String, u64>,
    ) -> ActiveStreamBroadcastSubscription {
        self.subscribe_owner(
            &ActiveStreamOwner::terminal(session_id),
            after_seq_by_stream,
            None,
        )
        .await
    }

    async fn subscribe_owner(
        &self,
        owner: &ActiveStreamOwner,
        after_seq_by_stream: &BTreeMap<String, u64>,
        activity_position: Option<&ConversationActivityPosition>,
    ) -> ActiveStreamBroadcastSubscription {
        let timeline = match owner.as_chat_address() {
            Some(address) => Some(self.conversation_timeline(address).await),
            None => None,
        };
        let timeline_guard = match timeline.as_ref() {
            Some(timeline) => Some(timeline.lock().await),
            None => None,
        };
        let mut replay: Vec<ActiveStreamBroadcastFrame> =
            if let Some(timeline) = timeline_guard.as_ref() {
                let positioned_next_sequence = activity_position
                    .filter(|position| position.activity_generation == self.activity_generation)
                    .map(|position| position.next_activity_sequence);
                let requested_after_seq = positioned_next_sequence
                    .map(|next_sequence| next_sequence.saturating_sub(1))
                    .unwrap_or_else(|| {
                        requested_conversation_activity_sequence(timeline, after_seq_by_stream)
                    });
                let mut replay = timeline
                    .frames
                    .iter()
                    .filter(|frame| {
                        if let Some(next_sequence) = positioned_next_sequence {
                            return matches!(
                                frame,
                                ActiveStreamBroadcastFrame::Event { payload, .. }
                                    if conversation_activity_position(payload)
                                        .is_some_and(|(_, sequence)| sequence >= next_sequence)
                            );
                        }
                        frame.seq().is_some_and(|seq| {
                            seq > after_seq_by_stream
                                .get(frame.stream_id())
                                .copied()
                                .unwrap_or(0)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if requested_after_seq.saturating_add(1) < timeline.first_available_sequence {
                    let mut checkpoint = if timeline.checkpoint_available {
                        timeline
                            .checkpoint_frames
                            .iter()
                            .map(checkpoint_frame)
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    checkpoint.insert(
                        0,
                        ActiveStreamBroadcastFrame::Gap {
                            device_delivery_id: String::new(),
                            requested_after_seq,
                            first_available_seq: timeline.first_available_sequence,
                        },
                    );
                    replay = checkpoint;
                }
                replay
            } else {
                let entries = self.entries.read().await;
                entries
                    .iter()
                    .filter(|(_, entry)| !entry.completed && entry.owner.as_ref() == Some(owner))
                    .flat_map(|(stream_id, entry)| {
                        replay_entry(
                            stream_id,
                            entry,
                            after_seq_by_stream.get(stream_id).copied().unwrap_or(0),
                        )
                    })
                    .collect()
            };
        replay.sort_by_key(conversation_replay_sort_key);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.owner_watchers
            .write()
            .await
            .entry(owner.clone())
            .or_default()
            .push(tx);
        ActiveStreamBroadcastSubscription {
            replay,
            rx: Some(rx),
        }
    }

    pub async fn subscribe_agent(&self, agent_id: &str) -> ActiveStreamBroadcastSubscription {
        let replay = {
            let entries = self.entries.read().await;
            entries
                .iter()
                .filter_map(|(stream_id, entry)| {
                    let owner = entry.owner.as_ref()?;
                    let address = owner.as_chat_address()?;
                    if entry.completed || address.agent_id().map(|id| id.as_str()) != Some(agent_id)
                    {
                        return None;
                    }
                    Some(ActiveStreamBroadcastFrame::Started {
                        device_delivery_id: stream_id.clone(),
                        owner: owner.clone(),
                        authority_epoch: entry.authority_epoch,
                        seq: 0,
                    })
                })
                .collect()
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.agent_watchers
            .write()
            .await
            .entry(agent_id.to_string())
            .or_default()
            .push(tx);
        ActiveStreamBroadcastSubscription {
            replay,
            rx: Some(rx),
        }
    }

    pub async fn subscribe_all(&self) -> ActiveStreamBroadcastSubscription {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.global_watchers.write().await.push(tx);
        ActiveStreamBroadcastSubscription {
            replay: Vec::new(),
            rx: Some(rx),
        }
    }
}

fn replay_entry(
    stream_id: &str,
    entry: &ActiveStreamBroadcastEntry,
    after_seq: u64,
) -> Vec<ActiveStreamBroadcastFrame> {
    let first_available_seq = entry
        .frames
        .front()
        .and_then(ActiveStreamBroadcastFrame::seq)
        .unwrap_or(entry.first_available_seq);
    if after_seq.saturating_add(1) < first_available_seq {
        return vec![ActiveStreamBroadcastFrame::Gap {
            device_delivery_id: stream_id.to_string(),
            requested_after_seq: after_seq,
            first_available_seq,
        }];
    }

    entry
        .frames
        .iter()
        .filter(|frame| frame.seq().is_some_and(|seq| seq > after_seq))
        .cloned()
        .collect()
}

fn subscribe_entry(
    stream_id: &str,
    entry: &mut ActiveStreamBroadcastEntry,
    after_seq: u64,
) -> ActiveStreamBroadcastSubscription {
    let replay = replay_entry(stream_id, entry, after_seq);

    if entry.completed {
        return ActiveStreamBroadcastSubscription { replay, rx: None };
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    entry.subscribers.push(tx);
    ActiveStreamBroadcastSubscription {
        replay,
        rx: Some(rx),
    }
}

impl ActiveStreamBroadcastHub {
    pub async fn publish_event(
        &self,
        device_delivery_id: &str,
        address: &ChatAddress,
        payload: String,
    ) {
        let _ = self
            .publish_event_inner(
                device_delivery_id,
                &ActiveStreamOwner::chat(address.clone()),
                None,
                payload,
                ActiveReplayPolicy::default(),
            )
            .await;
    }

    pub async fn publish_conversation_event(
        &self,
        device_delivery_id: &str,
        address: &ChatAddress,
        authority_epoch: ConversationAuthorityEpoch,
        payload: String,
    ) {
        let timeline = self.conversation_timeline(address).await;
        let mut timeline = timeline.lock().await;
        let sequence = timeline.next_sequence;
        let payload = with_conversation_activity_metadata(
            payload,
            &self.activity_generation,
            Some(sequence),
            false,
        );
        if let Some(frame) = self
            .publish_event_inner(
                device_delivery_id,
                &ActiveStreamOwner::chat(address.clone()),
                Some(authority_epoch),
                payload,
                ActiveReplayPolicy::default(),
            )
            .await
        {
            if conversation_event_type(&frame).as_deref() == Some("user_message") {
                timeline.checkpoint_frames.clear();
                timeline.checkpoint_bytes = 0;
                timeline.checkpoint_available = true;
            }
            if timeline.checkpoint_available {
                let frame_bytes = match &frame {
                    ActiveStreamBroadcastFrame::Event { payload, .. } => payload.len(),
                    _ => 0,
                };
                if timeline.checkpoint_bytes.saturating_add(frame_bytes)
                    <= CONVERSATION_CHECKPOINT_MAX_BYTES
                {
                    timeline.checkpoint_bytes =
                        timeline.checkpoint_bytes.saturating_add(frame_bytes);
                    timeline.checkpoint_frames.push_back(frame.clone());
                } else {
                    tracing::warn!(
                        chat = ?address,
                        checkpoint_bytes = timeline.checkpoint_bytes,
                        limit_bytes = CONVERSATION_CHECKPOINT_MAX_BYTES,
                        "conversation activity checkpoint exceeded its memory limit"
                    );
                    timeline.checkpoint_frames.clear();
                    timeline.checkpoint_bytes = 0;
                    timeline.checkpoint_available = false;
                }
            }
            timeline.next_sequence = timeline.next_sequence.saturating_add(1);
            push_conversation_timeline_frame(&mut timeline, frame);
        }
    }

    pub async fn publish_event_with_replay_max_bytes(
        &self,
        device_delivery_id: &str,
        address: &ChatAddress,
        payload: String,
        replay_max_bytes: usize,
    ) {
        let _ = self
            .publish_event_inner(
                device_delivery_id,
                &ActiveStreamOwner::chat(address.clone()),
                None,
                payload,
                ActiveReplayPolicy {
                    max_bytes: Some(replay_max_bytes),
                    ..ActiveReplayPolicy::default()
                },
            )
            .await;
    }

    pub async fn publish_terminal_event_with_replay_max_bytes(
        &self,
        device_delivery_id: &str,
        session_id: &str,
        payload: String,
        replay_max_bytes: usize,
    ) {
        let _ = self
            .publish_event_inner(
                device_delivery_id,
                &ActiveStreamOwner::terminal(session_id),
                None,
                payload,
                ActiveReplayPolicy {
                    max_bytes: Some(replay_max_bytes),
                    ..ActiveReplayPolicy::default()
                },
            )
            .await;
    }

    pub async fn publish_event_with_replay_max_lines(
        &self,
        device_delivery_id: &str,
        address: &ChatAddress,
        payload: String,
        replay_lines: usize,
        replay_max_lines: usize,
    ) {
        let _ = self
            .publish_event_inner(
                device_delivery_id,
                &ActiveStreamOwner::chat(address.clone()),
                None,
                payload,
                ActiveReplayPolicy {
                    frame_lines: replay_lines,
                    max_lines: Some(replay_max_lines),
                    ..ActiveReplayPolicy::default()
                },
            )
            .await;
    }

    pub async fn trim_replay_to_lines(&self, device_delivery_id: &str, max_lines: usize) {
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(device_delivery_id) else {
            return;
        };
        trim_active_replay_lines(entry, max_lines);
    }

    async fn publish_event_inner(
        &self,
        device_delivery_id: &str,
        owner: &ActiveStreamOwner,
        authority_epoch: Option<ConversationAuthorityEpoch>,
        payload: String,
        replay_policy: ActiveReplayPolicy,
    ) -> Option<ActiveStreamBroadcastFrame> {
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(device_delivery_id.to_string())
            .or_insert_with(ActiveStreamBroadcastEntry::new);
        if entry.completed {
            return None;
        }
        let starts_stream = entry.frames.is_empty();
        entry.owner = Some(owner.clone());
        entry.authority_epoch = authority_epoch;
        let started_frame = starts_stream.then(|| ActiveStreamBroadcastFrame::Started {
            device_delivery_id: device_delivery_id.to_string(),
            owner: owner.clone(),
            authority_epoch,
            seq: 0,
        });
        let frame = ActiveStreamBroadcastFrame::Event {
            device_delivery_id: device_delivery_id.to_string(),
            owner: owner.clone(),
            authority_epoch,
            seq: entry.next_seq,
            payload,
        };
        entry.next_seq = entry.next_seq.saturating_add(1);
        push_frame(entry, frame.clone(), replay_policy);
        drop(entries);
        if let Some(started_frame) = started_frame {
            self.push_owner_frame(owner, started_frame).await;
        }
        self.push_owner_frame(owner, frame.clone()).await;
        Some(frame)
    }

    pub async fn finish(&self, device_delivery_id: &str, address: &ChatAddress) {
        self.finish_inner(
            device_delivery_id,
            &ActiveStreamOwner::chat(address.clone()),
            None,
        )
        .await;
    }

    pub async fn finish_terminal(&self, device_delivery_id: &str, session_id: &str) {
        self.finish_inner(
            device_delivery_id,
            &ActiveStreamOwner::terminal(session_id),
            None,
        )
        .await;
    }

    pub async fn finish_conversation(
        &self,
        device_delivery_id: &str,
        address: &ChatAddress,
        authority_epoch: ConversationAuthorityEpoch,
    ) {
        self.finish_inner(
            device_delivery_id,
            &ActiveStreamOwner::chat(address.clone()),
            Some(authority_epoch),
        )
        .await;
    }

    pub async fn publish_authority_redirect(
        &self,
        address: &ChatAddress,
        placement: ConversationAuthorityPlacement,
    ) {
        self.push_owner_frame(
            &ActiveStreamOwner::chat(address.clone()),
            ActiveStreamBroadcastFrame::AuthorityRedirect {
                device_delivery_id: format!("authority-redirect:{address}"),
                address: address.clone(),
                placement,
                seq: 0,
            },
        )
        .await;
    }

    async fn finish_inner(
        &self,
        device_delivery_id: &str,
        owner: &ActiveStreamOwner,
        authority_epoch: Option<ConversationAuthorityEpoch>,
    ) {
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(device_delivery_id.to_string())
            .or_insert_with(ActiveStreamBroadcastEntry::new);
        if entry.completed {
            return;
        }
        entry.owner = Some(owner.clone());
        entry.authority_epoch = authority_epoch;
        let frame = ActiveStreamBroadcastFrame::Done {
            device_delivery_id: device_delivery_id.to_string(),
            owner: owner.clone(),
            authority_epoch,
            seq: entry.next_seq,
        };
        entry.next_seq = entry.next_seq.saturating_add(1);
        entry.completed = true;
        push_frame(entry, frame.clone(), ActiveReplayPolicy::default());
        trim_completed_replay(entry);
        entry.subscribers.clear();
        drop(entries);
        self.push_owner_frame(owner, frame).await;
    }

    pub async fn fail(
        &self,
        device_delivery_id: &str,
        address: Option<&ChatAddress>,
        detail: impl Into<String>,
    ) {
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(device_delivery_id.to_string())
            .or_insert_with(ActiveStreamBroadcastEntry::new);
        if entry.completed {
            return;
        }
        if let Some(address) = address {
            entry.owner = Some(ActiveStreamOwner::chat(address.clone()));
        }
        let frame = ActiveStreamBroadcastFrame::Error {
            device_delivery_id: device_delivery_id.to_string(),
            owner: entry.owner.clone(),
            authority_epoch: entry.authority_epoch,
            seq: entry.next_seq,
            detail: detail.into(),
        };
        let owner_for_watchers = entry.owner.clone();
        entry.next_seq = entry.next_seq.saturating_add(1);
        entry.completed = true;
        push_frame(entry, frame.clone(), ActiveReplayPolicy::default());
        trim_completed_replay(entry);
        entry.subscribers.clear();
        drop(entries);
        if let Some(owner) = owner_for_watchers {
            self.push_owner_frame(&owner, frame).await;
        }
    }

    async fn push_owner_frame(&self, owner: &ActiveStreamOwner, frame: ActiveStreamBroadcastFrame) {
        if let Some(hub) = self
            .navigation_activity
            .read()
            .expect("navigation activity hub lock poisoned")
            .as_ref()
        {
            if let Some(wake) =
                super::chat_activity_wake::ChatLiveWake::from_frame(frame.clone(), String::new())
            {
                if wake.activity.is_some() {
                    hub.navigation.observe(&wake);
                }
            }
        }
        if let Some(watchers) = self.owner_watchers.write().await.get_mut(owner) {
            watchers.retain(|tx| tx.send(frame.clone()).is_ok());
        }
        if let Some(address) = owner.as_chat_address() {
            if let Some(agent_id) = address.agent_id() {
                if let Some(watchers) = self.agent_watchers.write().await.get_mut(agent_id.as_str())
                {
                    watchers.retain(|tx| tx.send(frame.clone()).is_ok());
                }
            }
        }
        self.global_watchers
            .write()
            .await
            .retain(|tx| tx.send(frame.clone()).is_ok());
    }
}

fn push_frame(
    entry: &mut ActiveStreamBroadcastEntry,
    frame: ActiveStreamBroadcastFrame,
    replay_policy: ActiveReplayPolicy,
) {
    if entry.completed && entry.frames.len() >= MAX_COMPLETED_REPLAY_FRAMES {
        pop_front_frame(entry);
    }
    entry.replay_bytes = entry
        .replay_bytes
        .saturating_add(frame_replay_bytes(&frame));
    entry.replay_lines = entry.replay_lines.saturating_add(replay_policy.frame_lines);
    entry
        .frame_replay_lines
        .push_back(replay_policy.frame_lines);
    entry.frames.push_back(frame.clone());
    if !entry.completed {
        if let Some(max_bytes) = replay_policy.max_bytes {
            trim_active_replay(entry, max_bytes);
        }
        if let Some(max_lines) = replay_policy.max_lines {
            trim_active_replay_lines(entry, max_lines);
        }
    }
    entry
        .subscribers
        .retain(|tx| tx.send(frame.clone()).is_ok());
}

fn trim_active_replay_lines(entry: &mut ActiveStreamBroadcastEntry, max_lines: usize) {
    while entry.replay_lines > max_lines && entry.frames.len() > 1 {
        pop_front_frame(entry);
    }
}

fn trim_active_replay(entry: &mut ActiveStreamBroadcastEntry, max_bytes: usize) {
    while entry.replay_bytes > max_bytes {
        if entry.frames.is_empty() {
            entry.replay_bytes = 0;
            break;
        }
        pop_front_frame(entry);
    }
}

fn trim_completed_replay(entry: &mut ActiveStreamBroadcastEntry) {
    while entry.frames.len() > MAX_COMPLETED_REPLAY_FRAMES {
        pop_front_frame(entry);
    }
}

fn pop_front_frame(entry: &mut ActiveStreamBroadcastEntry) {
    let Some(frame) = entry.frames.pop_front() else {
        entry.replay_bytes = 0;
        entry.replay_lines = 0;
        entry.frame_replay_lines.clear();
        return;
    };
    let frame_lines = entry.frame_replay_lines.pop_front().unwrap_or_default();
    entry.replay_bytes = entry
        .replay_bytes
        .saturating_sub(frame_replay_bytes(&frame));
    entry.replay_lines = entry.replay_lines.saturating_sub(frame_lines);
    if let Some(seq) = frame.seq() {
        entry.first_available_seq = entry.first_available_seq.max(seq.saturating_add(1));
    }
}

fn frame_replay_bytes(frame: &ActiveStreamBroadcastFrame) -> usize {
    match frame {
        ActiveStreamBroadcastFrame::Event { payload, .. } => payload.len(),
        _ => 0,
    }
}

pub struct ActiveStreamBroadcastDeviceSessionHandler {
    hub: Arc<ActiveStreamBroadcastHub>,
    conversation_control_initial_state: Option<ConversationControlInitialState>,
    foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
}

struct ConversationControlInitialState {
    authority: Arc<crate::services::conversation_authority::ConversationAuthority>,
    intake: crate::services::conversation_input_intake::ConversationInputIntake,
    local_device_id: String,
}

impl ActiveStreamBroadcastDeviceSessionHandler {
    pub fn new(hub: Arc<ActiveStreamBroadcastHub>) -> Self {
        Self {
            hub,
            conversation_control_initial_state: None,
            foreground_operations: None,
        }
    }

    pub fn with_conversation_control_initial_state(
        mut self,
        authority: Arc<crate::services::conversation_authority::ConversationAuthority>,
        intake: crate::services::conversation_input_intake::ConversationInputIntake,
        local_device_id: String,
    ) -> Self {
        self.conversation_control_initial_state = Some(ConversationControlInitialState {
            authority,
            intake,
            local_device_id,
        });
        self
    }

    pub fn with_foreground_operations(
        mut self,
        foreground_operations: Arc<ForegroundOperationRegistry>,
    ) -> Self {
        self.foreground_operations = Some(foreground_operations);
        self
    }

    fn viewer_lease_guard(
        &self,
        request: &ActiveStreamBroadcastSubscribeRequest,
    ) -> Option<ForegroundOperationLeaseGuard> {
        let registry = self.foreground_operations.as_ref()?;
        let viewer = request.viewer.as_ref()?;
        let owner = request.owner.as_ref()?;
        let owner_id_is_empty = match owner {
            ActiveStreamOwner::Chat { address } => address.chat_id().as_str().is_empty(),
            ActiveStreamOwner::Terminal { session_id } => session_id.is_empty(),
        };
        if viewer.source_device_id.is_empty() || viewer.viewer_id.is_empty() || owner_id_is_empty {
            return None;
        }
        let operation_id = format!(
            "viewer:{:?}:{owner}:{}:{}",
            viewer.kind, viewer.source_device_id, viewer.viewer_id
        );
        let lease_id = format!("{operation_id}:{}", uuid::Uuid::new_v4());
        registry.acquire(lease_id.clone(), operation_id, viewer.kind);
        Some(ForegroundOperationLeaseGuard {
            registry: Arc::clone(registry),
            lease_id,
        })
    }

    async fn initial_conversation_control_frame(
        &self,
        request: &ActiveStreamBroadcastSubscribeRequest,
    ) -> Result<Option<ActiveStreamBroadcastFrame>, String> {
        if !request.initial_conversation_control {
            return Ok(None);
        }
        let initial_state = self
            .conversation_control_initial_state
            .as_ref()
            .ok_or_else(|| "initial Conversation Control View is unavailable".to_string())?;
        let owner = request
            .owner
            .as_ref()
            .ok_or_else(|| "initial Conversation Control View requires stream owner".to_string())?;
        let address = owner
            .as_chat_address()
            .ok_or_else(|| "initial Conversation Control View requires Chat owner".to_string())?;
        let chat_id = address.chat_id().as_str();
        let target = initial_state
            .intake
            .resolve_initial_conversation_control_target(
                address,
                initial_state.local_device_id.as_str(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let workspace_dir = target.scope_root;
        let project_agent_state = target.project_agent_state;
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &workspace_dir,
                chat_id,
            )
            .await?;
        if placement.device_id != initial_state.local_device_id {
            if address.agent_id().is_some()
                && request
                    .authority_epoch
                    .is_some_and(|expected_epoch| placement.authority_epoch > expected_epoch)
            {
                return Ok(Some(ActiveStreamBroadcastFrame::AuthorityRedirect {
                    device_delivery_id: format!("authority-redirect:{address}"),
                    address: address.clone(),
                    placement: ConversationAuthorityPlacement {
                        device_id: placement.device_id,
                        authority_epoch: placement.authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash,
                    },
                    seq: 0,
                }));
            }
            return Err(
                "Conversation Live Activity targets the wrong authority Device".to_string(),
            );
        }
        if let Some(expected_epoch) = request.authority_epoch {
            if placement.authority_epoch != expected_epoch {
                if address.agent_id().is_some() && placement.authority_epoch > expected_epoch {
                    return Ok(Some(ActiveStreamBroadcastFrame::AuthorityRedirect {
                        device_delivery_id: format!("authority-redirect:{address}"),
                        address: address.clone(),
                        placement: ConversationAuthorityPlacement {
                            device_id: placement.device_id,
                            authority_epoch: placement.authority_epoch,
                            base_manifest_hash: placement.base_manifest_hash,
                        },
                        seq: 0,
                    }));
                }
                return Err(
                    "Conversation Live Activity targets a stale authority Epoch".to_string()
                );
            }
        } else if address.agent_id().is_some() {
            return Err(
                "initial Agent Conversation Control View requires Authority Epoch".to_string(),
            );
        }
        let authority_epoch = placement.authority_epoch;
        let mut meta = crate::services::conversation_authority_target::load_chat_control_meta(
            &workspace_dir,
            chat_id,
        )
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Chat Session File '{chat_id}' is unavailable"))?;
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Ok(model) = initial_state
            .intake
            .authority_chat_model_for(&workspace_dir, chat_id)
            .await
        {
            meta.as_object_mut()
                .expect("control metadata is an object")
                .insert(
                    crate::services::conversation_authority_placement::CHAT_MODEL_META_KEY
                        .to_string(),
                    model,
                );
        }
        let mut control = initial_state.authority.control_view(address, &meta).await;
        control.project_chat_agent_state = project_agent_state;
        let payload = match address.agent_id() {
            Some(agent_id) => serde_json::json!({
                "type": "conversation_control",
                "agent_id": agent_id,
                "chat_id": chat_id,
                "reason": "viewer_attached",
                "control": control,
            }),
            None => serde_json::json!({
                "type": "conversation_control",
                "address": address,
                "reason": "viewer_attached",
                "control": control,
            }),
        };
        let payload = self.hub.conversation_activity_snapshot(payload.to_string());
        Ok(Some(ActiveStreamBroadcastFrame::Event {
            device_delivery_id: format!("control:{address}"),
            owner: ActiveStreamOwner::chat(address.clone()),
            authority_epoch: Some(authority_epoch),
            seq: 0,
            payload,
        }))
    }
}

impl std::fmt::Debug for ActiveStreamBroadcastDeviceSessionHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveStreamBroadcastDeviceSessionHandler")
            .finish()
    }
}

#[async_trait::async_trait]
impl DeviceSessionLiveActivityHandler for ActiveStreamBroadcastDeviceSessionHandler {
    async fn subscribe_live_activity(
        &self,
        payload: &[u8],
    ) -> Result<DeviceSessionFrameStream, String> {
        let request: ActiveStreamBroadcastSubscribeRequest =
            serde_json::from_slice(payload).map_err(|error| error.to_string())?;
        let initial_control = self.initial_conversation_control_frame(&request).await?;
        if initial_control
            .as_ref()
            .is_some_and(ActiveStreamBroadcastFrame::is_authority_redirect)
        {
            return Ok(Box::pin(futures::stream::iter(
                initial_control.into_iter().map(frame_bytes),
            )));
        }
        let viewer_lease = self.viewer_lease_guard(&request);
        let mut subscription = active_stream_subscription(&self.hub, &request).await;
        if request.collapse_replay {
            subscription.replay = collapse_replay_frames(subscription.replay);
        }
        if let Some(initial_control) = initial_control {
            subscription.replay.insert(0, initial_control);
        }
        let replay = VecDeque::from(subscription.replay);
        Ok(Box::pin(futures::stream::unfold(
            (replay, subscription.rx, viewer_lease),
            |(mut replay, mut rx, viewer_lease)| async move {
                if let Some(frame) = replay.pop_front() {
                    if let Some(viewer_lease) = viewer_lease.as_ref() {
                        viewer_lease.touch();
                    }
                    return Some((frame_bytes(frame), (replay, rx, viewer_lease)));
                }
                loop {
                    tokio::select! {
                        frame = rx.as_mut()?.recv() => {
                            let frame = frame?;
                            if let Some(viewer_lease) = viewer_lease.as_ref() {
                                viewer_lease.touch();
                            }
                            return Some((frame_bytes(frame), (replay, rx, viewer_lease)));
                        }
                        _ = tokio::time::sleep(VIEWER_LEASE_TOUCH_INTERVAL) => {
                            if let Some(viewer_lease) = viewer_lease.as_ref() {
                                viewer_lease.touch();
                            }
                        }
                    }
                }
            },
        )))
    }
}

struct ForegroundOperationLeaseGuard {
    registry: Arc<ForegroundOperationRegistry>,
    lease_id: String,
}

impl ForegroundOperationLeaseGuard {
    fn touch(&self) {
        self.registry.touch(&self.lease_id);
    }
}

impl Drop for ForegroundOperationLeaseGuard {
    fn drop(&mut self) {
        self.registry.release(&self.lease_id);
    }
}

async fn active_stream_subscription(
    hub: &ActiveStreamBroadcastHub,
    request: &ActiveStreamBroadcastSubscribeRequest,
) -> ActiveStreamBroadcastSubscription {
    if !request.device_delivery_id.is_empty() {
        hub.subscribe(
            &request.device_delivery_id,
            request
                .after_seq_by_stream
                .get(&request.device_delivery_id)
                .copied()
                .unwrap_or(0),
        )
        .await
    } else if let Some(owner) = request.owner.as_ref() {
        match owner {
            ActiveStreamOwner::Chat { address } => {
                if request.after_seq_by_stream.is_empty() {
                    if let Some(position) = request.conversation_activity_position.as_ref() {
                        return hub
                            .subscribe_chat_from_activity_position(address, position)
                            .await;
                    }
                }
                let empty_cursors = BTreeMap::new();
                let cursors =
                    if request.activity_generation.as_deref() == Some(hub.activity_generation()) {
                        &request.after_seq_by_stream
                    } else {
                        &empty_cursors
                    };
                hub.subscribe_chat(address, cursors).await
            }
            ActiveStreamOwner::Terminal { session_id } => {
                hub.subscribe_terminal(session_id, &request.after_seq_by_stream)
                    .await
            }
        }
    } else {
        ActiveStreamBroadcastSubscription {
            replay: vec![ActiveStreamBroadcastFrame::Error {
                device_delivery_id: String::new(),
                owner: None,
                authority_epoch: None,
                seq: 0,
                detail: "active stream broadcast subscribe request requires device_delivery_id or stream owner".to_string(),
            }],
            rx: None,
        }
    }
}

fn frame_bytes(frame: ActiveStreamBroadcastFrame) -> Vec<u8> {
    serde_json::to_vec(&frame).expect("Active Stream Broadcast frame must serialize")
}

#[derive(Debug)]
pub enum ActiveStreamBroadcastClientError {
    Cancelled,
    Connect(String),
    Json(serde_json::Error),
    Session(DeviceSessionError),
}

impl std::fmt::Display for ActiveStreamBroadcastClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "active stream broadcast cancelled"),
            Self::Connect(error) => write!(f, "connect: {error}"),
            Self::Json(error) => write!(f, "json: {error}"),
            Self::Session(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ActiveStreamBroadcastClientError {}

impl From<serde_json::Error> for ActiveStreamBroadcastClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<DeviceSessionError> for ActiveStreamBroadcastClientError {
    fn from(error: DeviceSessionError) -> Self {
        Self::Session(error)
    }
}

pub async fn stream_active_broadcast_via_session<F, Fut>(
    session_manager: &DeviceSessionManager,
    peer_device_id: &str,
    mut request: ActiveStreamBroadcastSubscribeRequest,
    connect_timeout: Duration,
    mut cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    mut on_frame: F,
) -> Result<(), ActiveStreamBroadcastClientError>
where
    F: FnMut(ActiveStreamBroadcastFrame) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if cancel_rx.borrow().is_terminal() {
        return Err(ActiveStreamBroadcastClientError::Cancelled);
    }
    // The viewer owns demand across short attach/snapshot deadlines. Cancelling
    // it releases interest without cancelling another consumer's establishment.
    let _interest = session_manager.retain_peer(peer_device_id).await?;
    let persistent_activity_watch =
        request.device_delivery_id.is_empty() && request.owner.is_some();
    let mut reconnects = 0usize;
    let mut connectivity = session_manager.subscribe_connectivity_changes();

    loop {
        let payload = serde_json::to_vec(&request)?;
        let opened = tokio::select! {
            _ = cancel_rx.wait_for(|control| control.is_terminal()) => {
                return Err(ActiveStreamBroadcastClientError::Cancelled);
            }
            opened = session_manager.open_live_activity_to(peer_device_id, &payload, connect_timeout) => opened,
        };
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(error) => {
                // A short logical attach deadline is not the lifetime of the
                // visible viewer's shared dial. Await the manager's outcome.
                if persistent_activity_watch
                    && matches!(
                        error,
                        DeviceSessionError::Timeout {
                            stage: "Device Session admission",
                            ..
                        }
                    )
                    && session_manager.connectivity_state(peer_device_id)?
                        == rua_device_network::DeviceConnectivityState::Connecting
                {
                    tokio::select! {
                        _ = cancel_rx.wait_for(|control| control.is_terminal()) => return Err(ActiveStreamBroadcastClientError::Cancelled),
                        _ = connectivity.changed() => {},
                    }
                    continue;
                }
                if reconnects >= ACTIVE_STREAM_BROADCAST_RECONNECT_ATTEMPTS
                    || error.is_version_incompatible()
                {
                    return Err(error.into());
                }
                reconnects += 1;
                tokio::time::sleep(ACTIVE_STREAM_BROADCAST_RECONNECT_BACKOFF).await;
                continue;
            }
        };

        loop {
            let frame = tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && cancel_rx.borrow().is_terminal() {
                        return Err(ActiveStreamBroadcastClientError::Cancelled);
                    }
                    continue;
                }
                frame = stream.next_frame() => frame,
            };
            match frame {
                Ok(Some(bytes)) => {
                    let frame: ActiveStreamBroadcastFrame = serde_json::from_slice(&bytes)?;
                    if let ActiveStreamBroadcastFrame::Event { payload, .. } = &frame {
                        if let Some(generation) = conversation_activity_generation(payload) {
                            if request.activity_generation.as_deref() != Some(generation.as_str()) {
                                request.after_seq_by_stream.clear();
                                request.activity_generation = Some(generation);
                            }
                            request.conversation_activity_position = None;
                        }
                    }
                    if let Some(seq) = frame.seq() {
                        request
                            .after_seq_by_stream
                            .entry(frame.stream_id().to_string())
                            .and_modify(|after_seq| *after_seq = (*after_seq).max(seq))
                            .or_insert(seq);
                    }
                    let terminal = frame.is_terminal();
                    let authority_redirect = frame.is_authority_redirect();
                    on_frame(frame).await;
                    reconnects = 0;
                    if authority_redirect || (terminal && !persistent_activity_watch) {
                        return Ok(());
                    }
                }
                Ok(None) => {
                    if reconnects >= ACTIVE_STREAM_BROADCAST_RECONNECT_ATTEMPTS {
                        return Err(ActiveStreamBroadcastClientError::Connect(
                            "Live Activity logical stream ended".to_string(),
                        ));
                    }
                    reconnects += 1;
                    tokio::time::sleep(ACTIVE_STREAM_BROADCAST_RECONNECT_BACKOFF).await;
                    break;
                }
                Err(error) => {
                    if reconnects >= ACTIVE_STREAM_BROADCAST_RECONNECT_ATTEMPTS {
                        return Err(error.into());
                    }
                    reconnects += 1;
                    tokio::time::sleep(ACTIVE_STREAM_BROADCAST_RECONNECT_BACKOFF).await;
                    break;
                }
            }
        }
    }
}
