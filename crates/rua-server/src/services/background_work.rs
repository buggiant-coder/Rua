use std::path::PathBuf;
use std::sync::{Arc, Weak};

use rua_core::{ChatAddress, ChatScopeStore};
use serde::{Deserialize, Serialize};

pub(crate) const BACKGROUND_RESULT_KEY: &str = "background_result";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundWorkView {
    pub suppressed: bool,
    pub items: Vec<BackgroundWorkItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundWorkItem {
    pub kind: String,
    pub id: String,
    pub status: String,
    pub chat: ChatAddress,
}

pub struct BackgroundWork {
    root: PathBuf,
    tasks: Arc<rua_task::TaskRuntime>,
    authority: Weak<super::conversation_authority::ConversationAuthority>,
}

impl BackgroundWork {
    pub fn new(
        root: PathBuf,
        tasks: Arc<rua_task::TaskRuntime>,
        authority: Weak<super::conversation_authority::ConversationAuthority>,
    ) -> Self {
        Self {
            root,
            tasks,
            authority,
        }
    }

    pub async fn view(&self, address: &ChatAddress) -> BackgroundWorkView {
        match self.load(address).await {
            Ok(view) => view,
            Err(error) => BackgroundWorkView {
                error: Some(error),
                ..Default::default()
            },
        }
    }

    pub async fn suppressed(&self, address: &ChatAddress) -> Result<bool, String> {
        let scope = ChatScopeStore::resolve(&self.root, address.clone());
        rua_session::WorkspaceSessionStore::new(scope.root())
            .load_collaboration_wake_state(address.chat_id().as_str())
            .await
            .map(|state| state.suppressed)
            .map_err(|e| e.to_string())
    }

    async fn load(&self, address: &ChatAddress) -> Result<BackgroundWorkView, String> {
        let scope = ChatScopeStore::resolve(&self.root, address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let mut view = BackgroundWorkView {
            suppressed: store
                .load_collaboration_wake_state(address.chat_id().as_str())
                .await
                .map_err(|e| e.to_string())?
                .suppressed,
            ..Default::default()
        };
        let owner = rua_task::TaskOwner::from_address(address.clone());
        let mut offset = 0;
        loop {
            let tasks = self
                .tasks
                .list(
                    &owner,
                    rua_task::TaskListFilter {
                        limit: Some(100),
                        offset,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            let count = tasks.len();
            for task in tasks.into_iter().filter(|task| !task.state.is_terminal()) {
                view.items.push(BackgroundWorkItem {
                    kind: "task".into(),
                    id: task.task_id,
                    status: serde_json::to_value(task.state)
                        .map_err(|e| e.to_string())?
                        .as_str()
                        .unwrap_or("unknown")
                        .into(),
                    chat: address.clone(),
                });
            }
            if count < 100 {
                break;
            }
            offset += count as u64;
        }
        for task in self
            .tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .map_err(|e| e.to_string())?
        {
            view.items.push(BackgroundWorkItem {
                kind: "task".into(),
                status: if !view.suppressed
                    && self
                        .tasks
                        .terminal_delivery_error(&owner, &task.task_id, task.attempt_no)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_some()
                {
                    "delivery_failed"
                } else {
                    "result_ready"
                }
                .into(),
                id: task.task_id,
                chat: address.clone(),
            });
        }
        for target in store
            .load_collaboration_targets(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
        {
            if let Some(authority) = self.authority.upgrade() {
                let Some(turn) = authority.active_turn(&target).await else {
                    continue;
                };
                view.items.push(BackgroundWorkItem {
                    kind: "collaboration".into(),
                    id: turn.turn_id,
                    status: "running".into(),
                    chat: target,
                });
            }
        }
        Ok(view)
    }

    pub async fn collaboration_source(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<(ChatAddress, serde_json::Value)>, String> {
        let scope = ChatScopeStore::resolve(&self.root, address.clone());
        let header = rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?;
        let descriptor = header
            .and_then(|header| {
                header
                    .extra
                    .get(rua_session::collaboration::COLLABORATION_META_KEY)
                    .cloned()
            })
            .map(serde_json::from_value::<rua_session::collaboration::CollaborationDescriptor>)
            .transpose()
            .map_err(|e| e.to_string())?;
        let Some(descriptor) = descriptor else {
            return Ok(None);
        };
        let scope = ChatScopeStore::resolve(&self.root, descriptor.source.clone());
        let header = rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(descriptor.source.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?;
        Ok(header.map(|header| (descriptor.source, serde_json::json!(header.extra))))
    }
}
