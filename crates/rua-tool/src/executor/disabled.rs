use async_trait::async_trait;

use super::{CommandError, CommandExecutor, CommandOutput, CommandRequest};

pub struct DisabledCommandExecutor {
    reason: String,
}

impl DisabledCommandExecutor {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Default for DisabledCommandExecutor {
    fn default() -> Self {
        Self::new("shell execution is not available on this platform")
    }
}

#[async_trait]
impl CommandExecutor for DisabledCommandExecutor {
    async fn execute(
        &self,
        _req: CommandRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<CommandOutput, CommandError> {
        Err(CommandError::Disabled(self.reason.clone()))
    }

    fn executor_type(&self) -> &'static str {
        "disabled"
    }
}
