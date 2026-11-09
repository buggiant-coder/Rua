use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rua_agent::context_manager::{ContextManager, HistoryReplacementGenerationRequest};
use rua_config::AgentsRunningConfig;
use rua_core::ChatAddress;
use rua_session::{
    CheckpointModelIdentity, HistoryReplacementCandidateDecision, HistoryReplacementReadyDraft,
    HistoryReplacementRejectedDraft, WorkspaceSessionStore,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(Clone)]
pub(crate) struct HistoryReplacementJobContext {
    pub workspace_dir: std::path::PathBuf,
    pub chat_address: ChatAddress,
    pub agent_id: String,
    pub authority_device_id: String,
    pub context_manager: Arc<dyn ContextManager>,
    pub running_config: Arc<AgentsRunningConfig>,
    pub model: CheckpointModelIdentity,
    pub session_sync: Option<crate::services::console_chat::SessionSyncEmitter>,
}

impl HistoryReplacementJobContext {
    fn key(&self) -> ChatAddress {
        self.chat_address.clone()
    }

    fn chat_id(&self) -> &str {
        self.chat_address.chat_id().as_str()
    }
}

struct ChatCoordinatorState {
    input: Arc<HistoryReplacementJobContext>,
    generation: u64,
    enabled: bool,
    rescan_requested: bool,
    current_cancel: Option<CancellationToken>,
    notify: Arc<Notify>,
}

#[derive(Default)]
struct CoordinatorState {
    chats: HashMap<ChatAddress, ChatCoordinatorState>,
    foreground_by_model: HashMap<String, usize>,
}

#[derive(Clone, Default)]
pub(crate) struct HistoryReplacementCoordinator {
    state: Arc<Mutex<CoordinatorState>>,
}

pub struct HistoryReplacementForegroundGuard {
    coordinator: HistoryReplacementCoordinator,
    model_key: String,
}

impl Drop for HistoryReplacementForegroundGuard {
    fn drop(&mut self) {
        self.coordinator.foreground_finished(&self.model_key);
    }
}

enum NextIteration {
    Run {
        input: Arc<HistoryReplacementJobContext>,
        generation: u64,
        cancel: CancellationToken,
    },
    Wait(Arc<Notify>),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IterationOutcome {
    Processed,
    NoCandidate,
    RetryableFailure,
    Cancelled,
}

impl HistoryReplacementCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&self, input: HistoryReplacementJobContext) {
        if input.authority_device_id.trim().is_empty() {
            return;
        }
        let key = input.key();
        if !input
            .running_config
            .context_compact
            .history_replacement_enabled
        {
            self.disable_chat(&key);
            return;
        }

