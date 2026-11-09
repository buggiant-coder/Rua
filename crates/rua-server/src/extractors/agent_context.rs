use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;

use rua_workspace::WorkspaceRuntime;

#[derive(Clone)]
pub struct AgentExtractorContext {
    pub agent_id: String,

    pub runtime: Arc<WorkspaceRuntime>,
}

impl<S> FromRequestParts<S> for AgentExtractorContext
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<AgentExtractorContext>() {
            Some(ctx) => Ok(ctx.clone()),
            None => {
                tracing::error!(
                    path = %parts.uri.path(),
                    "AgentExtractorContext extractor invoked on a route the agent_context_middleware does not cover"
                );
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}
