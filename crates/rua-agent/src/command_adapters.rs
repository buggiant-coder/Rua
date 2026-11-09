use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use agent_types::{ModelDialogueItem, Role};
use rua_command::conversation::{
    ContextCompactorSeam, ConversationContext, ConversationSessionSeam, PendingActionInfo,
    PendingActionSeam, PromptFileSeam, SeamResult,
};
use rua_command::{ConversationCommand, ConversationCommandRegistry};

use crate::command::{ClearCommand, CompactCommand, NewCommand, PendingAction};
use crate::context_manager::{ContextManager, ContextSummaryRequest};
use crate::projection_tool_results::project_message_log_entries;

pub struct WorkspaceConversationSessionSeam {
    workspace_dir: PathBuf,
    agent_id: String,
    chat_address: Option<rua_core::ChatAddress>,
    running_config: Arc<rua_config::AgentsRunningConfig>,
    model: Option<rua_session::CheckpointModelIdentity>,
    replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
}

impl WorkspaceConversationSessionSeam {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            agent_id: String::new(),
            chat_address: None,
            running_config: Arc::new(rua_config::AgentsRunningConfig::default()),
            model: None,
            replication_hook: None,
        }
    }

    pub fn with_checkpoint_context(
        mut self,
        agent_id: String,
        chat_address: rua_core::ChatAddress,
        running_config: Option<Arc<rua_config::AgentsRunningConfig>>,
        model: Option<rua_session::CheckpointModelIdentity>,
        replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
    ) -> Self {
        self.agent_id = agent_id;
        self.chat_address = Some(chat_address);
        self.model = model;
        if let Some(running_config) = running_config {
            self.running_config = running_config;
        }
        self.replication_hook = replication_hook;
        self
    }
}

#[async_trait]
impl ConversationSessionSeam for WorkspaceConversationSessionSeam {
    async fn load_messages(&self, session_id: &str) -> SeamResult<Vec<ModelDialogueItem>> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        if store.load_chat_header(session_id).await?.is_none() {
            return Ok(Vec::new());
        }
        let chat_address = self
            .chat_address
            .clone()
            .unwrap_or_else(|| rua_core::ChatAddress::agent(&self.agent_id, session_id));
        let projection = store
            .build_effective_context_projection_for_address(
                &chat_address,
                rua_session::ContextProjectionRequest {
                    current_user_input: None,
                },
                self.running_config
                    .context_compact
                    .history_replacement_enabled,
                self.running_config
                    .context_compact
                    .token_count_estimate_divisor,
            )
            .await?;
        let messages = projection
            .retained_messages
            .into_iter()
            .chain(projection.tail_messages)
            .collect::<Vec<_>>();
        Ok(projection
            .summary
            .into_iter()
            .map(summary_chat_message)
            .chain(project_message_log_entries(&messages, &self.running_config))
            .collect())
    }

    async fn clear_messages(&self, session_id: &str) -> SeamResult<()> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        store.record_clear_projection_boundary(session_id).await?;
        Ok(())
    }

    async fn get_active_context_summary(&self, session_id: &str) -> SeamResult<Option<String>> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        Ok(store
            .load_active_summary_checkpoint(session_id)
            .await?
            .map(|checkpoint| checkpoint.summary))
    }

    async fn publish_context_checkpoint(
        &self,
        session_id: &str,
        summary: String,
    ) -> SeamResult<()> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        let chat_address = self
            .chat_address
            .clone()
            .unwrap_or_else(|| rua_core::ChatAddress::agent(&self.agent_id, session_id));
        let previous = store.load_active_summary_checkpoint(session_id).await?;
        let tail_start_sequence = previous
            .as_ref()
            .map(|checkpoint| checkpoint.frontier.end_sequence.saturating_add(1))
            .unwrap_or(1);
        let mut message_groups = store
            .projection_message_group_estimates_effective_for_address(
                &chat_address,
                self.running_config
                    .context_compact
                    .history_replacement_enabled,
                self.running_config
                    .context_compact
                    .token_count_estimate_divisor,
            )
            .await?
            .into_iter()
            .filter(|group| group.last_sequence >= tail_start_sequence)
            .collect::<Vec<_>>();
        let projection = store
            .build_effective_context_projection_for_address(
                &chat_address,
                rua_session::ContextProjectionRequest {
                    current_user_input: None,
                },
                self.running_config
                    .context_compact
                    .history_replacement_enabled,
                self.running_config
                    .context_compact
                    .token_count_estimate_divisor,
            )
            .await?;
        crate::projection_preparation::finalize_projection_message_group_estimates(
            &projection,
            &mut message_groups,
            &self.running_config,
        );
        let frontier = rua_session::ProjectionPlanner::widest_fold_frontier(&message_groups)
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: "manual compaction found no complete message boundary".to_string(),
            })?;
        rua_session::early_background_compaction_coordinator()
            .preempt_for_foreground(&format!("{}::{session_id}", self.workspace_dir.display()));
        let publisher = crate::summary_checkpoint::SummaryCheckpointPublisher::new(
            crate::summary_checkpoint::SummaryCheckpointPublisherInput {
                store,
                chat_id: session_id.to_string(),
                chat_address: chat_address.clone(),
                agent_id: self.agent_id.clone(),
                context_manager: None,
                running_config: Arc::clone(&self.running_config),
                model: self
                    .model
                    .clone()
                    .ok_or_else(|| rua_core::RuaError::Runtime {
                        message: "active Chat model is required for manual compaction".to_string(),
                    })?,
                workspace_dir: self.workspace_dir.clone(),
                replication_hook: self.replication_hook.clone(),
                model_call_identity: Some(
                    crate::model_call_context::ModelCallIdentityContext::from_chat_address(
                        &chat_address,
                    ),
                ),
            },
        );
        publisher
            .publish_precomputed(&frontier, summary, || true)
            .await?;
        Ok(())
    }
}

