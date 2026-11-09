//! Time budgets include resource waits and native execution.
use mcp_manager::{
    error::{McpError, McpResult},
    types::McpCallContext,
};
use std::time::Duration;
use tokio::time::Instant;

pub struct CallScope {
    pub deadline: Instant,
    server: String,
    tool: String,
    budget: Duration,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
}
impl CallScope {
    pub fn new(ctx: &McpCallContext, server: &str, tool: &str, maximum: Duration) -> Self {
        let budget = ctx.timeout.unwrap_or(maximum).min(maximum);
        Self {
            deadline: Instant::now() + budget,
            server: server.into(),
            tool: tool.into(),
            budget,
            cancel: ctx.cancel.clone(),
        }
    }
    pub fn check(&self) -> McpResult<()> {
        if self.cancel.as_ref().is_some_and(|cancel| *cancel.borrow()) {
            return Err(McpError::Cancelled {
                server: self.server.clone(),
            });
        }
        if Instant::now() >= self.deadline {
            return Err(self.timeout());
        }
        Ok(())
    }
    fn timeout(&self) -> McpError {
        McpError::CallTimeout {
            server: self.server.clone(),
            tool: self.tool.clone(),
            timeout_secs: self.budget.as_secs(),
        }
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    pub async fn wait<T>(&self, future: impl std::future::Future<Output = T>) -> McpResult<T> {
        self.check()?;
        let mut cancel = self.cancel.clone();
        tokio::select! {
            biased;
            _ = async {
                if let Some(cancel)=&mut cancel {
                    loop {
                        if *cancel.borrow() { break; }
                        if cancel.changed().await.is_err() { std::future::pending::<()>().await; }
                    }
                } else { std::future::pending::<()>().await; }
            } => Err(McpError::Cancelled {server:self.server.clone()}),
            result=tokio::time::timeout_at(self.deadline,future) => result.map_err(|_|self.timeout()),
        }
    }
}
