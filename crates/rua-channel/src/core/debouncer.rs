use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};

use crate::core::content::IncomingContentPart;

pub fn content_allows_immediate(parts: &[IncomingContentPart]) -> bool {
    if parts.is_empty() {
        return false;
    }
    !parts
        .iter()
        .any(|p| matches!(p, IncomingContentPart::Text { text } if !text.trim().is_empty()))
}

struct BufferEntry<P> {
    payloads: Vec<P>,
    waiter: Option<oneshot::Sender<Vec<P>>>,
}

pub struct BufferDebouncer<K, P> {
    window: Duration,
    inner: Arc<Mutex<HashMap<K, BufferEntry<P>>>>,
}

impl<K, P> Clone for BufferDebouncer<K, P> {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            inner: self.inner.clone(),
        }
    }
}

impl<K, P> BufferDebouncer<K, P>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    P: Send + 'static,
{
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn push(&self, key: K, payload: P) -> Option<Vec<P>> {
        let mut state = self.inner.lock().await;

        if let Some(entry) = state.get_mut(&key) {
            entry.payloads.push(payload);
            return None;
        }

        let (tx, rx) = oneshot::channel::<Vec<P>>();
        let window = self.window;
        let inner = self.inner.clone();
        let timer_key = key.clone();

        tokio::spawn(async move {
            tokio::time::sleep(window).await;
            let mut state = inner.lock().await;
            if let Some(mut entry) = state.remove(&timer_key) {
                if let Some(waiter) = entry.waiter.take() {
                    let _ = waiter.send(entry.payloads);
                }
            }
        });

        state.insert(
            key.clone(),
            BufferEntry {
                payloads: vec![payload],
                waiter: Some(tx),
            },
        );
        drop(state);

        rx.await.ok()
    }

    pub fn window(&self) -> Duration {
        self.window
    }
}

/// QwenPaw's no-text batching: attachments wait for a text caption; audio is
/// a complete utterance. One wrapper is owned by each started channel.
pub struct NoTextDebounceProcess {
    inner: Arc<dyn crate::core::channel::Process>,
    pending: Mutex<HashMap<(String, String), Vec<IncomingContentPart>>>,
}

impl NoTextDebounceProcess {
    pub fn new(inner: Arc<dyn crate::core::channel::Process>) -> Self {
        Self {
            inner,
            pending: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl crate::core::channel::Process for NoTextDebounceProcess {
    async fn handle(
        &self,
        mut req: crate::core::channel::AgentRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        let key = (req.agent_id.clone(), req.session_id.clone());
        let has_text_or_audio = req.input.iter().any(|part| match part {
            IncomingContentPart::Text { text } => !text.trim().is_empty(),
            IncomingContentPart::Audio { .. } => true,
            IncomingContentPart::File { mime, .. } => mime
                .as_deref()
                .is_some_and(|mime| mime.starts_with("audio/")),
            _ => false,
        });
        let mut pending = self.pending.lock().await;
        if req.metadata["no_text_debounce"] == true && !has_text_or_audio && !req.input.is_empty() {
            pending.entry(key).or_default().extend(req.input);
            return Box::pin(futures::stream::empty());
        }
        if let Some(mut buffered) = pending.remove(&key) {
            buffered.extend(req.input);
            req.input = buffered;
        }
        drop(pending);
        self.inner.handle(req).await
    }
}
