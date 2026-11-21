use async_trait::async_trait;

use crate::model::{TaskAttemptAuthorization, TaskDeviceContext, TaskOwner, TaskSpec};

#[derive(Debug, Clone, thiserror::Error)]
pub enum TaskAuthorityError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unavailable(String),
}

#[async_trait]
pub trait TaskAuthorityResolver: Send + Sync {
    async fn accept_attempt(
        &self,
        spec: &TaskSpec,
        authorization: &TaskAttemptAuthorization,
        prior: Option<&TaskDeviceContext>,
    ) -> Result<TaskDeviceContext, TaskAuthorityError>;

    async fn is_current(
        &self,
        owner: &TaskOwner,
        accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError>;

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError>;
}