        let mut should_spawn = false;
        let notify = {
            let mut state = self.state.lock().expect("history replacement state");
            match state.chats.get_mut(&key) {
                Some(chat) => {
                    let model_changed = chat.input.model != input.model;
                    chat.input = Arc::new(input);
                    chat.enabled = true;
                    chat.rescan_requested = true;
                    if model_changed {
                        if let Some(cancel) = chat.current_cancel.as_ref() {
                            cancel.cancel();
                        }
                    }
                    Arc::clone(&chat.notify)
                }
                None => {
                    let notify = Arc::new(Notify::new());
                    state.chats.insert(
                        key.clone(),
                        ChatCoordinatorState {
                            input: Arc::new(input),
                            generation: 0,
                            enabled: true,
                            rescan_requested: true,
                            current_cancel: None,
                            notify: Arc::clone(&notify),
                        },
                    );
                    should_spawn = true;
                    notify
                }
            }
        };
        notify.notify_waiters();
        if should_spawn {
            let coordinator = self.clone();
            tokio::spawn(async move {
                coordinator.run_worker(key).await;
            });
        }
    }

    pub fn disable_agent(&self, agent_id: &str) {
        let mut state = self.state.lock().expect("history replacement state");
        for chat in state
            .chats
            .values_mut()
            .filter(|chat| chat.input.agent_id == agent_id)
        {
            chat.enabled = false;
            chat.rescan_requested = false;
            if let Some(cancel) = chat.current_cancel.as_ref() {
                cancel.cancel();
            }
            chat.notify.notify_waiters();
        }
    }

    pub fn foreground(&self, model: &CheckpointModelIdentity) -> HistoryReplacementForegroundGuard {
        let foreground_key = model_key(model);
        {
            let mut state = self.state.lock().expect("history replacement state");
            *state
                .foreground_by_model
                .entry(foreground_key.clone())
                .or_default() += 1;
            for chat in state
                .chats
                .values_mut()
                .filter(|chat| model_key(&chat.input.model) == foreground_key)
            {
                if let Some(cancel) = chat.current_cancel.as_ref() {
                    cancel.cancel();
                }
                chat.notify.notify_waiters();
            }
        }
        HistoryReplacementForegroundGuard {
            coordinator: self.clone(),
            model_key: foreground_key,
        }
    }

    fn foreground_finished(&self, foreground_key: &str) {
        let mut state = self.state.lock().expect("history replacement state");
        if let Some(count) = state.foreground_by_model.get_mut(foreground_key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.foreground_by_model.remove(foreground_key);
            }
        }
        for chat in state
            .chats
            .values()
            .filter(|chat| model_key(&chat.input.model) == foreground_key)
        {
            chat.notify.notify_waiters();
        }
    }

    fn disable_chat(&self, key: &ChatAddress) {
        let mut state = self.state.lock().expect("history replacement state");
        let Some(chat) = state.chats.get_mut(key) else {
            return;
        };
        chat.enabled = false;
        chat.rescan_requested = false;
        if let Some(cancel) = chat.current_cancel.as_ref() {
            cancel.cancel();
        }
        chat.notify.notify_waiters();
    }

    async fn run_worker(&self, key: ChatAddress) {
        loop {
            match self.next_iteration(&key) {
                NextIteration::Run {
                    input,
                    generation,
                    cancel,
                } => {
                    let outcome =
                        run_one_candidate(self, &key, generation, input, cancel.clone()).await;
                    if !self.finish_iteration(&key, generation, outcome) {
                        break;
                    }
                }
                NextIteration::Wait(notify) => notify.notified().await,
                NextIteration::Stop => break,
            }
        }
    }

    fn next_iteration(&self, key: &ChatAddress) -> NextIteration {
        let mut state = self.state.lock().expect("history replacement state");
        let foreground_active = {
            let Some(chat) = state.chats.get(key) else {
                return NextIteration::Stop;
            };
            state
                .foreground_by_model
                .get(&model_key(&chat.input.model))
                .copied()
                .unwrap_or_default()
                > 0
        };
        let Some(chat) = state.chats.get_mut(key) else {
            return NextIteration::Stop;
        };
        if !chat.enabled {
            state.chats.remove(key);
            return NextIteration::Stop;
        }
        if foreground_active {
            return NextIteration::Wait(Arc::clone(&chat.notify));
        }
        chat.generation = chat.generation.saturating_add(1);
        chat.rescan_requested = false;
        let cancel = CancellationToken::new();
        chat.current_cancel = Some(cancel.clone());
        NextIteration::Run {
            input: Arc::clone(&chat.input),
            generation: chat.generation,
            cancel,
        }
    }

    fn finish_iteration(
        &self,
        key: &ChatAddress,
        generation: u64,
        outcome: IterationOutcome,
    ) -> bool {
        let mut state = self.state.lock().expect("history replacement state");
        let Some(chat) = state.chats.get_mut(key) else {
            return false;
        };
        if chat.generation != generation {
            return true;
        }
        chat.current_cancel = None;
        if !chat.enabled {
            state.chats.remove(key);
            return false;
        }
        match outcome {
            IterationOutcome::Processed | IterationOutcome::Cancelled => true,
            IterationOutcome::NoCandidate if chat.rescan_requested => true,
            IterationOutcome::NoCandidate | IterationOutcome::RetryableFailure => {
                state.chats.remove(key);
                false
            }
        }
    }

    fn can_publish(&self, key: &ChatAddress, generation: u64, cancel: &CancellationToken) -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        let state = self.state.lock().expect("history replacement state");
        state.chats.get(key).is_some_and(|chat| {
            chat.enabled && chat.generation == generation && chat.running_config_enabled()
        })
    }
}

