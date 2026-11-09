//! Remember the platform target behind a session for scheduled replies.
use crate::core::channel::{AgentRequest, ToHandle};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::PathBuf, sync::Mutex};

#[derive(Clone, Serialize, Deserialize)]
struct Route {
    conversation_id: String,
    sender_id: String,
    sender_staff_id: String,
    conversation_type: String,
}

pub(super) struct Routes {
    entries: Mutex<VecDeque<Route>>,
    path: PathBuf,
}

impl Routes {
    pub fn new(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            entries: Mutex::new(entries),
            path,
        }
    }

    pub fn remember(&self, req: &AgentRequest) -> std::io::Result<()> {
        let route = Route {
            conversation_id: req.conversation_id.clone(),
            sender_id: req.user_id.clone(),
            sender_staff_id: req.metadata["sender_staff_id"]
                .as_str()
                .unwrap_or("")
                .into(),
            conversation_type: req.metadata["conversation_type"]
                .as_str()
                .unwrap_or("")
                .into(),
        };
        if route.conversation_id.is_empty() {
            return Ok(());
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| std::io::Error::other("DingTalk routes lock poisoned"))?;
        entries.retain(|r| {
            r.conversation_id != route.conversation_id || r.sender_id != route.sender_id
        });
        entries.push_back(route);
        while entries.len() > 4096 {
            entries.pop_front();
        }
        let parent = self.path.parent().expect("route file parent");
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".routes-{}.tmp", uuid::Uuid::new_v4()));
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(&*entries)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            std::fs::File::open(parent)?.sync_all()
        };
        let result = write();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    pub fn restore(&self, to: &mut ToHandle) {
        let Ok(entries) = self.entries.lock() else {
            return;
        };
        let route = entries.iter().rev().find(|route| {
            let conversation = &route.conversation_id;
            let session_matches = to.conversation_id == *conversation
                || to.conversation_id == format!("dingtalk:group:{conversation}")
                || to.conversation_id
                    == format!("dingtalk:group:{conversation}:{}", route.sender_id);
            session_matches
                && to
                    .sender_id
                    .as_deref()
                    .is_none_or(|sender| sender == route.sender_id)
        });
        if let Some(route) = route {
            to.conversation_id = route.conversation_id.clone();
            if !to.metadata.is_object() {
                to.metadata = serde_json::json!({});
            }
            for (key, value) in [
                ("sender_staff_id", &route.sender_staff_id),
                ("conversation_type", &route.conversation_type),
            ] {
                if to.metadata[key].as_str().is_none_or(str::is_empty) {
                    to.metadata[key] = serde_json::json!(value);
                }
            }
        }
    }
}
