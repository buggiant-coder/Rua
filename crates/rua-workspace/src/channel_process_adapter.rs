use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use rua_channel::core::channel::Process;
use rua_channel::core::console::{submit_runtime_turn, ConsoleChatFacade, ConsoleChatRequest};
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_core::{RuaError, RuaRoot, RuntimeEvent};
use rua_session::chat_deletion::ChatDeleteIntentStore;
use rua_session::external_chat::{ExternalChatBinding, ExternalChatRoute};
use rua_session::WorkspaceSessionStore;
use serde_json::{json, Value};

pub struct ChannelProcessAdapter {
    rua_root: PathBuf,
    subscription_agent_id: String,
    facade: Weak<dyn ConsoleChatFacade>,
}

impl ChannelProcessAdapter {
    pub fn new(
        rua_root: PathBuf,
        subscription_agent_id: String,
        facade: Weak<dyn ConsoleChatFacade>,
    ) -> Self {
        Self {
            rua_root,
            subscription_agent_id,
            facade,
        }
    }

    fn route(&self, req: &AgentChannelRequest) -> ExternalChatRoute {
        ExternalChatRoute {
            account_id: self.subscription_agent_id.clone(),
            platform_account_id: ["account_id", "robot_code", "bot_id"]
                .iter()
                .find_map(|key| {
                    req.metadata
                        .get(key)?
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                }),
            channel: req.channel_id.clone(),
            session_id: req.session_id.clone(),
            conversation_id: req.conversation_id.clone(),
        }
    }

