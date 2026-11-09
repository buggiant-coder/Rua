use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::runner::ActiveSessionTracker;

pub struct SessionGuard {
    tracker: Arc<ActiveSessionTracker>,
    counter: Arc<AtomicUsize>,
    session_id: String,
    request_id: String,
    released: bool,
}

impl SessionGuard {
    pub(crate) fn new(
        tracker: Arc<ActiveSessionTracker>,
        counter: Arc<AtomicUsize>,
        session_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            tracker,
            counter,
            session_id: session_id.into(),
            request_id: request_id.into(),
            released: false,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl std::fmt::Debug for SessionGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGuard")
            .field("session_id", &self.session_id)
            .field("request_id", &self.request_id)
            .finish()
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;

        self.counter.fetch_sub(1, Ordering::Relaxed);

        let tracker = Arc::clone(&self.tracker);
        let session_id = std::mem::take(&mut self.session_id);
        let request_id = std::mem::take(&mut self.request_id);

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    tracker.release(&session_id, &request_id).await;
                });
            }
            Err(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    request_id = %request_id,
                    "SessionGuard dropped outside tokio runtime; tracker entry will be reclaimed on next acquire"
                );
            }
        }
    }
}
