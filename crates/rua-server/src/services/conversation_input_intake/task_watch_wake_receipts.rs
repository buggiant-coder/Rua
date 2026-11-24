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
    pub(crate) enqueued: bool,
}

impl TaskWatchWakeReceipts {
    /// Claims the observation. Only one delivery may be in flight for a given
    /// observation, so an overlapping caller is rejected rather than made to wait
    /// behind work it cannot influence. A receipt is released once its delivery
    /// resolves, which lets a genuinely new delivery retry after a failure.
    pub(crate) fn register(
        &self,
        notification_id: String,
        address: ChatAddress,
    ) -> Result<TaskWatchWakeRegistration, String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.contains_key(&notification_id) {
            return Err(format!(
                "Runtime observation '{notification_id}' is already registered"
            ));
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
        Ok(TaskWatchWakeRegistration {
            receipt_id,
            receipts: self.clone(),
            receiver,
            enqueued: false,
        })
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
        // observation claimed, or a later retry could never be admitted.
        if !self.enqueued {
            self.receipts
                .fail(&self.receipt_id, "Runtime observation was not enqueued");
        }
    }
}