impl ChatCoordinatorState {
    fn running_config_enabled(&self) -> bool {
        self.input
            .running_config
            .context_compact
            .history_replacement_enabled
    }
}

async fn run_one_candidate(
    coordinator: &HistoryReplacementCoordinator,
    key: &ChatAddress,
    generation: u64,
    input: Arc<HistoryReplacementJobContext>,
    cancel: CancellationToken,
) -> IterationOutcome {
    let store = WorkspaceSessionStore::new(&input.workspace_dir);
    let divisor = input
        .running_config
        .context_compact
        .token_count_estimate_divisor;
    let mut blocks = match store
        .detect_history_replacement_blocks(input.chat_id(), divisor)
        .await
    {
        Ok(blocks) => blocks,
        Err(error) => {
            warn!(
                %error,
                chat_id = %input.chat_id(),
                "history replacement block detection failed"
            );
            return IterationOutcome::RetryableFailure;
        }
    };
    let covered_end_sequence = store
        .load_active_summary_checkpoint(input.chat_id())
        .await
        .ok()
        .flatten()
        .map(|checkpoint| checkpoint.frontier.end_sequence)
        .unwrap_or_default();
    blocks.retain(|block| block.source.sequence_range.start_sequence > covered_end_sequence);
    blocks.sort_by_key(|block| std::cmp::Reverse(block.source.token_estimate));

    let mut selected = None;
    for block in blocks {
        match store
            .load_history_replacement_for_address(&input.chat_address, &block.record_id)
            .await
        {
            Ok(None) => {
                selected = Some(block);
                break;
            }
            Ok(Some(record)) if record.allows_generation_for_model(&input.model) => {
                selected = Some(block);
                break;
            }
            Ok(Some(_)) => {}
            Err(error) => {
                warn!(
                    %error,
                    chat_id = %input.chat_id(),
                    record_id = %block.record_id,
                    "history replacement record lookup failed"
                );
                return IterationOutcome::RetryableFailure;
            }
        }
    }
    let Some(block) = selected else {
        return IterationOutcome::NoCandidate;
    };
    let usage_id = format!("history-replacement-{}", uuid::Uuid::new_v4());

    let generation_request =
        input
            .context_manager
            .generate_history_replacement(HistoryReplacementGenerationRequest {
                session_id: input.chat_id().to_string(),
                agent_id: input.agent_id.clone(),
                turn_id: block.source.turn_id.clone(),
                source_block_id: block.record_id.clone(),
                usage_id: usage_id.clone(),
                source_content: block.source_content.clone(),
                max_tokens: 4096,
                model: input.model.clone(),
                model_call_context: Some(
                    rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                        &input.chat_address,
                    )
                    .model_call(),
                ),
            });
    let generated = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                "cancelled",
                None,
                "generation cancelled before provider response",
            );
            return IterationOutcome::Cancelled;
        }
        generated = generation_request => generated,
    };
    let generated = match generated {
        Ok(generated) => generated,
        Err(error) => {
            if cancel.is_cancelled() || error.is_cancelled() {
                record_model_call_outcome(
                    &input,
                    &block.record_id,
                    &usage_id,
                    "cancelled",
                    None,
                    &error.to_string(),
                );
                return IterationOutcome::Cancelled;
            }
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                "transient_failure",
                None,
                &error.to_string(),
            );
            warn!(
                %error,
                chat_id = %input.chat_id(),
                record_id = %block.record_id,
                "history replacement generation failed"
            );
            return IterationOutcome::RetryableFailure;
        }
    };
    if !coordinator.can_publish(key, generation, &cancel) {
        record_model_call_outcome(
            &input,
            &block.record_id,
            &usage_id,
            "cancelled",
            generated.usage.as_ref(),
            "generation completed after its publication lease was cancelled",
        );
        return IterationOutcome::Cancelled;
    }
    let usage = generated.usage.clone();

    let replacement_text = format!(
        "{}\n\n{}",
        rua_session::history_replacement_locator(&block.source),
        generated.digest.trim()
    );
    let decision = rua_session::evaluate_history_replacement_candidate(
        block.source.token_estimate,
        &generated.digest,
        &replacement_text,
        &generated.finish_reason,
        divisor,
    );
    let now = chrono::Utc::now().to_rfc3339();
    let (result, terminal_outcome, terminal_detail) = match decision {
        HistoryReplacementCandidateDecision::Ready {
            replacement_token_estimate,
            ..
        } => (
            store
                .publish_ready_history_replacement_if(
                    &input.chat_address,
                    HistoryReplacementReadyDraft {
                        record_id: block.record_id.clone(),
                        source: block.source,
                        digest: generated.digest,
                        replacement_token_estimate,
                        model: input.model.clone(),
                        generated_at: now,
                    },
                    || coordinator.can_publish(key, generation, &cancel),
                )
                .await,
            "ready",
            "ready record published".to_string(),
        ),
        HistoryReplacementCandidateDecision::Rejected {
            reason,
            replacement_token_estimate,
            saved_token_estimate,
        } => {
            let detail = format!("terminal quality gate: {reason:?}");
            (
                store
                    .publish_rejected_history_replacement_if(
                        &input.chat_address,
                        HistoryReplacementRejectedDraft {
                            record_id: block.record_id.clone(),
                            source: block.source,
                            model: input.model.clone(),
                            reason,
                            replacement_token_estimate,
                            saved_token_estimate,
                            rejected_at: now,
                        },
                        || coordinator.can_publish(key, generation, &cancel),
                    )
                    .await,
                "rejected",
                detail,
            )
        }
        HistoryReplacementCandidateDecision::Retryable { reason } => {
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                "transient_failure",
                usage.as_ref(),
                &reason,
            );
            warn!(
                chat_id = %input.chat_id(),
                record_id = %block.record_id,
                %reason,
                "history replacement generation remains retryable"
            );
            return IterationOutcome::RetryableFailure;
        }
    };
    match result {
        Ok(Some(_)) => {
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                terminal_outcome,
                usage.as_ref(),
                &terminal_detail,
            );
            if let Some(session_sync) = input.session_sync.as_ref() {
                session_sync
                    .emit_evergreen_chat_snapshot(&input.workspace_dir, &input.chat_address)
                    .await;
            }
            IterationOutcome::Processed
        }
        Ok(None) => {
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                "cancelled",
                usage.as_ref(),
                "record publication was cancelled",
            );
            IterationOutcome::Cancelled
        }
        Err(error) => {
            record_model_call_outcome(
                &input,
                &block.record_id,
                &usage_id,
                "transient_failure",
                usage.as_ref(),
                &error.to_string(),
            );
            warn!(
                %error,
                chat_id = %input.chat_id(),
                "history replacement record publication failed"
            );
            IterationOutcome::RetryableFailure
        }
    }
}

