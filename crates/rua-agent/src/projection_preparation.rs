use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use agent_types::{AgentEvent, ChatRequest, ModelDialogueItem, Role};
use async_trait::async_trait;
use react_agent::{
    estimate_model_visible_conversation_tokens, estimate_model_visible_input_tokens_with_messages,
    AgentError, ModelCallProjector, ModelInputTokenEstimate, ModelInputTokenEstimateMethod,
    MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};
use rua_config::AgentsRunningConfig;
use rua_contract::agent::AgentRequest;
use rua_session::{
    context_target_band_from_running, early_background_compaction_coordinator,
    ActiveWorkPlanSnapshot, CheckpointModelIdentity, ContextProjection, ContextProjectionRequest,
    ProjectionEmergencyTail, ProjectionEntryAction, ProjectionEntryMode, ProjectionFitStatus,
    ProjectionMessageGroupEstimate, ProjectionPlanner, ProjectionPlanningInput,
    WorkspaceSessionStore,
};

use crate::context_manager::ContextManager;
pub use crate::projection_tool_results::{
    project_message_log_entries_for_model, project_tool_results_for_model,
};
use crate::reactive_compaction::ContextOverflowDetector;
use crate::summary_checkpoint::{
    prepare_summary_projection_for_budget, SummaryCheckpointPublisher,
    SummaryCheckpointPublisherInput,
};

const LLM_DEBUG_PROJECTION_EVIDENCE_KEY: &str = "rua_llm_debug_projection_evidence";
const ACTIVE_WORK_PLAN_METADATA_KEY: &str = "rua_active_work_plan";

pub use crate::projection_tool_results::ToolResultProjectionHook;

pub fn finalize_projection_message_group_estimates(
    projection: &ContextProjection,
    groups: &mut [ProjectionMessageGroupEstimate],
    running: &AgentsRunningConfig,
) {
    let effective_entries = projection
        .retained_messages
        .iter()
        .chain(projection.tail_messages.iter())
        .cloned()
        .collect::<Vec<_>>();
    let messages = project_message_log_entries_for_model(&effective_entries);
    let projected = project_tool_results_for_model(&messages, running);
    let divisor = running.context_compact.token_count_estimate_divisor;
    let tokens_by_message_id = projected
        .iter()
        .filter_map(|message| {
            message
                .metadata
                .get("rua_source_message_id")
                .and_then(serde_json::Value::as_str)
                .map(|message_id| {
                    (
                        message_id.to_string(),
                        estimate_serialized(message, divisor),
                    )
                })
        })
        .fold(
            HashMap::<String, u64>::new(),
            |mut totals, (message_id, tokens)| {
                *totals.entry(message_id).or_default() += tokens;
                totals
            },
        );
    for group in groups {
        group.token_estimate = effective_entries
            .iter()
            .filter(|message| {
                message.sequence >= group.first_sequence
                    && message.sequence <= group.last_sequence
                    && !group.retained_sequence_ranges.iter().any(|retained| {
                        message.sequence >= retained.start_sequence
                            && message.sequence <= retained.end_sequence
                    })
            })
            .filter_map(|message| tokens_by_message_id.get(&message.message_id))
            .copied()
            .sum::<u64>()
            .max(1);
    }
}

pub fn final_projection_token_estimate(
    projection: &ContextProjection,
    running: &AgentsRunningConfig,
) -> u64 {
    let mut messages = Vec::new();
    if let Some(summary) = projection.summary.as_deref() {
        messages.push(ModelDialogueItem::text(
            Role::System,
            format!("# Compact Summary of Previous Conversation\n\n{summary}"),
        ));
    }
    messages.extend(project_message_log_entries_for_model(
        &projection
            .retained_messages
            .iter()
            .chain(projection.tail_messages.iter())
            .cloned()
            .collect::<Vec<_>>(),
    ));
    let projected = project_tool_results_for_model(&messages, running);
    estimate_model_visible_conversation_tokens(
        &projected,
        running.context_compact.token_count_estimate_divisor,
    )
}

fn active_work_plan_visible_tokens_since(
    projection: &ContextProjection,
    sequence: u64,
    running: &AgentsRunningConfig,
) -> u64 {
    let entries = projection
        .retained_messages
        .iter()
        .chain(projection.tail_messages.iter())
        .filter(|message| message.sequence > sequence)
        .cloned()
        .collect::<Vec<_>>();
    let messages = project_message_log_entries_for_model(&entries);
    let projected = project_tool_results_for_model(&messages, running);
    estimate_model_visible_conversation_tokens(
        &projected,
        running.context_compact.token_count_estimate_divisor,
    )
}

