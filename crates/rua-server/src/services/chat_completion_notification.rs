use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use crate::services::agent_manager::AgentManager;
use crate::services::conversation_authority::ConversationTurnTerminalFact;
use crate::services::user_notification::{
    UserNotificationRequest, UserNotificationSender, UserNotificationService,
};

pub struct ChatCompletionNotifier {
    local_device_id: String,
    rua_root: PathBuf,
    device_list_path: PathBuf,
    notifications: Arc<UserNotificationService>,
    agent_service: Arc<AgentManager>,
    collaboration: Arc<OnceLock<Weak<crate::services::collaboration::CollaborationService>>>,
    background_work: OnceLock<Arc<crate::services::background_work::BackgroundWork>>,
}

impl ChatCompletionNotifier {
    pub fn new(
        local_device_id: String,
        rua_root: PathBuf,
        sync_dir: PathBuf,
        notifications: Arc<UserNotificationService>,
        agent_service: Arc<AgentManager>,
        collaboration: Arc<OnceLock<Weak<crate::services::collaboration::CollaborationService>>>,
    ) -> Self {
        Self {
            local_device_id,
            rua_root,
            device_list_path: sync_dir.join("device_list.json"),
            notifications,
            agent_service,
            collaboration,
            background_work: OnceLock::new(),
        }
    }

    pub fn install_background_work(
        &self,
        work: Arc<crate::services::background_work::BackgroundWork>,
    ) {
        let _ = self.background_work.set(work);
    }

    pub async fn notify(&self, fact: &ConversationTurnTerminalFact) {
        if let Some(service) = self.collaboration.get().and_then(Weak::upgrade) {
            match service.settle_terminal_result(fact).await {
                Ok(Some(request_id)) => {
                    let fact = fact.clone();
                    tokio::spawn(async move {
                        if let Err(error) = service
                            .deliver_settled_terminal_result(&fact, request_id)
                            .await
                        {
                            tracing::warn!(
                                address = %fact.address,
                                turn_id = %fact.turn_id,
                                error = %error,
                                "failed to deliver collaboration result"
                            );
                        }
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        address = %fact.address,
                        turn_id = %fact.turn_id,
                        error = %error,
                        "failed to settle collaboration result"
                    );
                }
            }
        }
        if let Some(work) = self.background_work.get() {
            let view = work.view(&fact.address).await;
            if !view.items.is_empty() || view.error.is_some() {
                return;
            }
        }
        if fact.status != "completed" {
            return;
        }
        let scope_store =
            rua_core::chat_store::ChatScopeStore::resolve(&self.rua_root, fact.address.clone());
        let header = rua_session::WorkspaceSessionStore::new(scope_store.root())
            .load_chat_header(fact.address.chat_id().as_str())
            .await
            .ok()
            .flatten();
        let chat_title = header.as_ref().map(|header| header.metadata.title.as_str());
        let agent_id = header
            .as_ref()
            .map(|header| header.agent_id.trim())
            .filter(|agent_id| !agent_id.is_empty())
            .unwrap_or_else(|| fact.address.scope.owner_id());
        let agent_name = self
            .agent_service
            .get_agent(agent_id)
            .await
            .ok()
            .flatten()
            .map(|agent| agent.name);
        let Some(template) = request_for_completed_chat(
            fact,
            &self.local_device_id,
            chat_title,
            agent_id,
            agent_name.as_deref(),
        ) else {
            return;
        };
        let mut targets = BTreeSet::from([self.local_device_id.clone()]);
        if let Ok(contents) = std::fs::read_to_string(&self.device_list_path) {
            if let Ok(device_list) =
                serde_json::from_str::<rua_sync::device_list::DeviceList>(&contents)
            {
                targets.extend(
                    device_list
                        .devices
                        .into_iter()
                        .map(|device| hex::encode(device.device_id)),
                );
            }
        }
        let mut sends = tokio::task::JoinSet::new();
        for target_device_id in targets {
            let mut request = template.clone();
            request.target_device_id = target_device_id;
            let notifications = Arc::clone(&self.notifications);
            sends.spawn(async move { notifications.send(request).await });
        }
        while sends.join_next().await.is_some() {}
    }
}

pub fn request_for_completed_chat(
    fact: &ConversationTurnTerminalFact,
    target_device_id: &str,
    chat_title: Option<&str>,
    agent_id: &str,
    agent_name: Option<&str>,
) -> Option<UserNotificationRequest> {
    if fact.status != "completed" {
        return None;
    }
    let owner_id = fact.address.scope.owner_id();
    let owner_kind = match fact.address.scope.kind() {
        "agent" => "agents",
        "project" => "projects",
        _ => return None,
    };
    Some(UserNotificationRequest {
        target_device_id: target_device_id.to_string(),
        sender: UserNotificationSender {
            id: agent_id.to_string(),
            name: non_empty(agent_name).unwrap_or(agent_id).to_string(),
        },
        title: non_empty(chat_title)
            .map(str::to_string)
            .unwrap_or_else(|| format!("对话 {}", fact.address.chat_id())),
        body: Some("已完成回复".to_string()),
        rua_uri: Some(format!(
            "rua://{owner_kind}/{owner_id}/chats/{}?turn={}",
            fact.address.chat_id(),
            fact.turn_id
        )),
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