fn record_model_call_outcome(
    input: &HistoryReplacementJobContext,
    source_block_id: &str,
    usage_id: &str,
    outcome: &str,
    usage: Option<&agent_types::TokenUsage>,
    detail: &str,
) {
    tracing::info!(
        target: "diag.history_replacement",
        event = "history_replacement_model_call",
        source = "history_replacement",
        outcome,
        usage_id,
        agent_id = %input.agent_id,
        chat_id = %input.chat_id(),
        provider_id = %input.model.provider,
        model = %input.model.model,
        source_block_id,
        usage_reported = usage.is_some(),
        prompt_tokens = usage.map(|usage| usage.prompt_tokens).unwrap_or_default(),
        completion_tokens = usage
            .map(|usage| usage.completion_tokens)
            .unwrap_or_default(),
        reasoning_tokens = usage
            .and_then(|usage| usage.reasoning_tokens)
            .unwrap_or_default(),
        cached_tokens = usage
            .and_then(|usage| usage.cached_tokens)
            .unwrap_or_default(),
        total_tokens = usage.map(agent_types::TokenUsage::total).unwrap_or_default(),
        detail,
        "History Replacement model call completed",
    );
}

fn model_key(model: &CheckpointModelIdentity) -> String {
    format!("{}\0{}", model.provider, model.model)
}

