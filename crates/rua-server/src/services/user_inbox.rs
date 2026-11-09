use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const USER_INBOX_FILE: &str = "user_inbox.json";
const USER_INBOX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserInboxDocument {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default)]
    messages: Vec<InboxMessage>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

fn default_schema_version() -> u32 {
    USER_INBOX_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSender {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDelivery {
    pub sender: InboxSender,
    pub title: String,
    pub body_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: String,
    pub sender: InboxSender,
    pub title: String,
    pub body_markdown: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxPage {
    pub items: Vec<InboxMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub unread_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum UserInboxError {
    #[error("{0}")]
    Validation(String),
    #[error("inbox message not found")]
    NotFound,
    #[error("user inbox persistence failed: {0}")]
    Persistence(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub struct UserInbox {
    path: PathBuf,
    messages: Arc<Mutex<Vec<InboxMessage>>>,
    schema_version: u32,
    extra: serde_json::Map<String, serde_json::Value>,
}

impl UserInbox {
    pub fn open(rua_root: &Path) -> Result<Self, UserInboxError> {
        let path = rua_root.join(USER_INBOX_FILE);
        let document = match std::fs::read(&path) {
            Ok(contents) => {
                serde_json::from_slice::<UserInboxDocument>(&contents).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid {}: {error}", path.display()),
                    )
                })?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => UserInboxDocument {
                schema_version: USER_INBOX_SCHEMA_VERSION,
                messages: Vec::new(),
                extra: serde_json::Map::new(),
            },
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            messages: Arc::new(Mutex::new(document.messages)),
            schema_version: document.schema_version,
            extra: document.extra,
        })
    }

    pub async fn deliver(&self, delivery: InboxDelivery) -> Result<InboxMessage, UserInboxError> {
        validate_delivery(&delivery)?;
        let message = InboxMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender: delivery.sender,
            title: delivery.title,
            body_markdown: delivery.body_markdown,
            created_at: chrono::Utc::now().to_rfc3339(),
            read_at: None,
        };
        let mut messages = self.messages.lock().await;
        let mut next = messages.clone();
        next.push(message.clone());
        self.persist(&next)?;
        *messages = next;
        Ok(message)
    }

    pub async fn list_all(&self) -> Vec<InboxMessage> {
        let mut messages = self.messages.lock().await.clone();
        messages.reverse();
        messages
    }

    pub async fn list(
        &self,
        cursor: Option<&str>,
        limit: usize,
        read: Option<bool>,
    ) -> Result<InboxPage, UserInboxError> {
        if !(1..=100).contains(&limit) {
            return Err(UserInboxError::Validation(
                "limit must be between 1 and 100".to_string(),
            ));
        }
        let messages = self.messages.lock().await;
        let unread_count = messages
            .iter()
            .filter(|message| message.read_at.is_none())
            .count();
        let filtered = messages
            .iter()
            .rev()
            .filter(|message| read.is_none_or(|read| message.read_at.is_some() == read))
            .collect::<Vec<_>>();
        let start = if let Some(cursor) = cursor {
            let cursor = decode_cursor(cursor)?;
            filtered
                .iter()
                .position(|message| message.id == cursor)
                .map(|index| index + 1)
                .ok_or_else(|| UserInboxError::Validation("invalid cursor".to_string()))?
        } else {
            0
        };
        let mut items = filtered
            .into_iter()
            .skip(start)
            .take(limit + 1)
            .cloned()
            .collect::<Vec<_>>();
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| items.last().map(|message| encode_cursor(&message.id)))
            .flatten();
        Ok(InboxPage {
            items,
            next_cursor,
            unread_count,
        })
    }

    pub async fn get(&self, id: &str) -> Option<InboxMessage> {
        self.messages
            .lock()
            .await
            .iter()
            .find(|message| message.id == id)
            .cloned()
    }

    pub async fn unread_count(&self) -> usize {
        self.messages
            .lock()
            .await
            .iter()
            .filter(|message| message.read_at.is_none())
            .count()
    }

    pub async fn set_read(&self, id: &str, read: bool) -> Result<InboxMessage, UserInboxError> {
        let mut messages = self.messages.lock().await;
        let Some(index) = messages.iter().position(|message| message.id == id) else {
            return Err(UserInboxError::NotFound);
        };
        let mut next = messages.clone();
        next[index].read_at = read.then(|| chrono::Utc::now().to_rfc3339());
        let message = next[index].clone();
        self.persist(&next)?;
        *messages = next;
        Ok(message)
    }

    pub async fn mark_all_read(&self) -> Result<usize, UserInboxError> {
        let mut messages = self.messages.lock().await;
        let unread_count = messages
            .iter()
            .filter(|message| message.read_at.is_none())
            .count();
        if unread_count == 0 {
            return Ok(0);
        }
        let read_at = chrono::Utc::now().to_rfc3339();
        let mut next = messages.clone();
        for message in &mut next {
            if message.read_at.is_none() {
                message.read_at = Some(read_at.clone());
            }
        }
        self.persist(&next)?;
        *messages = next;
        Ok(unread_count)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, UserInboxError> {
        let mut messages = self.messages.lock().await;
        let mut next = messages.clone();
        next.retain(|message| message.id != id);
        if next.len() == messages.len() {
            return Ok(false);
        }
        self.persist(&next)?;
        *messages = next;
        Ok(true)
    }

    fn persist(&self, messages: &[InboxMessage]) -> Result<(), UserInboxError> {
        let value = serde_json::to_value(UserInboxDocument {
            schema_version: self.schema_version,
            messages: messages.to_vec(),
            extra: self.extra.clone(),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        rua_config::atomic_write_json(&self.path, &value)?;
        Ok(())
    }
}

fn validate_delivery(delivery: &InboxDelivery) -> Result<(), UserInboxError> {
    for (field, value) in [
        ("sender.kind", delivery.sender.kind.as_str()),
        ("sender.display_name", delivery.sender.display_name.as_str()),
        ("title", delivery.title.as_str()),
        ("body_markdown", delivery.body_markdown.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(UserInboxError::Validation(format!(
                "{field} must not be empty"
            )));
        }
    }
    Ok(())
}

fn encode_cursor(id: &str) -> String {
    hex::encode(id.as_bytes())
}

fn decode_cursor(cursor: &str) -> Result<String, UserInboxError> {
    let bytes = hex::decode(cursor)
        .map_err(|_| UserInboxError::Validation("invalid cursor".to_string()))?;
    String::from_utf8(bytes).map_err(|_| UserInboxError::Validation("invalid cursor".to_string()))
}
