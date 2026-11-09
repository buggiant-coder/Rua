//! Lightweight, process-local facts for the navigation list. These do not acquire
//! a conversation viewer or read message history.
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::chat_activity_wake::ChatLiveWake;
use super::conversation_authority_placement::ConversationAuthorityEpoch;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationActiveTurn {
    pub turn_id: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NavigationActivityChange {
    Control {
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_turn: Option<NavigationActiveTurn>,
    },
    Terminal {
        turn_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationActivityFact {
    pub generation: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub change: NavigationActivityChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationCompletion {
    pub turn_id: String,
    pub completed_at: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationNavigationActivity {
    pub address: ChatAddress,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<ConversationCompletion>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationNavigationEvent {
    Snapshot {
        activities: Vec<ConversationNavigationActivity>,
    },
    Activity {
        #[serde(flatten)]
        activity: ConversationNavigationActivity,
    },
}

struct Entry {
    epoch: ConversationAuthorityEpoch,
    generation: String,
    sequence: u64,
    retired_generations: HashSet<String>,
    active_turn: Option<NavigationActiveTurn>,
    completion: Option<ConversationCompletion>,
}

impl Entry {
    fn activity(&self, address: &ChatAddress) -> ConversationNavigationActivity {
        ConversationNavigationActivity {
            address: address.clone(),
            running: self.active_turn.is_some(),
            completion: self.completion.clone(),
        }
    }
}

pub struct ConversationNavigationActivityHub {
    entries: Mutex<HashMap<ChatAddress, Entry>>,
    events: broadcast::Sender<ConversationNavigationEvent>,
}

impl Default for ConversationNavigationActivityHub {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            events: broadcast::channel(256).0,
        }
    }
}

impl ConversationNavigationActivityHub {
    pub fn observe(&self, wake: &ChatLiveWake) {
        let Some(fact) = wake.activity.as_ref() else {
            return;
        };
        let mut entries = self
            .entries
            .lock()
            .expect("navigation activity lock poisoned");
        let entry = entries
            .entry(wake.address.clone())
            .or_insert_with(|| Entry {
                epoch: wake.authority_epoch,
                generation: fact.generation.clone(),
                sequence: 0,
                retired_generations: HashSet::new(),
                active_turn: None,
                completion: None,
            });
        if wake.authority_epoch < entry.epoch {
            return;
        }
        let newer_epoch = wake.authority_epoch > entry.epoch;
        if !newer_epoch && entry.retired_generations.contains(&fact.generation) {
            return;
        }
        let before = entry.activity(&wake.address);
        if newer_epoch || fact.generation != entry.generation {
            if newer_epoch {
                // The authority can return to a still-running process on a previous device.
                entry.retired_generations.clear();
            } else {
                entry.retired_generations.insert(entry.generation.clone());
            }
            entry.active_turn = None;
            entry.sequence = 0;
        }
        entry.epoch = wake.authority_epoch;
        entry.generation = fact.generation.clone();
        let latest = fact.sequence > entry.sequence;
        entry.sequence = entry.sequence.max(fact.sequence);
        match &fact.change {
            NavigationActivityChange::Control { active_turn, .. } => {
                if latest {
                    entry.active_turn = active_turn.clone();
                }
            }
            NavigationActivityChange::Terminal {
                turn_id,
                status,
                completed_at,
            } => {
                // A late terminal from an earlier Turn must not clear the next Turn.
                if latest
                    && entry.active_turn.as_ref().is_some_and(|active| {
                        active.turn_id == *turn_id
                            && match completed_at.as_ref() {
                                Some(completed) => at_or_after(completed, &active.started_at),
                                None => true,
                            }
                    })
                {
                    entry.active_turn = None;
                }
                if status == "completed" {
                    if let Some(completed_at) =
                        completed_at.as_ref().filter(|time| !time.is_empty())
                    {
                        let is_latest_completion = match entry.completion.as_ref() {
                            Some(previous) => at_or_after(completed_at, &previous.completed_at),
                            None => true,
                        };
                        if is_latest_completion {
                            entry.completion = Some(ConversationCompletion {
                                turn_id: turn_id.clone(),
                                completed_at: completed_at.clone(),
                                token: format!("{}:{turn_id}:{completed_at}", wake.authority_epoch),
                            });
                        }
                    }
                }
            }
        }
        let activity = entry.activity(&wake.address);
        if activity != before {
            let _ = self
                .events
                .send(ConversationNavigationEvent::Activity { activity });
        }
    }

    /// Subscription and snapshot share the mutation lock: a transition is either
    /// represented in the snapshot or delivered after it, never lost between them.
    pub fn subscribe(
        &self,
    ) -> (
        ConversationNavigationEvent,
        broadcast::Receiver<ConversationNavigationEvent>,
    ) {
        let entries = self
            .entries
            .lock()
            .expect("navigation activity lock poisoned");
        let receiver = self.events.subscribe();
        let activities = entries
            .iter()
            .map(|(address, entry)| entry.activity(address))
            .collect();
        (
            ConversationNavigationEvent::Snapshot { activities },
            receiver,
        )
    }
}

fn at_or_after(candidate: &str, reference: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(candidate),
        chrono::DateTime::parse_from_rfc3339(reference),
    ) {
        (Ok(candidate), Ok(reference)) => candidate >= reference,
        _ => false,
    }
}