    async fn target(
        &self,
        req: &AgentChannelRequest,
        reserve: bool,
    ) -> Result<Option<(ExternalChatBinding, String)>, String> {
        let Some(binding) = self
            .route(req)
            .resolve(&self.rua_root, reserve)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        if ChatDeleteIntentStore::from_rua_root(&self.rua_root)
            .load(&binding.address)?
            .is_some()
        {
            return Err(format!("Chat {} was deleted", binding.address));
        }
        let root = RuaRoot::new(&self.rua_root);
        let header = WorkspaceSessionStore::new(root.chat_workspace().as_path())
            .load_chat_header(binding.address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?;
        let agent = match header {
            Some(header) => header.agent_id,
            None if binding.admitted => {
                return Err(format!("Chat {} is unavailable", binding.address))
            }
            None if !reserve => return Ok(None),
            None if !req.agent_id.is_empty() => req.agent_id.clone(),
            None => self.subscription_agent_id.clone(),
        };
        Ok(Some((binding, agent)))
    }

    fn facade(&self) -> Result<Arc<dyn ConsoleChatFacade>, String> {
        self.facade
            .upgrade()
            .ok_or_else(|| "Chat admission service is unavailable".into())
    }

    async fn submit(
        &self,
        req: AgentChannelRequest,
    ) -> Result<BoxStream<'static, RuntimeEvent>, String> {
        let facade = self.facade()?;
        let (binding, agent) = self
            .target(&req, true)
            .await?
            .ok_or("external Chat binding is unavailable")?;
        let content: Vec<_> = req.input.iter().map(channel_part).collect();
        let message = req
            .input
            .iter()
            .filter_map(|part| match part {
                IncomingContentPart::Text { text } => Some(text.as_str()),
                IncomingContentPart::Url { url, .. } => Some(url.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut extra = serde_json::Map::new();
        extra.insert("channel".into(), json!(req.channel_id));
        extra.insert(
            "user_id".into(),
            json!(rua_channel::core::pipeline::acl_sender_id(&req)),
        );
        if let Some(name) = ["sender_nick", "user_name", "username"]
            .iter()
            .find_map(|key| {
                req.metadata
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
            })
        {
            extra.insert("sender_name".into(), json!(name));
        }
        if let Some(id) = req.message_id.filter(|id| !id.is_empty()) {
            let id = binding.platform_message_key(&id);
            let turn_id = format!("turn-im-{id}");
            let store =
                WorkspaceSessionStore::new(RuaRoot::new(&self.rua_root).chat_workspace().as_path());
            if store
                .read_turn(binding.address.chat_id().as_str(), &turn_id)
                .await
                .map_err(|e| e.to_string())?
                .is_some_and(|turn| {
                    rua_session::TurnLifecycleStatus::parse(&turn.lifecycle.status)
                        .is_syncable_conversation_state()
                })
            {
                return Ok(stream::empty().boxed());
            }
            extra.insert("turn_id".into(), json!(turn_id));
            extra.insert("submission_id".into(), json!(format!("submission-im-{id}")));
        }
        let events = submit_runtime_turn(
            Arc::clone(&facade),
            binding.address.clone(),
            &agent,
            ConsoleChatRequest {
                message,
                chat_id: Some(binding.address.chat_id().to_string()),
                model: None,
                is_reconnect: false,
                attachments: vec![],
                input: Some(vec![json!({"role":"user","content":content})]),
                messages: None,
                extra,
                channel_kind: None,
            },
        )
        .await?;
        binding
            .confirm(&self.rua_root)
            .await
            .map_err(|e| e.to_string())?;
        Ok(events
            .map(|event| event.unwrap_or_else(|error| RuntimeEvent::Error { error }))
            .boxed())
    }

    async fn control(
        &self,
        req: &AgentChannelRequest,
        command: &str,
        approval_id: Option<&str>,
    ) -> Result<String, String> {
        let Some((binding, agent)) = self.target(req, false).await? else {
            return Ok("当前没有可操作的对话".into());
        };
        let facade = self.facade()?;
        if command == "/stop" {
            return Ok(if facade
                .stop_chat(&agent, binding.address.chat_id().as_str())
                .await
            {
                "已停止当前任务"
            } else {
                "当前没有正在执行的任务"
            }
            .into());
        }
        let approve = command == "/approve";
        let decided = facade
            .decide_chat_approval(
                &binding.address,
                &agent,
                approval_id,
                approve,
                rua_channel::core::pipeline::acl_sender_id(req),
            )
            .await?;
        Ok(if !decided {
            "当前没有待审批请求"
        } else if approve {
            "已批准"
        } else {
            "已拒绝"
        }
        .into())
    }
}

fn channel_part(part: &IncomingContentPart) -> Value {
    let url = |path: &Path| format!("file://{}", path.display());
    match part {
        IncomingContentPart::Text { text } => json!({"type":"text","text":text}),
        IncomingContentPart::Url { url, .. } => json!({"type":"text","text":url}),
        IncomingContentPart::Image { path, mime, .. } => {
            json!({"type":"image","url":url(path),"mime_type":mime})
        }
        IncomingContentPart::File {
            path, mime, name, ..
        } => json!({"type":"file","url":url(path),"mime_type":mime,"name":name}),
        IncomingContentPart::Audio { path, mime, .. } => {
            json!({"type":"audio","url":url(path),"mime_type":mime})
        }
        IncomingContentPart::Video { path, mime, .. } => {
            json!({"type":"video","url":url(path),"mime_type":mime})
        }
    }
}

#[async_trait]
impl Process for ChannelProcessAdapter {
    async fn control_chat_id(&self, req: &AgentChannelRequest) -> rua_core::Result<Option<String>> {
        self.target(req, false)
            .await
            .map(|result| result.map(|(binding, _)| binding.address.chat_id().to_string()))
            .map_err(|message| RuaError::Runtime { message })
    }

    async fn handle_chat_control(&self, req: &AgentChannelRequest) -> Option<RuntimeEvent> {
        let text = rua_channel::core::extract_query_text_from_request(req);
        let mut words = text.split_whitespace();
        let command = words.next()?;
        if !command.starts_with('/') {
            return None;
        }
        let command = command.to_ascii_lowercase();
        let (command, id) = match command.as_str() {
            "/stop" | "/approve" | "/deny" => (command.as_str(), None),
            "/daemon" | "/approval" => {
                let action = words.next()?.to_ascii_lowercase();
                let decision = match action.as_str() {
                    "approve" => "/approve",
                    "deny" | "reject" => "/deny",
                    _ => return None,
                };
                if command == "/approval" {
                    (decision, Some(words.next()?))
                } else {
                    (decision, None)
                }
            }
            _ => return None,
        };
        Some(match self.control(req, command, id).await {
            Ok(delta) => RuntimeEvent::TextDelta {
                delta,
                message_id: None,
            },
            Err(message) => RuntimeEvent::Error {
                error: RuaError::Runtime { message },
            },
        })
    }

    async fn handle(&self, req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        if let Some(control) = self.handle_chat_control(&req).await {
            return stream::iter([control]).boxed();
        }
        match self.submit(req).await {
            Ok(events) => events,
            Err(message) => stream::iter([RuntimeEvent::Error {
                error: RuaError::Runtime { message },
            }])
            .boxed(),
        }
    }
}
