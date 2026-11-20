use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::errors::ApiError;
use crate::services::chat_context::load_chat_context;
use crate::state::AppState;

/// Extracts the saved executor followed by the canonical Chat path parameters.
/// The executor is read from the header; it is not part of the public address.
pub struct ChatPath<T>(pub T);

impl<T> FromRequestParts<AppState> for ChatPath<T>
where
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(parameters) = Path::<Vec<(String, String)>>::from_request_parts(parts, state)
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        let chat_id = parameters
            .iter()
            .find(|(key, _)| matches!(key.as_str(), "id" | "chatId" | "chat_id"))
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| ApiError::BadRequest("Chat ID is required".into()))?;
        let context = load_chat_context(&state.rua_root, chat_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Chat not found: {chat_id}")))?;
        let mut values = vec![serde_json::Value::String(context.agent_id)];
        values.extend(
            parameters
                .into_iter()
                .map(|(_, value)| serde_json::Value::String(value)),
        );
        serde_json::from_value(serde_json::Value::Array(values))
            .map(Self)
            .map_err(|error| ApiError::Internal(error.to_string()))
    }
}

impl FromRequestParts<AppState> for crate::services::chat_context::ChatContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        #[derive(serde::Deserialize)]
        struct ChatIdPath {
            #[serde(rename = "chatId")]
            chat_id: String,
        }
        let Path(path) = Path::<ChatIdPath>::from_request_parts(parts, state)
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        load_chat_context(&state.rua_root, &path.chat_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Chat not found: {}", path.chat_id)))
    }
}
