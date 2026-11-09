use std::sync::atomic::{AtomicUsize, Ordering};

pub struct KeySelector {
    in_flight: Vec<AtomicUsize>,
    last_selected: AtomicUsize,
}

impl KeySelector {
    pub fn new(key_count: usize) -> Self {
        assert!(key_count > 0);
        let in_flight = (0..key_count).map(|_| AtomicUsize::new(0)).collect();
        Self {
            in_flight,
            last_selected: AtomicUsize::new(0),
        }
    }

    pub fn acquire(&self) -> usize {
        let count = self.in_flight.len();
        if count == 1 {
            self.in_flight[0].fetch_add(1, Ordering::Relaxed);
            return 0;
        }

        let start = (self.last_selected.load(Ordering::Relaxed) + 1) % count;
        let mut best_idx = start;
        let mut best_val = self.in_flight[start].load(Ordering::Relaxed);

        for i in 1..count {
            let idx = (start + i) % count;
            let val = self.in_flight[idx].load(Ordering::Relaxed);
            if val < best_val {
                best_val = val;
                best_idx = idx;
            }
        }

        self.in_flight[best_idx].fetch_add(1, Ordering::Relaxed);
        self.last_selected.store(best_idx, Ordering::Relaxed);
        best_idx
    }

    pub fn release(&self, idx: usize) {
        self.in_flight[idx].fetch_sub(1, Ordering::Relaxed);
    }

    pub fn in_flight_for(&self, idx: usize) -> usize {
        self.in_flight[idx].load(Ordering::Relaxed)
    }

    pub fn key_count(&self) -> usize {
        self.in_flight.len()
    }
}