fn summary_chat_message(summary: String) -> ModelDialogueItem {
    ModelDialogueItem::text(
        Role::System,
        format!("# Compact Summary of Previous Conversation\n\n{summary}"),
    )
}

pub struct ContextCompactorAdapter {
    context_manager: Arc<dyn ContextManager>,
    agent_id: String,
    session_id: String,
    model: rua_session::CheckpointModelIdentity,
}

impl ContextCompactorAdapter {
    pub fn new(
        context_manager: Arc<dyn ContextManager>,
        agent_id: String,
        session_id: String,
        model: rua_session::CheckpointModelIdentity,
    ) -> Self {
        Self {
            context_manager,
            agent_id,
            session_id,
            model,
        }
    }
}

#[async_trait]
impl ContextCompactorSeam for ContextCompactorAdapter {
    async fn compact_context(
        &self,
        messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        extra_instruction: &str,
    ) -> SeamResult<Option<String>> {
        let mut messages = messages.to_vec();
        if !extra_instruction.is_empty() {
            messages.push(ModelDialogueItem::text(
                agent_types::Role::User,
                format!("Additional instruction for context summarization: {extra_instruction}"),
            ));
        }
        let req = ContextSummaryRequest {
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            messages,
            previous_summary: previous_summary.map(str::to_string),
            max_tokens: None,
            model: Some(self.model.clone()),
            model_call_context: None,
        };
        match self.context_manager.summarize_context(req).await {
            Ok(result) if result.summary.is_empty() => Ok(None),
            Ok(result) => Ok(Some(result.summary)),
            Err(e) => Err(Box::new(e) as _),
        }
    }
}

pub struct InMemoryPromptFileAdapter {
    files: Mutex<Vec<String>>,
}

impl Default for InMemoryPromptFileAdapter {
    fn default() -> Self {
        Self {
            files: Mutex::new(Vec::new()),
        }
    }
}

impl InMemoryPromptFileAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PromptFileSeam for InMemoryPromptFileAdapter {
    fn get_active_prompt_files(&self) -> Vec<String> {
        self.files.lock().unwrap().clone()
    }

