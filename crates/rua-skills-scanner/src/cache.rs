use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::types::ScanResult;

pub const MAX_CACHE_ENTRIES: usize = 64;

struct Entry {
    key: PathBuf,
    mtime: f64,
    result: ScanResult,
}

pub struct ScanCache {
    inner: Mutex<VecDeque<Entry>>,
    capacity: usize,
}

impl Default for ScanCache {
    fn default() -> Self {
        Self::with_capacity(MAX_CACHE_ENTRIES)
    }
}

impl ScanCache {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    pub fn get(&self, dir: &Path) -> Option<ScanResult> {
        let key = dir.to_path_buf();
        let mtime = compute_dir_mtime(dir);
        let mut guard = self.inner.lock().unwrap();
        if let Some(idx) = guard.iter().position(|e| e.key == key) {
            if (guard[idx].mtime - mtime).abs() < f64::EPSILON {
                let entry = guard.remove(idx).unwrap();
                let result = entry.result.clone();
                guard.push_back(entry);
                return Some(result);
            }
        }
        None
    }

    pub fn put(&self, dir: &Path, result: ScanResult) {
        let key = dir.to_path_buf();
        let mtime = compute_dir_mtime(dir);
        let mut guard = self.inner.lock().unwrap();
        if let Some(idx) = guard.iter().position(|e| e.key == key) {
            guard.remove(idx);
        }
        guard.push_back(Entry { key, mtime, result });
        while guard.len() > self.capacity {
            guard.pop_front();
        }
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn contains(&self, dir: &Path) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.iter().any(|e| e.key == dir)
    }
}

fn compute_dir_mtime(dir: &Path) -> f64 {
    let mut latest = match std::fs::metadata(dir) {
        Ok(m) => mtime_secs(&m),
        Err(_) => 0.0,
    };
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    if let Ok(m) = entry.metadata() {
                        let v = mtime_secs(&m);
                        if v > latest {
                            latest = v;
                        }
                    }
                }
            }
        }
    }
    latest
}

fn mtime_secs(meta: &std::fs::Metadata) -> f64 {
    match meta.modified() {
        Ok(time) => match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64(),
            Err(_) => 0.0,
        },
        Err(_) => 0.0,
    }
}
