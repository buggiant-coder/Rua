use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDedupDecision {
    New,
    Duplicate,
}

pub struct EventDeduper {
    ttl: Duration,
    capacity: usize,
    seen: HashMap<String, SystemTime>,
    order: VecDeque<String>,
    now: Box<dyn Fn() -> SystemTime + Send + Sync>,
}

impl EventDeduper {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self::with_now(ttl, capacity, Box::new(SystemTime::now))
    }

    pub fn with_now(
        ttl: Duration,
        capacity: usize,
        now: Box<dyn Fn() -> SystemTime + Send + Sync>,
    ) -> Self {
        Self {
            ttl,
            capacity,
            seen: HashMap::new(),
            order: VecDeque::new(),
            now,
        }
    }

    pub fn check_and_mark(&mut self, key: impl AsRef<str>) -> EventDedupDecision {
        let key = key.as_ref().trim();
        if key.is_empty() {
            return EventDedupDecision::New;
        }
        let now = (self.now)();
        self.prune_expired(now);
        if self.seen.contains_key(key) {
            return EventDedupDecision::Duplicate;
        }
        self.seen.insert(key.to_string(), now);
        self.order.push_back(key.to_string());
        self.enforce_capacity();
        EventDedupDecision::New
    }

    pub fn clear(&mut self) {
        self.seen.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn prune_expired(&mut self, now: SystemTime) {
        while let Some(key) = self.order.front() {
            let expired = self
                .seen
                .get(key)
                .is_none_or(|seen_at| now.duration_since(*seen_at).unwrap_or_default() > self.ttl);
            if !expired {
                break;
            }
            if let Some(key) = self.order.pop_front() {
                self.seen.remove(&key);
            }
        }
    }

    fn enforce_capacity(&mut self) {
        if self.capacity == 0 {
            self.clear();
            return;
        }
        while self.seen.len() > self.capacity {
            if let Some(key) = self.order.pop_front() {
                self.seen.remove(&key);
            } else {
                break;
            }
        }
    }
}
