use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_types::ModelDialogueItem;
use async_trait::async_trait;
use react_agent::{AgentError, AgentHook};
use react_agent::{ModelCallSection, ModelCallSectionKind};

const BOOTSTRAP_FILE: &str = "BOOTSTRAP.md";
const BOOTSTRAP_FLAG: &str = ".bootstrap_completed";

pub struct BootstrapContextContributor {
    working_dir: PathBuf,
    injected: AtomicBool,
}

impl BootstrapContextContributor {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            injected: AtomicBool::new(false),
        }
    }

    fn flag_path(&self) -> PathBuf {
        self.working_dir.join(BOOTSTRAP_FLAG)
    }

    fn bootstrap_path(&self) -> PathBuf {
        self.working_dir.join(BOOTSTRAP_FILE)
    }

    pub async fn contribute(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
    ) -> Option<ModelCallSection> {
        if iteration != 0 {
            return None;
        }

        if self.flag_path().exists() {
            return None;
        }

        if !is_first_user_interaction(messages) {
            return None;
        }

        let bootstrap_content = match tokio::fs::read_to_string(self.bootstrap_path()).await {
            Ok(content) => content,
            Err(_) => return None,
        };

        if bootstrap_content.is_empty() {
            return None;
        }

        self.injected.store(true, Ordering::Relaxed);
        Some(ModelCallSection::new(
            ModelCallSectionKind::BootstrapContext,
            format!("[Bootstrap Context]\n{bootstrap_content}"),
        ))
    }

    pub async fn finish(&self) {
        if !self.injected.load(Ordering::Relaxed) {
            return;
        }
        if let Err(e) = tokio::fs::write(self.flag_path(), "").await {
            tracing::warn!(
                path = %self.flag_path().display(),
                error = %e,
                "Failed to write bootstrap flag"
            );
        }
    }
}

#[async_trait]
impl AgentHook for BootstrapContextContributor {
    async fn on_finish(&self, _iterations: u32, _result: &Result<(), AgentError>) {
        self.finish().await;
    }
}

fn is_first_user_interaction(messages: &[ModelDialogueItem]) -> bool {
    let mut non_system = messages.iter().filter(|msg| {
        !matches!(
            msg.role,
            agent_types::Role::System | agent_types::Role::Developer
        )
    });

    matches!(
        non_system.next().map(|msg| msg.role),
        Some(agent_types::Role::User)
    ) && non_system.next().is_none()
}
