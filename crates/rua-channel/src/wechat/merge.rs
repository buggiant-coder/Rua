//! QwenPaw text reply merging; media bypasses the buffer.
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::channel::{OutMessage, ToHandle};
use crate::core::content::OutgoingContentPart;
use crate::core::pipeline::SendFn;
use rua_core::Result;

pub(super) struct ReplyMerger {
    pending: Arc<Mutex<Vec<(ToHandle, OutMessage)>>>,
    send: SendFn,
    delay: Duration,
    timer: Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
    cancel: CancellationToken,
}

impl ReplyMerger {
    pub fn new(send: SendFn, delay_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::default(),
            send,
            delay: Duration::from_millis(delay_ms),
            timer: Mutex::new(None),
            cancel: CancellationToken::new(),
        })
    }

    pub fn sender(self: &Arc<Self>) -> SendFn {
        let merger = self.clone();
        Arc::new(move |to, msg| {
            let merger = merger.clone();
            Box::pin(async move { merger.send(to, msg).await })
        })
    }

    async fn send(&self, to: ToHandle, mut msg: OutMessage) -> Result<()> {
        let (text, media): (Vec<_>, Vec<_>) = msg.content.into_iter().partition(|part| {
            matches!(
                part,
                OutgoingContentPart::Text { .. }
                    | OutgoingContentPart::Markdown { .. }
                    | OutgoingContentPart::Refusal { .. }
            )
        });
        msg.content = text;
        if !msg.content.is_empty() {
            let mut pending = self.pending.lock().await;
            if let Some((target, buffered)) = pending.iter_mut().find(|(target, _)| {
                target.conversation_id == to.conversation_id
                    && target.sender_id == to.sender_id
                    && target.metadata["wechat_context_token"]
                        == to.metadata["wechat_context_token"]
            }) {
                *target = to.clone();
                buffered.content.extend(msg.content.clone());
            } else {
                pending.push((to.clone(), msg.clone()));
            }
            drop(pending);
            if !self.delay.is_zero() {
                let mut timer = self.timer.lock().await;
                if let Some((cancel, task)) = timer.take() {
                    cancel.cancel();
                    let _ = task.await;
                }
                let pending = self.pending.clone();
                let send = self.send.clone();
                let delay = self.delay;
                let cancel = self.cancel.child_token();
                let timer_cancel = cancel.clone();
                let lifetime = self.cancel.clone();
                *timer = Some((
                    timer_cancel,
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = cancel.cancelled() => {},
                            _ = tokio::time::sleep(delay) => {
                                tokio::select! {
                                    _ = lifetime.cancelled() => {},
                                    result = flush(&pending, &send) => {
                                        if let Err(error) = result {
                                            tracing::warn!(%error, "WeChat merged reply failed");
                                        }
                                    }
                                }
                            }
                        }
                    }),
                ));
            }
        }
        if !media.is_empty() {
            msg.content = media;
            (self.send)(to, msg).await?;
        }
        Ok(())
    }

    pub async fn finish(&self) -> Result<()> {
        let mut timer = self.timer.lock().await;
        if let Some((cancel, task)) = timer.take() {
            cancel.cancel();
            let _ = task.await;
        }
        flush(&self.pending, &self.send).await
    }
}

impl Drop for ReplyMerger {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn flush(pending: &Mutex<Vec<(ToHandle, OutMessage)>>, send: &SendFn) -> Result<()> {
    let messages = std::mem::take(&mut *pending.lock().await);
    for (to, msg) in messages {
        send(to, msg).await?;
    }
    Ok(())
}
