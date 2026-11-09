use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Method};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use uuid::Uuid;

use crate::core::streaming::{StreamHandler, StreamType};
use crate::dingtalk::token::AccessTokenManager;
use crate::http_client::default_http_client;
use rua_core::Result;

const CREATE_CARD_URL: &str = "https://api.dingtalk.com/v1.0/card/instances";
const DELIVER_CARD_URL: &str = "https://api.dingtalk.com/v1.0/card/instances/deliver";
const STREAMING_UPDATE_URL: &str = "https://api.dingtalk.com/v1.0/card/streaming";
const AI_CARD_PROCESSING_TEXT: &str = "处理中...";

const STREAM_MIN_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCardRequest {
    card_template_id: String,
    out_track_id: String,
    card_data: CardData,
    callback_type: String,
    im_group_open_space_model: OpenSpaceModel,
    im_robot_open_space_model: OpenSpaceModel,
    #[serde(skip_serializing_if = "Option::is_none")]
    card_at_user_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id_type: Option<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CardData {
    card_param_map: HashMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenSpaceModel {
    support_forward: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliverCardRequest {
    out_track_id: String,
    user_id_type: u8,
    open_space_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    im_group_open_deliver_model: Option<GroupDeliverModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    im_robot_open_deliver_model: Option<RobotDeliverModel>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupDeliverModel {
    robot_code: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RobotDeliverModel {
    space_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamingUpdateRequest {
    out_track_id: String,
    guid: String,
    key: String,
    content: String,
    is_full: bool,
    is_finalize: bool,
    is_error: bool,
}

pub fn build_create_card_body(
    card_template_id: &str,
    out_track_id: &str,
    card_template_key: &str,
    sender_staff_id: &str,
    is_group: bool,
) -> serde_json::Value {
    build_create_card_body_with_initial_text(
        card_template_id,
        out_track_id,
        card_template_key,
        "",
        sender_staff_id,
        is_group,
    )
}

pub fn build_create_card_body_with_initial_text(
    card_template_id: &str,
    out_track_id: &str,
    card_template_key: &str,
    initial_text: &str,
    sender_staff_id: &str,
    is_group: bool,
) -> serde_json::Value {
    let mut card_param_map = HashMap::new();
    card_param_map.insert(card_template_key.to_string(), initial_text.to_string());

    let (card_at_user_ids, user_id_type) = if is_group && !sender_staff_id.is_empty() {
        (Some(vec![sender_staff_id.to_string()]), Some(1u8))
    } else {
        (None, None)
    };

    let body = CreateCardRequest {
        card_template_id: card_template_id.to_string(),
        out_track_id: out_track_id.to_string(),
        card_data: CardData { card_param_map },
        callback_type: "STREAM".to_string(),
        im_group_open_space_model: OpenSpaceModel {
            support_forward: true,
        },
        im_robot_open_space_model: OpenSpaceModel {
            support_forward: true,
        },
        card_at_user_ids,
        user_id_type,
    };
    serde_json::to_value(body).expect("CreateCardRequest serializes")
}

pub fn build_deliver_card_body(
    out_track_id: &str,
    robot_code: &str,
    conversation_id: &str,
    sender_staff_id: &str,
    is_group: bool,
) -> serde_json::Value {
    let (open_space_id, group_model, robot_model) = if is_group {
        (
            format!("dtv1.card//IM_GROUP.{conversation_id}"),
            Some(GroupDeliverModel {
                robot_code: robot_code.to_string(),
            }),
            None,
        )
    } else {
        (
            format!("dtv1.card//IM_ROBOT.{sender_staff_id}"),
            None,
            Some(RobotDeliverModel {
                space_type: "IM_ROBOT".to_string(),
            }),
        )
    };

    let body = DeliverCardRequest {
        out_track_id: out_track_id.to_string(),
        user_id_type: 1,
        open_space_id,
        im_group_open_deliver_model: group_model,
        im_robot_open_deliver_model: robot_model,
    };
    serde_json::to_value(body).expect("DeliverCardRequest serializes")
}

pub fn build_streaming_update_body(
    out_track_id: &str,
    card_template_key: &str,
    content: &str,
    finalize: bool,
) -> serde_json::Value {
    let body = StreamingUpdateRequest {
        out_track_id: out_track_id.to_string(),
        guid: Uuid::new_v4().to_string(),
        key: card_template_key.to_string(),
        content: content.to_string(),
        is_full: true,
        is_finalize: finalize,
        is_error: false,
    };
    serde_json::to_value(body).expect("StreamingUpdateRequest serializes")
}

struct SinkState {
    created: bool,
    last_content: String,
    last_update: Option<Instant>,
}

pub struct DingTalkCardSink {
    token_manager: Arc<AccessTokenManager>,
    http: Client,
    robot_code: String,
    card_template_id: String,
    card_template_key: String,
    conversation_id: String,
    sender_staff_id: String,
    is_group: bool,
    auto_layout: bool,
    mention_sender: bool,
    api_base: String,
    out_track_id: String,
    state: Mutex<SinkState>,
}

impl DingTalkCardSink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        token_manager: Arc<AccessTokenManager>,
        robot_code: String,
        card_template_id: String,
        card_template_key: String,
        conversation_id: String,
        sender_staff_id: String,
        is_group: bool,
    ) -> Self {
        Self {
            token_manager,
            http: default_http_client(),
            robot_code,
            card_template_id,
            card_template_key,
            conversation_id,
            sender_staff_id,
            is_group,
            auto_layout: false,
            mention_sender: true,
            api_base: "https://api.dingtalk.com".into(),
            out_track_id: format!("card_{}", Uuid::new_v4()),
            state: Mutex::new(SinkState {
                created: false,
                last_content: String::new(),
                last_update: None,
            }),
        }
    }

    pub fn with_options(mut self, auto_layout: bool, mention_sender: bool) -> Self {
        self.auto_layout = auto_layout;
        self.mention_sender = mention_sender;
        self
    }

    pub fn with_api_base(mut self, api_base: String) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_string();
        self
    }

    async fn token(&self) -> Result<String> {
        self.token_manager.current_token().await
    }

    async fn send_json(
        &self,
        method: Method,
        url: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let url = url.replacen("https://api.dingtalk.com", &self.api_base, 1);
        let resp = self
            .http
            .request(method, &url)
            .header("x-acs-dingtalk-access-token", token)
            .json(body)
            .send()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("dingtalk card request to {url} failed: {e}"),
            })?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(rua_core::RuaError::Runtime {
            message: format!("dingtalk card {url} HTTP {status}: {text}"),
        })
    }

    async fn create_and_deliver(&self) -> Result<()> {
        let token = self.token().await?;

        let mut create_body = build_create_card_body_with_initial_text(
            &self.card_template_id,
            &self.out_track_id,
            &self.card_template_key,
            AI_CARD_PROCESSING_TEXT,
            if self.mention_sender {
                &self.sender_staff_id
            } else {
                ""
            },
            self.is_group,
        );
        if self.auto_layout {
            create_body["cardData"]["cardParamMap"]["config"] =
                serde_json::json!("{\"autoLayout\":true}");
        }
        self.send_json(Method::POST, CREATE_CARD_URL, &token, &create_body)
            .await?;

        let deliver_body = build_deliver_card_body(
            &self.out_track_id,
            &self.robot_code,
            &self.conversation_id,
            &self.sender_staff_id,
            self.is_group,
        );
        self.send_json(Method::POST, DELIVER_CARD_URL, &token, &deliver_body)
            .await?;

        Ok(())
    }

    async fn streaming_update(&self, content: &str, finalize: bool) -> Result<()> {
        let body = build_streaming_update_body(
            &self.out_track_id,
            &self.card_template_key,
            content,
            finalize,
        );

        let token = self.token().await?;
        match self
            .send_json(Method::PUT, STREAMING_UPDATE_URL, &token, &body)
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if is_unauthorized(&e) => {
                let token = self.token().await?;
                self.send_json(Method::PUT, STREAMING_UPDATE_URL, &token, &body)
                    .await
            }
            Err(e) => Err(e),
        }
    }
}