    fn add_prompt_file(&self, file: &str) -> bool {
        let mut files = self.files.lock().unwrap();
        if files.iter().any(|f| f == file) {
            return false;
        }
        files.push(file.to_string());
        true
    }

    fn remove_prompt_file(&self, file: &str) -> bool {
        let mut files = self.files.lock().unwrap();
        let len_before = files.len();
        files.retain(|f| f != file);
        files.len() < len_before
    }

    fn reset_prompt_files(&self) {
        self.files.lock().unwrap().clear();
    }
}

pub struct PendingActionAdapter {
    pending: Arc<Mutex<Option<PendingAction>>>,
}

impl PendingActionAdapter {
    pub fn new(pending: Arc<Mutex<Option<PendingAction>>>) -> Self {
        Self { pending }
    }

    pub fn empty() -> Self {
        Self {
            pending: Arc::new(Mutex::new(None)),
        }
    }
}

impl PendingActionSeam for PendingActionAdapter {
    fn resolve_pending_action(&self, _approved: bool) -> Option<PendingActionInfo> {
        let mut guard = self.pending.lock().unwrap();
        let pending = guard.take()?;
        Some(PendingActionInfo {
            action_type: pending.action_type,
            payload: pending.payload,
            prompt: pending.prompt,
        })
    }
}

pub struct ConversationContextParams {
    pub agent_id: String,
    pub session_id: String,
    pub chat_address: rua_core::ChatAddress,
    pub user_id: String,
    pub channel: String,
    pub locale: String,
    pub workspace_dir: PathBuf,
    pub context_manager: Option<Arc<dyn ContextManager>>,
    pub running_config: Option<Arc<rua_config::AgentsRunningConfig>>,
    pub model: Option<rua_session::CheckpointModelIdentity>,
    pub replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
}

pub fn build_conversation_context(params: ConversationContextParams) -> ConversationContext {
    let conversation_session: Arc<dyn ConversationSessionSeam> = Arc::new(
        WorkspaceConversationSessionSeam::new(params.workspace_dir.clone())
            .with_checkpoint_context(
                params.agent_id.clone(),
                params.chat_address.clone(),
                params.running_config.clone(),
                params.model.clone(),
                params.replication_hook,
            ),
    );

    let context_compact_enabled = params
        .running_config
        .as_ref()
        .map(|cfg| cfg.context_compact.context_compact_enabled)
        .unwrap_or(true);
    let compactor_seam: Option<Arc<dyn ContextCompactorSeam>> =
        params.context_manager.as_ref().and_then(|cm| {
            if !context_compact_enabled {
                return None;
            }
            let model = params.model.clone()?;
            Some(Arc::new(ContextCompactorAdapter::new(
                cm.clone(),
                params.agent_id.clone(),
                params.session_id.clone(),
                model,
            )) as Arc<dyn ContextCompactorSeam>)
        });

    let prompt_seam: Arc<dyn PromptFileSeam> = Arc::new(InMemoryPromptFileAdapter::new());

    let pending_seam: Arc<dyn PendingActionSeam> = Arc::new(PendingActionAdapter::empty());

    ConversationContext {
        agent_id: params.agent_id,
        session_id: params.session_id,
        user_id: params.user_id,
        channel: params.channel,
        workspace_dir: params.workspace_dir.to_string_lossy().into_owned(),
        session: Some(conversation_session),
        context_compactor: compactor_seam,
        prompt_files: Some(prompt_seam),
        pending_action: Some(pending_seam),
        locale: params.locale,
    }
}

pub fn build_conversation_registry() -> ConversationCommandRegistry {
    let mut registry = ConversationCommandRegistry::new();

    let commands: Vec<Arc<dyn ConversationCommand>> = vec![
        Arc::new(CompactCommand),
        Arc::new(NewCommand),
        Arc::new(ClearCommand),
    ];

    for cmd in commands {
        registry.register(cmd);
    }

    registry
}
