use serde::Serialize;

use crate::dingtalk::token::AccessTokenManager;
use crate::http_client::default_http_client;

const REPLY_URL: &str = "https://api.dingtalk.com/v1.0/robot/emotion/reply";
const RECALL_URL: &str = "https://api.dingtalk.com/v1.0/robot/emotion/recall";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmotionRequest {
    robot_code: String,
    open_msg_id: String,
    open_conversation_id: String,
    emotion_type: u32,
    emotion_name: String,
    text_emotion: TextEmotion,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextEmotion {
    emotion_id: String,
    emotion_name: String,
    text: String,
    background_id: String,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct EmotionEndpointUrls<'a> {
    pub reply: &'a str,
    pub recall: &'a str,
}

pub async fn send_emotion(
    token_manager: &AccessTokenManager,
    robot_code: &str,
    open_msg_id: &str,
    open_conversation_id: &str,
    emoji_name: &str,
    recall: bool,
) {
    send_emotion_with_urls(
        token_manager,
        robot_code,
        open_msg_id,
        open_conversation_id,
        emoji_name,
        recall,
        EmotionEndpointUrls {
            reply: REPLY_URL,
            recall: RECALL_URL,
        },
    )
    .await;
}

#[doc(hidden)]
pub async fn send_emotion_with_urls(
    token_manager: &AccessTokenManager,
    robot_code: &str,
    open_msg_id: &str,
    open_conversation_id: &str,
    emoji_name: &str,
    recall: bool,
    urls: EmotionEndpointUrls<'_>,
) {
    let token = match token_manager.current_token().await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!("dingtalk emotion: token unavailable: {e}");
            return;
        }
    };

    let url = if recall { urls.recall } else { urls.reply };
    let body = build_emotion_request(robot_code, open_msg_id, open_conversation_id, emoji_name);

    let action = if recall { "recall" } else { "reply" };
    if let Err(e) = do_request(&token, url, &body).await {
        tracing::debug!("dingtalk emotion {action} failed: {e}");
    } else {
        tracing::debug!("dingtalk emotion {action} {emoji_name} on msg={open_msg_id}");
    }
}

#[doc(hidden)]
pub fn build_emotion_payload(
    robot_code: &str,
    open_msg_id: &str,
    open_conversation_id: &str,
    emoji_name: &str,
) -> serde_json::Value {
    serde_json::to_value(build_emotion_request(
        robot_code,
        open_msg_id,
        open_conversation_id,
        emoji_name,
    ))
    .expect("emotion request serializes")
}

fn build_emotion_request(
    robot_code: &str,
    open_msg_id: &str,
    open_conversation_id: &str,
    emoji_name: &str,
) -> EmotionRequest {
    EmotionRequest {
        robot_code: robot_code.to_string(),
        open_msg_id: open_msg_id.to_string(),
        open_conversation_id: open_conversation_id.to_string(),
        emotion_type: 2,
        emotion_name: emoji_name.to_string(),
        text_emotion: TextEmotion {
            emotion_id: "2659900".to_string(),
            emotion_name: emoji_name.to_string(),
            text: emoji_name.to_string(),
            background_id: "im_bg_1".to_string(),
        },
    }
}

async fn do_request(token: &str, url: &str, body: &EmotionRequest) -> Result<(), String> {
    let client = default_http_client();
    let resp = client
        .post(url)
        .header("x-acs-dingtalk-access-token", token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("http error: {e}"))?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(format!("HTTP {status}: {text}"))
    }
}
