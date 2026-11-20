//! A platform session can route to a Rua Chat without becoming its identity.
use std::path::{Path, PathBuf};

use rua_core::{ChatAddress, Result, RuaError, RuaRoot};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const BINDINGS_FILE: &str = "external-channel-bindings.sqlite";

pub const EXTERNAL_CHANNEL_META_KEY: &str = "external_channel";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalChatRoute {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_account_id: Option<String>,
    pub channel: String,
    /// Platform session after the channel's thread/group partitioning rules.
    pub session_id: String,
    /// Platform conversation for channel delivery.
    pub conversation_id: String,
}

#[derive(Debug, Clone)]
pub struct ExternalChatBinding {
    pub address: ChatAddress,
    pub admitted: bool,
}

impl ExternalChatRoute {
    fn path(rua_root: &Path) -> PathBuf {
        RuaRoot::new(rua_root)
            .chat_workspace()
            .as_path()
            .join(BINDINGS_FILE)
    }

    /// Reserve the same independent ID on simultaneous or retried first input.
    pub async fn resolve(
        &self,
        rua_root: &Path,
        reserve: bool,
    ) -> Result<Option<ExternalChatBinding>> {
        if self.account_id.trim().is_empty()
            || self.channel.trim().is_empty()
            || self.session_id.trim().is_empty()
        {
            return Err(RuaError::Validation {
                message: "external channel subscription, channel and session are required".into(),
            });
        }
        if let Some(binding) = self.local_binding(rua_root, None).await? {
            return Ok(Some(binding));
        }

        // A paired Device receives Chat Sessions, but channel subscriptions and their SQLite
        // database are local. Consult the durable headers only on a local binding miss.
        let store =
            crate::WorkspaceSessionStore::new(RuaRoot::new(rua_root).chat_workspace().as_path());
        let mut known = None;
        for header in store.list_chat_headers(0, usize::MAX).await? {
            let Some(value) = header.extra.get(EXTERNAL_CHANNEL_META_KEY) else {
                continue;
            };
            let route: ExternalChatRoute = serde_json::from_value(value.clone()).map_err(err)?;
            if route.channel == self.channel
                && route.account_id == self.account_id
                && route.platform_account_id == self.platform_account_id
                && route.session_id == self.session_id
            {
                if known.is_some() {
                    return Err(RuaError::Validation {
                        message: format!(
                            "multiple Chats claim the same external {} session",
                            self.channel
                        ),
                    });
                }
                known = Some(ExternalChatBinding {
                    address: ChatAddress::new(header.chat_id),
                    admitted: true,
                });
            }
        }
        let candidate = known.or_else(|| {
            reserve.then(|| ExternalChatBinding {
                address: ChatAddress::new(uuid::Uuid::new_v4().to_string()),
                admitted: false,
            })
        });
        match candidate {
            Some(binding) => self.local_binding(rua_root, Some(binding)).await,
            None => Ok(None),
        }
    }

    async fn local_binding(
        &self,
        rua_root: &Path,
        candidate: Option<ExternalChatBinding>,
    ) -> Result<Option<ExternalChatBinding>> {
        let path = Self::path(rua_root);
        let route = self.clone();
        tokio::task::spawn_blocking(move || {
            if candidate.is_none() && !path.try_exists().map_err(err)? {
                return Ok(None);
            }
            if candidate.is_some() {
                std::fs::create_dir_all(path.parent().expect("binding database parent")).map_err(err)?;
            }
            let db = open_bindings(&path)?;
            let platform_account = route.platform_account_id.as_deref().unwrap_or("");
            if let Some(candidate) = candidate.as_ref() {
                db.execute(
                    "INSERT INTO channel_binding
                     (account_id,platform_account_id,channel,session_id,conversation_id,chat_id,admitted)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(account_id,platform_account_id,channel,session_id) DO NOTHING",
                    params![route.account_id, platform_account, route.channel, route.session_id,
                        route.conversation_id, candidate.address.chat_id().as_str(), candidate.admitted],
                ).map_err(err)?;
            }
            let found = db.query_row(
                "SELECT chat_id, admitted FROM channel_binding
                 WHERE account_id=?1 AND platform_account_id=?2 AND channel=?3 AND session_id=?4",
                params![route.account_id, platform_account, route.channel, route.session_id],
                |row| Ok(ExternalChatBinding {
                    address: ChatAddress::new(row.get::<_, String>(0)?), admitted: row.get(1)?,
                }),
            ).optional().map_err(err)?;
            if let (Some(expected), Some(actual)) = (candidate.filter(|c| c.admitted), &found) {
                if expected.address != actual.address {
                    return Err(RuaError::Validation { message: "saved external Chat conflicts with local binding".into() });
                }
            }
            Ok(found)
        }).await.map_err(err)?
    }
}

/// Retrieve the trusted native route reserved by the channel adapter before admission.
pub async fn route_for_chat(
    chat_workspace: &Path,
    address: &ChatAddress,
) -> Result<Option<ExternalChatRoute>> {
    let path = chat_workspace.join(BINDINGS_FILE);
    let chat_id = address.chat_id().to_string();
    tokio::task::spawn_blocking(move || {
        if !path.try_exists().map_err(err)? {
            return Ok(None);
        }
        let db = open_bindings(&path)?;
        db.query_row(
            "SELECT account_id, channel, session_id, conversation_id, NULLIF(platform_account_id, '')
             FROM channel_binding WHERE chat_id=?1",
            [chat_id],
            |row| Ok(ExternalChatRoute {
                account_id: row.get(0)?,
                channel: row.get(1)?,
                session_id: row.get(2)?,
                conversation_id: row.get(3)?,
                platform_account_id: row.get(4)?,
            }),
        ).optional().map_err(err)
    }).await.map_err(err)?
}

fn open_bindings(path: &Path) -> Result<Connection> {
    let db = Connection::open(path).map_err(err)?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(err)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS channel_binding (
            account_id TEXT NOT NULL,
            platform_account_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            session_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            chat_id TEXT NOT NULL UNIQUE,
            admitted INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(account_id, platform_account_id, channel, session_id)
        )",
    )
    .map_err(err)?;
    Ok(db)
}

impl ExternalChatBinding {
    /// Platform message IDs may contain separators or be too long for a storage key.
    pub fn platform_message_key(&self, message_id: &str) -> String {
        format!(
            "{}-{}",
            self.address.chat_id(),
            blake3::hash(message_id.as_bytes()).to_hex()
        )
    }

    pub async fn confirm(&self, rua_root: &Path) -> Result<()> {
        if self.admitted {
            return Ok(());
        }
        let path = ExternalChatRoute::path(rua_root);
        let chat_id = self.address.chat_id().to_string();
        tokio::task::spawn_blocking(move || {
            let db = Connection::open(path).map_err(err)?;
            db.busy_timeout(std::time::Duration::from_secs(5))
                .map_err(err)?;
            db.execute(
                "UPDATE channel_binding SET admitted=1 WHERE chat_id=?1",
                [chat_id],
            )
            .map_err(err)?;
            Ok(())
        })
        .await
        .map_err(err)?
    }
}

fn err(error: impl std::fmt::Display) -> RuaError {
    RuaError::Io {
        message: format!("external Chat binding: {error}"),
    }
}
