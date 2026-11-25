use rua_core::ChatAddress;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

struct TaskWatchWakeReceipt {
    address: ChatAddress,
    receipt_id: String,
    sender: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

#[derive(Clone, Default)]
pub(crate) struct TaskWatchWakeReceipts {
    inner: Arc<StdMutex<HashMap<String, TaskWatchWakeReceipt>>>,
}

pub(crate) struct TaskWatchWakeRegistration {
    pub(crate) receipt_id: String,
    receipts: TaskWatchWakeReceipts,
    receiver: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
    /// False only for the caller that created the receipt.
    pub(crate) fresh: bool,
    pub(crate) enqueued: bool,
}

impl TaskWatchWakeReceipts {
    /// Claims the observation, or joins the delivery already in flight for it.
    ///
    /// Duplicate deliveries of one observation observe the same durable outcome,
    /// so the later one waits on the existing receipt instead of admitting a
    /// second copy. Rejecting duplicates here would surface as a spurious
    /// delivery error and lose the shared acknowledgement.
    pub(crate) fn register(
        &self,
        notification_id: String,
        address: ChatAddress,
    ) -> TaskWatchWakeRegistration {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(receipt) = inner.get(&notification_id) {
            return TaskWatchWakeRegistration {
                receipt_id: receipt.receipt_id.clone(),
                receipts: self.clone(),
                receiver: receipt.sender.subscribe(),
                fresh: false,
                enqueued: true,
            };
        }
        let receipt_id = format!("runtime-observation-{}", uuid::Uuid::new_v4());
        let (sender, receiver) = tokio::sync::watch::channel(None);
        inner.insert(
            notification_id,
            TaskWatchWakeReceipt {
                address,
                receipt_id: receipt_id.clone(),
                sender,
            },
        );
        TaskWatchWakeRegistration {
            receipt_id,
            receipts: self.clone(),
            receiver,
            fresh: true,
            enqueued: false,
        }
    }

    fn resolve(&self, receipt_id: &str, result: Result<(), String>) {
        let receipt = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let key = inner
                .iter()
                .find(|(_, receipt)| receipt.receipt_id == receipt_id)
                .map(|(key, _)| key.clone());
            key.and_then(|key| inner.remove(&key))
        };
        if let Some(receipt) = receipt {
            let _ = receipt.sender.send(Some(result));
        }
    }

    pub(crate) fn complete(&self, receipt_id: &str) {
        self.resolve(receipt_id, Ok(()));
    }
    pub(crate) fn fail(&self, receipt_id: &str, error: &str) {
        self.resolve(receipt_id, Err(error.to_owned()));
    }

    pub(crate) fn fail_all_for_address(&self, address: &ChatAddress, error: &str) {
        let receipts = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let ids = inner
                .iter()
                .filter(|(_, receipt)| receipt.address == *address)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| inner.remove(&id))
                .collect::<Vec<_>>()
        };
        for receipt in receipts {
            let _ = receipt.sender.send(Some(Err(error.to_owned())));
        }
    }
}

impl TaskWatchWakeRegistration {
    pub(crate) async fn wait(mut self) -> Result<(), String> {
        loop {
            if let Some(result) = self.receiver.borrow().clone() {
                return result;
            }
            self.receiver
                .changed()
                .await
                .map_err(|_| "Runtime observation acknowledgement was dropped".to_string())?;
        }
    }
}

impl Drop for TaskWatchWakeRegistration {
    fn drop(&mut self) {
        // A registration that never reached the durable queue must not leave the
        // observation claimed, or a later retry could never be admitted. A caller
        // that merely joined an existing receipt does not own it.
        if self.fresh && !self.enqueued {
            self.receipts
                .fail(&self.receipt_id, "Runtime observation was not enqueued");
        }
    }
}
