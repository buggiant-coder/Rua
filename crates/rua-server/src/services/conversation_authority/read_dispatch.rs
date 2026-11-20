use std::collections::{hash_map::Entry, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use uuid::Uuid;

use super::ConversationAuthority;

pub(super) type ReadDispatches = Arc<Mutex<HashMap<Uuid, (String, String)>>>;

/// Held by the local tool call for the lifetime of its approved read dispatch.
#[must_use]
pub struct ReadDispatchPermit {
    token: Uuid,
    dispatches: ReadDispatches,
}

impl ReadDispatchPermit {
    pub fn token(&self) -> Uuid {
        self.token
    }
}

impl Drop for ReadDispatchPermit {
    fn drop(&mut self) {
        self.dispatches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

impl ConversationAuthority {
    pub fn begin_read_dispatch(
        &self,
        approval_id: &str,
        target_device_id: &str,
    ) -> ReadDispatchPermit {
        let token = Uuid::new_v4();
        self.read_dispatches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, (approval_id.to_owned(), target_device_id.to_owned()));
        ReadDispatchPermit {
            token,
            dispatches: Arc::clone(&self.read_dispatches),
        }
    }

    pub(crate) fn claim_read_dispatch(
        &self,
        token: Uuid,
        approval_id: &str,
        target_device_id: &str,
    ) -> bool {
        let mut dispatches = self
            .read_dispatches
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Entry::Occupied(dispatch) = dispatches.entry(token) else {
            return false;
        };
        let (id, device) = dispatch.get();
        if id != approval_id || device != target_device_id {
            return false;
        }
        dispatch.remove();
        true
    }
}