pub struct ContextProjectionProjector {
    store: WorkspaceSessionStore,
    chat_id: String,
    chat_address: rua_core::ChatAddress,
    running_config: Arc<AgentsRunningConfig>,
    model: CheckpointModelIdentity,
    model_context_window: u64,
    early_background_compaction_key: String,
    publisher: SummaryCheckpointPublisher,
    reactive_compaction_pending: AtomicBool,
    last_input_estimate_tokens: AtomicU64,
    emergency_tail_start_sequence: AtomicU64,
}

impl ContextProjectionProjector {
    pub fn from_request(
        request: &AgentRequest,
        context_manager: Option<Arc<dyn ContextManager>>,
        running_config: Arc<AgentsRunningConfig>,
        model: CheckpointModelIdentity,
        model_context_window: Option<u64>,
        replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
    ) -> Option<Self> {
        request.metadata.get("turn_id")?.as_str()?;
        request
            .metadata
            .get(crate::turn_persistence::LOADED_MESSAGE_COUNT_METADATA_KEY)?
            .as_u64()?;
        let conversation_store_dir: PathBuf = request.conversation_store_dir()?.to_path_buf();
        let store = WorkspaceSessionStore::new(&conversation_store_dir);
        let chat_id = request.session_id.to_string();
        let chat_address = request
            .metadata
            .get(crate::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_else(|| {
                rua_core::ChatAddress::agent(request.agent_id.to_string(), chat_id.clone())
            });
        let early_background_compaction_key =
            format!("{}::{chat_id}", conversation_store_dir.display());
        Some(Self {
            publisher: SummaryCheckpointPublisher::new(SummaryCheckpointPublisherInput {
                store: WorkspaceSessionStore::new(&conversation_store_dir),
                chat_id: chat_id.clone(),
                chat_address: chat_address.clone(),
                agent_id: request.agent_id.to_string(),
                context_manager,
                running_config: Arc::clone(&running_config),
                model: model.clone(),
                workspace_dir: conversation_store_dir.clone(),
                replication_hook,
                model_call_identity:
                    crate::model_call_context::ModelCallIdentityContext::from_metadata(
                        &request.metadata,
                    ),
            }),
            store,
            chat_id,
            chat_address,
            model,
            model_context_window: model_context_window
                .unwrap_or(running_config.max_input_length.max(1) as u64)
                .min(running_config.max_input_length.max(1) as u64),
            early_background_compaction_key,
            running_config,
            reactive_compaction_pending: AtomicBool::new(false),
            last_input_estimate_tokens: AtomicU64::new(0),
            emergency_tail_start_sequence: AtomicU64::new(0),
        })
    }

    async fn prepare_request(
        &self,
        request: ChatRequest,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<ChatRequest, AgentError> {
        let mut status_started = false;
        let result = self
            .prepare_request_inner(request, event_tx, &mut status_started)
            .await;
        match result {
            Ok((mut request, published, estimate)) => {
                self.last_input_estimate_tokens
                    .store(estimate.fast_estimate_tokens, Ordering::Relaxed);
                // The planner already measured these exact final messages. Keep
                // that result instead of serializing the long history again.
                request.metadata.insert(
                    MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
                    serde_json::to_value(estimate).unwrap_or_default(),
                );
                if status_started {
                    emit_compaction_status(
                        event_tx,
                        if published { "completed" } else { "skipped" },
                        None,
                    )
                    .await;
                }
                Ok(request)
            }
            Err(error) => {
                if status_started {
                    emit_compaction_status(event_tx, "failed", Some(error.to_string())).await;
                }
                Err(error)
            }
        }
    }

    fn model_input_token_estimate(
        &self,
        request: &ChatRequest,
        messages: &[ModelDialogueItem],
    ) -> ModelInputTokenEstimate {
        calibrated_model_input_token_estimate(
            request,
            messages,
            &self.model,
            self.running_config
                .context_compact
                .token_count_estimate_divisor,
        )
    }

    fn attach_model_input_token_estimate(&self, request: &mut ChatRequest) {
        let estimate = self.model_input_token_estimate(request, &request.messages);
        request.metadata.insert(
            MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
            serde_json::to_value(estimate).unwrap_or_default(),
        );
    }

    async fn prepare_request_inner(
        &self,
        mut request: ChatRequest,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
        status_started: &mut bool,
    ) -> Result<(ChatRequest, bool, ModelInputTokenEstimate), AgentError> {
        let incoming_estimate = self.model_input_token_estimate(&request, &request.messages);
        if incoming_estimate.method == ModelInputTokenEstimateMethod::ProviderDelta {
            request.metadata.insert(
                MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
                serde_json::to_value(incoming_estimate).unwrap_or_default(),
            );
        }
        let emergency_start = self.emergency_tail_start_sequence.load(Ordering::Relaxed);
        let projection = if emergency_start > 0 {
            self.store
                .build_emergency_context_projection(
                    &self.chat_address,
                    emergency_start,
                    ContextProjectionRequest {
                        current_user_input: None,
                    },
                    self.running_config
                        .context_compact
                        .history_replacement_enabled,
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .await
        } else {
            self.store
                .build_effective_context_projection_for_address(
                    &self.chat_address,
                    ContextProjectionRequest {
                        current_user_input: None,
                    },
                    self.running_config
                        .context_compact
                        .history_replacement_enabled,
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .await
        }
        .map_err(internal_error)?;
        set_projection_evidence(&mut request, &projection);
        let mut active_work_plan = self.active_work_plan_message(&projection).await;
        request.messages =
            self.rebuild_messages(&request.messages, projection, active_work_plan.as_ref());
        self.attach_model_input_token_estimate(&mut request);
        let reactive_recovery = self.reactive_compaction_pending.load(Ordering::Relaxed);
        let input_limit = self.model_context_window;
        let target_band =
            context_target_band_from_running(&self.running_config, input_limit, false);
        if !self.running_config.context_compact.context_compact_enabled {
            let messages = project_tool_results_for_model(&request.messages, &self.running_config);
            let estimate = self.model_input_token_estimate(&request, &messages);
            let input_tokens = estimate.calibrated_estimate_tokens;
            if input_tokens > target_band.emergency_limit_tokens {
                return Err(AgentError::Internal(format!(
                    "final model input exceeds max_input_length: {input_tokens} > {}",
                    target_band.emergency_limit_tokens
                )));
            }
            request.messages = messages;
            return Ok((request, false, estimate));
        }

        let mut attempted_frontiers = HashSet::new();
        let mut published_any = false;
        let mut previous_emergency_tokens = None;

        'projection: loop {
            let candidate_messages =
                project_tool_results_for_model(&request.messages, &self.running_config);
            let current_projection_estimate =
                self.model_input_token_estimate(&request, &candidate_messages);
            let current_projection_tokens = current_projection_estimate.calibrated_estimate_tokens;
            let planner = ProjectionPlanner::new(target_band);
            let initial_plan = planner.plan(ProjectionPlanningInput {
                entry_mode: ProjectionEntryMode::PreModelCall,
                current_projection_tokens,
                checkpoint_tokens: 0,
                system_context_tokens: 0,
                pending_user_input_tokens: 0,
                message_groups: Vec::new(),
            });
            let recovered = !reactive_recovery
                || current_projection_estimate.fast_estimate_tokens
                    < self.last_input_estimate_tokens.load(Ordering::Relaxed);
            if recovered
                && (initial_plan.status == ProjectionFitStatus::FitsTarget
                    || (reactive_recovery
                        && current_projection_tokens <= target_band.emergency_limit_tokens))
            {
                self.reactive_compaction_pending
                    .store(false, Ordering::Relaxed);
                request.messages = candidate_messages;
                return Ok((request, published_any, current_projection_estimate));
            }
            if let Some(previous_tokens) = previous_emergency_tokens {
                if current_projection_tokens >= previous_tokens
                    && (!recovered
                        || current_projection_tokens > target_band.emergency_limit_tokens)
                {
                    return Err(AgentError::Internal(format!(
                        "context recovery made no progress; input estimate: {current_projection_tokens}, configured input limit: {}",
                        target_band.emergency_limit_tokens
                    )));
                }
            }
            if !*status_started {
                early_background_compaction_coordinator()
                    .preempt_for_foreground(&self.early_background_compaction_key);
                emit_compaction_status(event_tx, "started", None).await;
                *status_started = true;
            }

            let previous = self
                .store
                .load_active_summary_checkpoint(&self.chat_id)
                .await
                .map_err(internal_error)?;
            let previous_checkpoint_id = previous
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone());
            let tail_start_sequence = previous
                .as_ref()
                .map(|checkpoint| checkpoint.frontier.end_sequence.saturating_add(1))
                .unwrap_or(1);
            let mut message_groups = self
                .store
                .projection_message_group_estimates_effective_for_address(
                    &self.chat_address,
                    self.running_config
                        .context_compact
                        .history_replacement_enabled,
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .await
                .map_err(internal_error)?
                .into_iter()
                .filter(|group| group.last_sequence >= tail_start_sequence)
                .collect::<Vec<_>>();
            let durable_projection = self
                .store
                .build_effective_context_projection_for_address(
                    &self.chat_address,
                    ContextProjectionRequest {
                        current_user_input: None,
                    },
                    self.running_config
                        .context_compact
                        .history_replacement_enabled,
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .await
                .map_err(internal_error)?;
            finalize_projection_message_group_estimates(
                &durable_projection,
                &mut message_groups,
                &self.running_config,
            );
            let plan = planner.plan(ProjectionPlanningInput {
                entry_mode: ProjectionEntryMode::PreModelCall,
                current_projection_tokens,
                checkpoint_tokens: previous
                    .as_ref()
                    .map(|checkpoint| checkpoint.token_estimate)
                    .unwrap_or_default(),
                system_context_tokens: estimate_system_and_tool_tokens(
                    &request,
                    self.current_static_message_count(&request.messages),
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .max(
                    current_projection_tokens.saturating_sub(
                        message_groups
                            .iter()
                            .map(|group| group.token_estimate)
                            .sum::<u64>(),
                    ),
                ),
                pending_user_input_tokens: 0,
                message_groups: message_groups.clone(),
            });
            match if reactive_recovery && !recovered {
                ProjectionEntryAction::CreateCheckpointSynchronously
            } else {
                plan.action
            } {
                ProjectionEntryAction::BuildProjection => {
                    request.messages = candidate_messages;
                    return Ok((request, published_any, current_projection_estimate));
                }
                ProjectionEntryAction::UseEmergencyBoundedProjection => {
                    let emergency = emergency_tail_or_latest(plan.emergency_tail, &message_groups);
                    previous_emergency_tokens = Some(current_projection_tokens);
                    request = self
                        .build_reduced_history_candidate(
                            request,
                            emergency,
                            active_work_plan.as_ref(),
                        )
                        .await?;
                    continue;
                }
                ProjectionEntryAction::CreateCheckpointSynchronously => {}
                other => {
                    return Err(AgentError::Internal(format!(
                        "Projection Planner returned invalid PreModelCall action {other:?}"
                    )));
                }
            }
            let frontier = if reactive_recovery && !recovered {
                ProjectionPlanner::widest_fold_frontier(&message_groups)
            } else {
                plan.fold_frontier
            };
            let Some(frontier) = frontier else {
                if recovered && current_projection_tokens <= target_band.emergency_limit_tokens {
                    request.messages = candidate_messages;
                    return Ok((request, published_any, current_projection_estimate));
                }
                let emergency = emergency_tail_or_latest(plan.emergency_tail, &message_groups);
                previous_emergency_tokens = Some(current_projection_tokens);
                request = self
                    .build_reduced_history_candidate(request, emergency, active_work_plan.as_ref())
                    .await?;
                continue;
            };
            let mut summary_input_limit_tokens = target_band
                .upper_target_tokens
                .saturating_sub(target_band.tail_reserve_tokens)
                .max(1);
            let requested_frontier = frontier;
            let mut summary_provider_overflow_retries = 0_usize;
            let (frontier, published) = loop {
                let summary_projection = prepare_summary_projection_for_budget(
                    &durable_projection,
                    &message_groups,
                    requested_frontier.clone(),
                    &self.running_config,
                    summary_input_limit_tokens,
                )
                .map_err(internal_error)?;
                let Some((frontier, summary_source)) = summary_projection else {
                    let emergency = emergency_tail_or_latest(plan.emergency_tail, &message_groups);
                    previous_emergency_tokens = Some(current_projection_tokens);
                    request = self
                        .build_reduced_history_candidate(
                            request,
                            emergency,
                            active_work_plan.as_ref(),
                        )
                        .await?;
                    continue 'projection;
                };

                let publication = self.publisher.publish(&frontier, summary_source, || true);
                let result = if reactive_recovery {
                    tokio::time::timeout(crate::compaction::DEFAULT_SUMMARY_TIMEOUT, publication)
                        .await
                        .unwrap_or_else(|_| {
                            Err(rua_core::RuaError::Runtime {
                                message: "context summary timed out during overflow recovery"
                                    .to_string(),
                            })
                        })
                } else {
                    publication.await
                };
                match result {
                    Ok(published) => break (frontier, published),
                    Err(error)
                        if ContextOverflowDetector::is_context_overflow_message(
                            &error.to_string(),
                        ) && summary_input_limit_tokens > 1 =>
                    {
                        summary_provider_overflow_retries += 1;
                        summary_input_limit_tokens =
                            summary_input_limit_tokens.saturating_div(2).max(1);
                        tracing::warn!(
                            frontier_end_sequence = frontier.end_sequence,
                            summary_input_limit_tokens,
                            summary_provider_overflow_retries,
                            %error,
                            "summary provider rejected input; retrying with a smaller summary source"
                        );
                    }
                    Err(error)
                        if ContextOverflowDetector::is_context_overflow_message(
                            &error.to_string(),
                        ) =>
                    {
                        tracing::warn!(
                            frontier_end_sequence = frontier.end_sequence,
                            summary_input_limit_tokens,
                            %error,
                            "summary provider still rejected reduced input; using emergency bounded projection"
                        );
                        let emergency =
                            emergency_tail_or_latest(plan.emergency_tail, &message_groups);
                        previous_emergency_tokens = Some(current_projection_tokens);
                        request = self
                            .build_reduced_history_candidate(
                                request,
                                emergency,
                                active_work_plan.as_ref(),
                            )
                            .await?;
                        continue 'projection;
                    }
                    Err(error) if reactive_recovery => {
                        tracing::warn!(
                            frontier_end_sequence = frontier.end_sequence,
                            %error,
                            "overflow checkpoint failed; using emergency bounded projection"
                        );
                        let emergency =
                            emergency_tail_or_latest(plan.emergency_tail, &message_groups);
                        previous_emergency_tokens = Some(current_projection_tokens);
                        request = self
                            .build_reduced_history_candidate(
                                request,
                                emergency,
                                active_work_plan.as_ref(),
                            )
                            .await?;
                        continue 'projection;
                    }
                    Err(error) => return Err(internal_error(error)),
                }
            };
            let attempt_key = (previous_checkpoint_id, frontier.end_sequence);
            if !attempted_frontiers.insert(attempt_key) {
                return Err(AgentError::Internal(
                    "blocking context compaction did not make projection progress".to_string(),
                ));
            }
            published_any |= published.is_some();

            let projection = self
                .store
                .build_effective_context_projection_for_address(
                    &self.chat_address,
                    ContextProjectionRequest {
                        current_user_input: None,
                    },
                    self.running_config
                        .context_compact
                        .history_replacement_enabled,
                    self.running_config
                        .context_compact
                        .token_count_estimate_divisor,
                )
                .await
                .map_err(internal_error)?;
            if let Some(checkpoint) = published.as_ref() {
                if projection.active_checkpoint_id.as_deref()
                    != Some(checkpoint.checkpoint_id.as_str())
                {
                    return Err(AgentError::Internal(
                        "published checkpoint is not active during projection rebuild".to_string(),
                    ));
                }
            }
            set_projection_evidence(&mut request, &projection);
            if active_work_plan.is_none() {
                active_work_plan = self.active_work_plan_message(&projection).await;
            }
            request.messages =
                self.rebuild_messages(&request.messages, projection, active_work_plan.as_ref());
        }
    }

    async fn build_reduced_history_candidate(
        &self,
        mut request: ChatRequest,
        emergency: ProjectionEmergencyTail,
        active_work_plan: Option<&ModelDialogueItem>,
    ) -> Result<ChatRequest, AgentError> {
        let projection = self
            .store
            .build_emergency_context_projection(
                &self.chat_address,
                emergency.start_sequence,
                ContextProjectionRequest {
                    current_user_input: None,
                },
                self.running_config
                    .context_compact
                    .history_replacement_enabled,
                self.running_config
                    .context_compact
                    .token_count_estimate_divisor,
            )
            .await
            .map_err(internal_error)?;
        self.emergency_tail_start_sequence
            .fetch_max(emergency.start_sequence, Ordering::Relaxed);
        set_projection_evidence(&mut request, &projection);
        let active_work_plan = self
            .active_work_plan_message(&projection)
            .await
            .or_else(|| active_work_plan.cloned());
        request.messages =
            self.rebuild_messages(&request.messages, projection, active_work_plan.as_ref());
        Ok(request)
    }

    fn rebuild_messages(
        &self,
        original: &[ModelDialogueItem],
        projection: ContextProjection,
        active_work_plan: Option<&ModelDialogueItem>,
    ) -> Vec<ModelDialogueItem> {
        let static_count = self.current_static_message_count(original);
        let storage_root = self
            .store
            .sessions_dir()
            .parent()
            .expect("session storage root");
        let mut rebuilt = original[..static_count].to_vec();
        if let Some(summary) = projection.summary {
            rebuilt.push(ModelDialogueItem::text(
                Role::System,
                format!("# Compact Summary of Previous Conversation\n\n{summary}"),
            ));
        }

        let mut projected = project_message_log_entries_for_model(
            &projection
                .retained_messages
                .iter()
                .chain(projection.tail_messages.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        // Durable projection owns membership, order, and portable media references.
        // Resolve every reference from the Chat store, including history absent
        // from the original request after truncation or checkpoint changes.
        crate::media::resolve_projected_media(&mut projected, storage_root);
        rebuilt.extend(projected);
        if let Some(active_work_plan) = active_work_plan {
            rebuilt.push(active_work_plan.clone());
        }
        rebuilt
    }

    async fn active_work_plan_message(
        &self,
        projection: &ContextProjection,
    ) -> Option<ModelDialogueItem> {
        if !self
            .running_config
            .context_compact
            .active_work_plan_injection_enabled
        {
            return None;
        }
        let snapshot = match self.store.load_active_work_plan(&self.chat_id).await {
            Ok(snapshot) => snapshot?,
            Err(error) => {
                tracing::warn!(
                    chat_id = %self.chat_id,
                    %error,
                    "failed to load active work plan; skipping projection injection"
                );
                return None;
            }
        };
        if !self.should_inject_active_work_plan(&snapshot, projection) {
            return None;
        }
        let mut content = String::from("<active_work_plan>\n");
        if let Some(explanation) = snapshot
            .explanation
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            content.push_str("Explanation: ");
            content.push_str(explanation);
            content.push('\n');
        }
        for step in &snapshot.plan {
            content.push_str("- [");
            content.push_str(step.status.as_str());
            content.push_str("] ");
            content.push_str(&step.step);
            content.push('\n');
        }
        content.push_str("</active_work_plan>");
        let mut message = ModelDialogueItem::text(Role::Assistant, content);
        message.metadata.insert(
            ACTIVE_WORK_PLAN_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );
        if let Err(error) = self
            .store
            .mark_active_work_plan_injected(
                &self.chat_id,
                &snapshot.checksum,
                projection.tail_end_sequence.or(snapshot.source_sequence),
                projection.active_checkpoint_id.as_deref(),
            )
            .await
        {
            tracing::warn!(
                chat_id = %self.chat_id,
                %error,
                "failed to persist active work plan injection anchor"
            );
        }
        Some(message)
    }

    fn should_inject_active_work_plan(
        &self,
        snapshot: &ActiveWorkPlanSnapshot,
        projection: &ContextProjection,
    ) -> bool {
        let anchor_sequence = snapshot.last_injected_sequence.or(snapshot.source_sequence);
        let Some(anchor_sequence) = anchor_sequence else {
            return true;
        };
        if projection
            .tail_start_sequence
            .is_some_and(|start| start > anchor_sequence)
            && !projection
                .retained_messages
                .iter()
                .any(|message| message.sequence == anchor_sequence)
        {
            return true;
        }
        if projection
            .tail_end_sequence
            .is_some_and(|tail_end| tail_end < anchor_sequence)
        {
            return true;
        }
        if projection.active_checkpoint_id.is_some()
            && snapshot.last_injected_checkpoint_id != projection.active_checkpoint_id
        {
            return true;
        }
        let ratio = self
            .running_config
            .context_compact
            .active_work_plan_injection_interval_ratio
            .clamp(0.01, 0.25);
        let interval_tokens = ((self.model_context_window as f64) * ratio).max(1.0) as u64;
        active_work_plan_visible_tokens_since(projection, anchor_sequence, &self.running_config)
            >= interval_tokens
    }

    fn current_static_message_count(&self, messages: &[ModelDialogueItem]) -> usize {
        messages
            .iter()
            .take_while(|message| {
                message
                    .metadata
                    .contains_key(react_agent::MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
            })
            .count()
    }
}

fn set_projection_evidence(request: &mut ChatRequest, projection: &ContextProjection) {
    let references = |messages: &[rua_session::MessageLogEntry]| {
        messages
            .iter()
            .map(|message| {
                serde_json::json!({
                    "message_id": message.message_id,
                    "sequence": message.sequence,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut aggregation = Vec::<serde_json::Value>::new();
    let mut aggregation_indices = HashMap::<String, usize>::new();
    for message in projection
        .retained_messages
        .iter()
        .chain(projection.tail_messages.iter())
    {
        let dialogue_item_id = message
            .metadata
            .get(agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&message.message_id)
            .to_string();
        let index = *aggregation_indices
            .entry(dialogue_item_id.clone())
            .or_insert_with(|| {
                aggregation.push(serde_json::json!({
                    "model_dialogue_item_id": dialogue_item_id,
                    "sources": [],
                }));
                aggregation.len() - 1
            });
        aggregation[index]["sources"]
            .as_array_mut()
            .expect("aggregation sources are an array")
            .push(serde_json::json!({
                "message_id": message.message_id,
                "sequence": message.sequence,
            }));
    }
    request.metadata.insert(
        LLM_DEBUG_PROJECTION_EVIDENCE_KEY.into(),
        serde_json::json!({
            "active_checkpoint_id": projection.active_checkpoint_id,
            "tail_start_sequence": projection.tail_start_sequence,
            "tail_end_sequence": projection.tail_end_sequence,
            "retained_messages": references(&projection.retained_messages),
            "tail_messages": references(&projection.tail_messages),
            "history_replacements": projection.history_replacement_evidence,
            "aggregation": aggregation,
        }),
    );
}

#[async_trait]
impl ModelCallProjector for ContextProjectionProjector {
    async fn project(
        &self,
        request: ChatRequest,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<ChatRequest, AgentError> {
        self.prepare_request(request, event_tx).await
    }

    async fn recover_context_overflow(
        &self,
        messages: &[ModelDialogueItem],
        error_message: &str,
    ) -> Result<Vec<ModelDialogueItem>, AgentError> {
        if !self.running_config.context_compact.context_compact_enabled {
            return Err(AgentError::ContextOverflow {
                message: error_message.to_string(),
            });
        }
        let rejected_tokens = self.last_input_estimate_tokens.load(Ordering::Relaxed);
        self.reactive_compaction_pending
            .store(true, Ordering::Relaxed);
        tracing::warn!(
            chat_id = %self.chat_id,
            rejected_tokens,
            input_limit = self.model_context_window,
            error = %error_message,
            "provider rejected context; rebuilding a smaller durable projection"
        );
        // The next model-call boundary creates a checkpoint or bounded tail,
        // then remeasures it. An in-memory rewrite would be overwritten by
        // the canonical history rebuild at that boundary.
        Ok(messages.to_vec())
    }
}

async fn emit_compaction_status(
    event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    status: &str,
    message: Option<String>,
) {
    let _ = event_tx
        .send(AgentEvent::ContextCompaction {
            status: status.to_string(),
            mode: "blocking".to_string(),
            message,
        })
        .await;
}

fn emergency_tail_or_latest(
    planned: Option<ProjectionEmergencyTail>,
    groups: &[ProjectionMessageGroupEstimate],
) -> ProjectionEmergencyTail {
    planned
        .or_else(|| {
            groups.last().map(|group| ProjectionEmergencyTail {
                start_turn_ordinal: group.turn_ordinal,
                start_sequence: group.first_sequence,
                estimated_tail_tokens: group.token_estimate,
            })
        })
        .unwrap_or(ProjectionEmergencyTail {
            start_turn_ordinal: 0,
            start_sequence: u64::MAX,
            estimated_tail_tokens: 0,
        })
}

pub fn calibrated_model_input_token_estimate(
    request: &ChatRequest,
    messages: &[ModelDialogueItem],
    model: &CheckpointModelIdentity,
    divisor: f64,
) -> ModelInputTokenEstimate {
    let fast_estimate_tokens =
        estimate_model_visible_input_tokens_with_messages(request, messages, divisor);
    let conversation_estimate_tokens =
        estimate_model_visible_conversation_tokens(messages, divisor);
    let latest_usage = messages.iter().rev().find_map(|message| {
        let estimate = message
            .metadata
            .get(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value::<ModelInputTokenEstimate>(value).ok())?;
        let prompt_tokens = message
            .metadata
            .get(crate::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY)
            .and_then(|usage| usage.get("last_call_usage"))
            .and_then(|usage| usage.get("prompt_tokens"))
            .and_then(serde_json::Value::as_u64)?;
        Some((estimate, prompt_tokens))
    });
    if let Some(previous) = request
        .metadata
        .get(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<ModelInputTokenEstimate>(value).ok())
        .filter(|_| latest_usage.is_none())
    {
        return ModelInputTokenEstimate {
            provider_id: Some(model.provider.clone()),
            model: Some(model.model.clone()),
            fast_estimate_divisor: divisor,
            fast_estimate_tokens,
            calibrated_estimate_tokens: apply_token_delta(
                previous.calibrated_estimate_tokens,
                fast_estimate_tokens,
                previous.fast_estimate_tokens,
            ),
            conversation_estimate_tokens,
            method: previous.method,
            calibration_source_provider_id: previous.calibration_source_provider_id,
            calibration_source_model: previous.calibration_source_model,
            previous_fast_estimate_tokens: previous.previous_fast_estimate_tokens,
            previous_provider_prompt_tokens: previous.previous_provider_prompt_tokens,
        };
    }
    let Some((previous, previous_provider_prompt_tokens)) = latest_usage else {
        return ModelInputTokenEstimate {
            provider_id: Some(model.provider.clone()),
            model: Some(model.model.clone()),
            fast_estimate_divisor: divisor,
            fast_estimate_tokens,
            calibrated_estimate_tokens: fast_estimate_tokens,
            conversation_estimate_tokens,
            method: ModelInputTokenEstimateMethod::Fast,
            calibration_source_provider_id: None,
            calibration_source_model: None,
            previous_fast_estimate_tokens: None,
            previous_provider_prompt_tokens: None,
        };
    };
    ModelInputTokenEstimate {
        provider_id: Some(model.provider.clone()),
        model: Some(model.model.clone()),
        fast_estimate_divisor: divisor,
        fast_estimate_tokens,
        calibrated_estimate_tokens: apply_token_delta(
            previous_provider_prompt_tokens,
            fast_estimate_tokens,
            previous.fast_estimate_tokens,
        ),
        conversation_estimate_tokens,
        method: ModelInputTokenEstimateMethod::ProviderDelta,
        calibration_source_provider_id: previous.provider_id,
        calibration_source_model: previous.model,
        previous_fast_estimate_tokens: Some(previous.fast_estimate_tokens),
        previous_provider_prompt_tokens: Some(previous_provider_prompt_tokens),
    }
}

fn estimate_system_and_tool_tokens(
    request: &ChatRequest,
    model_call_context_count: usize,
    divisor: f64,
) -> u64 {
    estimate_messages(
        &request.messages[..model_call_context_count.min(request.messages.len())],
        divisor,
    )
    .saturating_add(estimate_serialized(&request.tools, divisor))
    .saturating_add(estimate_serialized(&request.structured_output, divisor))
}

fn estimate_messages(messages: &[ModelDialogueItem], divisor: f64) -> u64 {
    #[derive(serde::Serialize)]
    struct ModelVisibleMessage<'a> {
        role: &'a agent_types::Role,
        content: &'a [agent_types::ContentPart],
        name: &'a Option<String>,
        tool_call_id: Option<&'a str>,
    }

    let visible_messages = messages
        .iter()
        .map(|message| ModelVisibleMessage {
            role: &message.role,
            content: &message.content,
            name: &message.name,
            tool_call_id: message
                .metadata
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str),
        })
        .collect::<Vec<_>>();
    estimate_serialized(&visible_messages, divisor)
}

fn estimate_serialized<T: serde::Serialize>(value: &T, divisor: f64) -> u64 {
    rua_core::token_estimate::estimate_serialized_tokens(value, divisor)
}

fn apply_token_delta(base: u64, current_fast: u64, previous_fast: u64) -> u64 {
    if current_fast >= previous_fast {
        base.saturating_add(current_fast - previous_fast)
    } else {
        base.saturating_sub(previous_fast - current_fast)
    }
}

fn internal_error(error: rua_core::RuaError) -> AgentError {
    AgentError::Internal(error.to_string())
}