#[doc(hidden)]
pub async fn reconcile_agent_config(state: &crate::AppState, agent_id: &str) {
    let local_device_id = state
        .sync_device
        .read()
        .ok()
        .and_then(|device| {
            device
                .as_ref()
                .map(|device| hex::encode(device.device_id_short()))
        })
        .unwrap_or_default();
    if local_device_id.is_empty() {
        return;
    }
    let runtime = match state.registry.get_or_create(agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(
                %error,
                agent_id,
                "history replacement backfill could not load workspace runtime"
            );
            return;
        }
    };
    let Some(context_manager) = runtime.context_manager().await else {
        return;
    };
    let workspace_dir = runtime.workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let headers = match store.list_chat_headers(0, usize::MAX).await {
        Ok(headers) => headers,
        Err(error) => {
            warn!(
                %error,
                agent_id,
                "history replacement backfill could not list chats"
            );
            return;
        }
    };
    for header in headers
        .into_iter()
        .filter(|header| header.agent_id == agent_id)
    {
        let address = ChatAddress::agent(agent_id, &header.chat_id);
        let configuration = match state.product_hooks.chat_configuration(&address).await {
            Some(configuration) => configuration,
            None => {
                warn!(chat_id = %header.chat_id, "could not load Chat configuration for history replacement");
                continue;
            }
        };
        let chat_running = match configuration.running_config.resolve() {
            Ok(effective) => effective,
            Err(error) => {
                warn!(%error, chat_id = %header.chat_id, "invalid Chat configuration for history replacement");
                continue;
            }
        };
        if !chat_running.context_compact.history_replacement_enabled {
            continue;
        }
        let meta = serde_json::Value::Object(header.extra.clone());
        let Ok(placement) =
            crate::services::conversation_authority_placement::parse_conversation_authority_placement(
                &meta,
            )
        else {
            continue;
        };
        if placement.device_id != local_device_id {
            continue;
        }
        let Some(model) = checkpoint_model_from_header(&header) else {
            continue;
        };
        state
            .conversation_authority
            .schedule_history_replacement(HistoryReplacementJobContext {
                workspace_dir: workspace_dir.clone(),
                chat_address: ChatAddress::agent(agent_id, &header.chat_id),
                agent_id: agent_id.to_string(),
                authority_device_id: placement.device_id,
                context_manager: Arc::clone(&context_manager),
                running_config: Arc::new(chat_running),
                model,
                session_sync: Some(
                    crate::services::console_chat::SessionSyncEmitter::from_app_state(state),
                ),
            });
    }
}

pub(crate) fn checkpoint_model_from_header(
    header: &rua_session::ChatSessionHeader,
) -> Option<CheckpointModelIdentity> {
    let slot = rua_chat::model_slot_from_header(header)?;
    let model = slot.as_object()?;
    let provider = model
        .get("provider_id")
        .and_then(serde_json::Value::as_str)?
        .trim();
    let model_id = model
        .get("model")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(CheckpointModelIdentity {
        provider: provider.to_string(),
        model: model_id.to_string(),
    })
}
