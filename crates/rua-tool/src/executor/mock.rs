use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{CommandError, CommandExecutor, CommandOutput, CommandRequest};

#[derive(Debug, Clone, Default)]
pub struct MockResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,

    pub delay_ms: u64,
}

pub struct MockCommandExecutor {
    responses: Mutex<HashMap<String, MockResponse>>,
    default_response: MockResponse,

    history: Mutex<Vec<CommandRequest>>,
}

impl MockCommandExecutor {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default_response: MockResponse::default(),
            history: Mutex::new(Vec::new()),
        }
    }

    pub fn add_response(&self, command_prefix: impl Into<String>, response: MockResponse) {
        self.responses
            .lock()
            .unwrap()
            .insert(command_prefix.into(), response);
    }

    pub fn set_default_response(&mut self, response: MockResponse) {
        self.default_response = response;
    }

    pub fn history(&self) -> Vec<CommandRequest> {
        self.history.lock().unwrap().clone()
    }
}

impl Default for MockCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandExecutor for MockCommandExecutor {
    async fn execute(
        &self,
        req: CommandRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<CommandOutput, CommandError> {
        self.history.lock().unwrap().push(req.clone());

        let response = {
            let responses = self.responses.lock().unwrap();
            responses
                .iter()
                .find(|(prefix, _)| req.command.starts_with(prefix.as_str()))
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| self.default_response.clone())
        };

        if response.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(response.delay_ms)).await;
        }

        Ok(CommandOutput {
            stdout: response.stdout,
            stderr: response.stderr,
            exit_code: Some(response.exit_code),
            timed_out: false,
            cancelled: false,
            duration_ms: response.delay_ms,
            raw_capture: None,
        })
    }

    fn executor_type(&self) -> &'static str {
        "mock"
    }
}