fn is_unauthorized(err: &rua_core::RuaError) -> bool {
    err.to_string().contains("HTTP 401")
}

#[async_trait]
impl StreamHandler for DingTalkCardSink {
    async fn on_start(&self, stream_type: StreamType) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        self.create_and_deliver().await?;
        let mut state = self.state.lock().await;
        state.created = true;
        Ok(())
    }

    async fn on_delta(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        let content = accumulated.trim().to_string();
        if content.is_empty() {
            return Ok(());
        }

        {
            let state = self.state.lock().await;
            if !state.created {
                return Ok(());
            }
            if content == state.last_content {
                return Ok(());
            }
            if let Some(last) = state.last_update {
                if last.elapsed() < STREAM_MIN_INTERVAL {
                    return Ok(());
                }
            }
        }

        self.streaming_update(&content, false).await?;

        let mut state = self.state.lock().await;
        state.last_content = content;
        state.last_update = Some(Instant::now());
        Ok(())
    }

    async fn on_end(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        {
            let state = self.state.lock().await;
            if !state.created {
                return Ok(());
            }
        }
        let content = accumulated.trim().to_string();
        if content.is_empty() {
            return Ok(());
        }

        self.streaming_update(&content, true).await?;

        let mut state = self.state.lock().await;
        state.last_content = content;
        state.last_update = Some(Instant::now());
        Ok(())
    }
}
