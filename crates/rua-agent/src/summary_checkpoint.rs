use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent_types::{ContentPart, ToolCall, ToolResult};
use rua_config::AgentsRunningConfig;
use rua_session::{
    CheckpointModelIdentity, CheckpointSequenceRange, CheckpointTurnRange, ContextProjection,
    ContextProjectionRequest, ProjectionFoldFrontier, SummaryCheckpoint, SummaryCheckpointDraft,
    WorkspaceSessionStore,
};

use crate::context_manager::{ContextManager, ContextSummaryRequest};

const SUMMARY_OUTPUT_MAX_TOKENS: usize = 4_096;

#[derive(Debug, Clone)]
pub struct SummaryProjectionSlice {
    pub previous_summary: Option<String>,
    pub messages: Vec<agent_types::ModelDialogueItem>,
}

pub fn fit_summary_projection_slice_to_budget(
    mut source: SummaryProjectionSlice,
    running_config: &AgentsRunningConfig,
    max_tokens: u64,
) -> rua_core::Result<Option<SummaryProjectionSlice>> {
    let max_tokens = max_tokens.max(1);
    retain_summary_thinking(&mut source.messages, running_config);
    let initial_tokens = summary_request_token_estimate(&source, running_config);
    if initial_tokens <= max_tokens {
        return Ok(Some(source));
    }

    let removed_thinking_parts = remove_thinking_parts(&mut source.messages);
    let after_thinking = summary_request_token_estimate(&source, running_config);
    let divisor = running_config
        .context_compact
        .token_count_estimate_divisor
        .max(1.0);
    let mut remaining_bytes_to_remove =
        (after_thinking.saturating_sub(max_tokens) as f64 * divisor).ceil() as usize;
    let mut compacted_tool_results = 0_usize;

    if remaining_bytes_to_remove > 0 {
        for replacement in summary_tool_round_replacements(&source.messages)? {
            if let Some((message_index, part_index, tool_use)) = replacement.tool_use {
                source.messages[message_index].content[part_index] = tool_use;
            }
            let (message_index, part_index, tool_result) = replacement.tool_result;
            source.messages[message_index].content[part_index] = tool_result;
            compacted_tool_results += 1;
            remaining_bytes_to_remove =
                remaining_bytes_to_remove.saturating_sub(replacement.saved_bytes);
            if remaining_bytes_to_remove == 0 {
                break;
            }
        }
    }

    let final_tokens = summary_request_token_estimate(&source, running_config);
    if final_tokens > max_tokens {
        return Ok(None);
    }

    tracing::info!(
        initial_tokens,
        final_tokens,
        max_tokens,
        removed_thinking_parts,
        compacted_tool_results,
        "fitted fixed-frontier context summary input"
    );
    Ok(Some(source))
}

pub fn prepare_summary_projection_for_budget(
    projection: &ContextProjection,
    message_groups: &[rua_session::ProjectionMessageGroupEstimate],
    requested: ProjectionFoldFrontier,
    running_config: &AgentsRunningConfig,
    max_tokens: u64,
) -> rua_core::Result<Option<(ProjectionFoldFrontier, SummaryProjectionSlice)>> {
    let requested_source =
        summary_projection_slice(projection, running_config, requested.end_sequence)?;
    if let Some(source) =
        fit_summary_projection_slice_to_budget(requested_source, running_config, max_tokens)?
    {
        return Ok(Some((requested, source)));
    }

    let eligible = message_groups
        .iter()
        .take_while(|group| group.last_sequence <= requested.end_sequence)
        .collect::<Vec<_>>();
    let Some(first) = eligible.first() else {
        return Ok(None);
    };
    for (index, group) in eligible.iter().enumerate().rev() {
        if group.last_sequence == requested.end_sequence {
            continue;
        }
        let Some(source) =
            summary_projection_slice_if_boundary(projection, running_config, group.last_sequence)?
        else {
            continue;
        };
        let Some(source) =
            fit_summary_projection_slice_to_budget(source, running_config, max_tokens)?
        else {
            continue;
        };
        let selected = &eligible[..=index];
        return Ok(Some((
            ProjectionFoldFrontier {
                turn_id: group.turn_id.clone(),
                start_turn_ordinal: first.turn_ordinal,
                end_turn_ordinal: group.turn_ordinal,
                start_sequence: first.first_sequence,
                end_sequence: group.last_sequence,
                estimated_tokens_to_fold: selected.iter().map(|group| group.token_estimate).sum(),
                retained_sequence_ranges: group.retained_sequence_ranges.clone(),
            },
            source,
        )));
    }
    Ok(None)
}

pub fn summary_request_token_estimate(
    source: &SummaryProjectionSlice,
    running_config: &AgentsRunningConfig,
) -> u64 {
    let request = crate::llm_summary::build_summary_request(
        &source.messages,
        source.previous_summary.as_deref(),
        Some(SUMMARY_OUTPUT_MAX_TOKENS),
        false,
    );
    // A summary changes the selected history and request instructions. The
    // ordinary conversation's provider delta is not a calibration for this input.
    react_agent::estimate_model_visible_input_tokens_with_messages(
        &request,
        &request.messages,
        running_config.context_compact.token_count_estimate_divisor,
    )
}

pub fn summary_projection_slice(
    projection: &ContextProjection,
    running_config: &AgentsRunningConfig,
    frontier_end_sequence: u64,
) -> rua_core::Result<SummaryProjectionSlice> {
    summary_projection_slice_if_boundary(projection, running_config, frontier_end_sequence)?
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: "context compression frontier does not align with a final projection message boundary"
                .to_string(),
        })
}

pub fn summary_projection_slice_if_boundary(
    projection: &ContextProjection,
    running_config: &AgentsRunningConfig,
    frontier_end_sequence: u64,
) -> rua_core::Result<Option<SummaryProjectionSlice>> {
    let all_entries = projection
        .retained_messages
        .iter()
        .chain(projection.tail_messages.iter())
        .cloned()
        .collect::<Vec<_>>();
    let prefix_entries = all_entries
        .iter()
        .filter(|entry| entry.sequence <= frontier_end_sequence)
        .cloned()
        .collect::<Vec<_>>();
    let identity_key =
        agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY;
    let boundary_splits_aggregated_message = prefix_entries
        .last()
        .zip(all_entries.get(prefix_entries.len()))
        .is_some_and(|(before, after)| {
            before.role == "assistant"
                && after.role == "assistant"
                && before
                    .metadata
                    .get(identity_key)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|identity| {
                        after
                            .metadata
                            .get(identity_key)
                            .and_then(serde_json::Value::as_str)
                            == Some(identity)
                    })
        });
    if boundary_splits_aggregated_message {
        return Ok(None);
    }
    let prefix_aggregated =
        crate::projection_tool_results::project_message_log_entries_for_model(&prefix_entries);
    let mut messages =
        crate::projection_tool_results::project_message_log_entries(&all_entries, running_config)
            .into_iter()
            .take(prefix_aggregated.len())
            .collect::<Vec<_>>();
    retain_summary_thinking(&mut messages, running_config);
    Ok(Some(SummaryProjectionSlice {
        previous_summary: projection.summary.clone(),
        messages,
    }))
}

pub async fn build_summary_projection_slice(
    store: &WorkspaceSessionStore,
    chat_address: &rua_core::ChatAddress,
    running_config: &AgentsRunningConfig,
    frontier_end_sequence: u64,
) -> rua_core::Result<SummaryProjectionSlice> {
    let projection = store
        .build_effective_context_projection_for_address(
            chat_address,
            ContextProjectionRequest {
                current_user_input: None,
            },
            running_config.context_compact.history_replacement_enabled,
            running_config.context_compact.token_count_estimate_divisor,
        )
        .await?;
    summary_projection_slice(&projection, running_config, frontier_end_sequence)
}

pub struct SummaryCheckpointPublisherInput {
    pub store: WorkspaceSessionStore,
    pub chat_id: String,
    pub chat_address: rua_core::ChatAddress,
    pub agent_id: String,
    pub context_manager: Option<Arc<dyn ContextManager>>,
    pub running_config: Arc<AgentsRunningConfig>,
    pub model: CheckpointModelIdentity,
    pub workspace_dir: PathBuf,
    pub replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
    pub model_call_identity: Option<crate::model_call_context::ModelCallIdentityContext>,
}

pub struct SummaryCheckpointPublisher {
    store: WorkspaceSessionStore,
    chat_id: String,
    chat_address: rua_core::ChatAddress,
    agent_id: String,
    context_manager: Option<Arc<dyn ContextManager>>,
    running_config: Arc<AgentsRunningConfig>,
    model: CheckpointModelIdentity,
    workspace_dir: PathBuf,
    replication_hook: Option<Arc<dyn rua_contract::agent::SessionReplicationHook>>,
    model_call_identity: Option<crate::model_call_context::ModelCallIdentityContext>,
}

struct CheckpointPublicationBase {
    previous: Option<SummaryCheckpoint>,
    previous_checkpoint_id: Option<String>,
    expected_active_checkpoint_id: Option<String>,
    source_start: u64,
    projection_state: rua_session::CheckpointProjectionState,
}

impl SummaryCheckpointPublisher {
    pub fn new(input: SummaryCheckpointPublisherInput) -> Self {
        Self {
            store: input.store,
            chat_id: input.chat_id,
            chat_address: input.chat_address,
            agent_id: input.agent_id,
            context_manager: input.context_manager,
            running_config: input.running_config,
            model: input.model,
            workspace_dir: input.workspace_dir,
            replication_hook: input.replication_hook,
            model_call_identity: input.model_call_identity,
        }
    }

    pub async fn publish<F>(
        &self,
        frontier: &ProjectionFoldFrontier,
        mut source: SummaryProjectionSlice,
        can_publish: F,
    ) -> rua_core::Result<Option<SummaryCheckpoint>>
    where
        F: Fn() -> bool,
    {
        if !self.running_config.context_compact.context_compact_enabled || !can_publish() {
            return Ok(None);
        }
        let Some(base) = self.publication_base(frontier).await? else {
            return Ok(None);
        };
        if source.messages.is_empty()
            && source.previous_summary.as_deref().is_none_or(str::is_empty)
        {
            return Ok(None);
        }
        crate::media::resolve_projected_media(&mut source.messages, &self.workspace_dir);
        let context_manager =
            self.context_manager
                .as_ref()
                .ok_or_else(|| rua_core::RuaError::Runtime {
                    message: "context manager is required to generate a checkpoint summary"
                        .to_string(),
                })?;
        let summary = context_manager
            .summarize_context(ContextSummaryRequest {
                session_id: self.chat_id.clone(),
                agent_id: self.agent_id.clone(),
                messages: source.messages,
                previous_summary: source.previous_summary,
                max_tokens: Some(SUMMARY_OUTPUT_MAX_TOKENS),
                model: Some(self.model.clone()),
                model_call_context: self
                    .model_call_identity
                    .as_ref()
                    .map(crate::model_call_context::ModelCallIdentityContext::model_call),
            })
            .await
            .map_err(|error| rua_core::RuaError::Runtime {
                message: format!("context summary failed: {error}"),
            })?
            .summary;
        if summary.trim().is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: "context summary produced an empty result".to_string(),
            });
        }
        self.publish_summary(frontier, base, summary, &can_publish)
            .await
    }

    pub async fn publish_precomputed<F>(
        &self,
        frontier: &ProjectionFoldFrontier,
        summary: String,
        can_publish: F,
    ) -> rua_core::Result<Option<SummaryCheckpoint>>
    where
        F: Fn() -> bool,
    {
        if summary.trim().is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: "context summary produced an empty result".to_string(),
            });
        }
        if !can_publish() {
            return Ok(None);
        }
        let Some(base) = self.publication_base(frontier).await? else {
            return Ok(None);
        };
        self.publish_summary(frontier, base, summary, &can_publish)
            .await
    }

    async fn publication_base(
        &self,
        frontier: &ProjectionFoldFrontier,
    ) -> rua_core::Result<Option<CheckpointPublicationBase>> {
        let expected_active_checkpoint_id = self
            .store
            .load_chat_header(&self.chat_id)
            .await?
            .and_then(|header| header.active_checkpoint_id);
        let previous = self
            .store
            .load_active_summary_checkpoint(&self.chat_id)
            .await?;
        let previous_checkpoint_id = previous
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone());
        let source_start = previous
            .as_ref()
            .map(|checkpoint| checkpoint.frontier.end_sequence.saturating_add(1))
            .unwrap_or(frontier.start_sequence);
        if source_start > frontier.end_sequence {
            return Ok(None);
        }
        let projection_state = self
            .store
            .checkpoint_projection_state(&self.chat_id, frontier.end_sequence)
            .await?;
        Ok(Some(CheckpointPublicationBase {
            previous,
            previous_checkpoint_id,
            expected_active_checkpoint_id,
            source_start,
            projection_state,
        }))
    }

    async fn publish_summary<F>(
        &self,
        frontier: &ProjectionFoldFrontier,
        base: CheckpointPublicationBase,
        summary: String,
        can_publish: &F,
    ) -> rua_core::Result<Option<SummaryCheckpoint>>
    where
        F: Fn() -> bool,
    {
        if !can_publish() {
            return Ok(None);
        }
        let CheckpointPublicationBase {
            previous,
            previous_checkpoint_id,
            expected_active_checkpoint_id,
            source_start,
            projection_state,
        } = base;

        let covered_turn_start = previous
            .as_ref()
            .map(|checkpoint| checkpoint.covered_turn_range.start_turn_ordinal)
            .unwrap_or(frontier.start_turn_ordinal);
        let covered_sequence_start = previous
            .as_ref()
            .map(|checkpoint| checkpoint.covered_sequence_range.start_sequence)
            .unwrap_or(frontier.start_sequence);
        let checkpoint = self
            .store
            .publish_summary_checkpoint(
                &self.chat_id,
                expected_active_checkpoint_id.as_deref(),
                SummaryCheckpointDraft {
                    checkpoint_id: uuid::Uuid::new_v4().to_string(),
                    previous_checkpoint_id,
                    covered_turn_range: CheckpointTurnRange {
                        start_turn_ordinal: covered_turn_start,
                        end_turn_ordinal: frontier.end_turn_ordinal,
                    },
                    covered_sequence_range: CheckpointSequenceRange {
                        start_sequence: covered_sequence_start,
                        end_sequence: frontier.end_sequence,
                    },
                    source_sequence_range: CheckpointSequenceRange {
                        start_sequence: source_start,
                        end_sequence: frontier.end_sequence,
                    },
                    frontier: projection_state.frontier,
                    retained_sequence_ranges: projection_state.retained_sequence_ranges,
                    projection_lineage: projection_state.projection_lineage,
                    token_estimate: estimate_text_tokens(
                        &summary,
                        self.running_config
                            .context_compact
                            .token_count_estimate_divisor,
                    ),
                    summary,
                    model: self.model.clone(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .await?;
        if let Some(hook) = self.replication_hook.as_ref() {
            hook.on_session_state_changed(
                &self.chat_address,
                &serde_json::json!({
                    "kind": "summary_checkpoint_published",
                    "workspace_dir": self.workspace_dir,
                    "checkpoint_id": checkpoint.checkpoint_id,
                }),
            )
            .await;
        }
        Ok(Some(checkpoint))
    }
}

fn thinking_message_indices_to_keep(
    messages: &[agent_types::ModelDialogueItem],
    thinking_turns_to_keep: i32,
) -> HashSet<usize> {
    if thinking_turns_to_keep < 0 {
        return (0..messages.len()).collect();
    }

    let mut remaining = thinking_turns_to_keep as usize;
    let mut kept = HashSet::new();
    for (index, message) in messages.iter().enumerate().rev() {
        if message.role == agent_types::Role::Assistant && remaining > 0 {
            kept.insert(index);
            remaining -= 1;
        }
    }
    kept
}

fn retain_summary_thinking(
    messages: &mut Vec<agent_types::ModelDialogueItem>,
    running: &AgentsRunningConfig,
) {
    let turns = if running.context_compact.compact_with_thinking_block {
        running.context_compact.thinking_turns_to_keep
    } else {
        0
    };
    let kept = thinking_message_indices_to_keep(messages, turns);
    for (index, message) in messages.iter_mut().enumerate() {
        if !kept.contains(&index) {
            message
                .content
                .retain(|part| !matches!(part, ContentPart::Thinking { .. }));
        }
    }
    messages.retain(|message| !message.content.is_empty());
}

fn remove_thinking_parts(messages: &mut Vec<agent_types::ModelDialogueItem>) -> usize {
    let mut removed = 0_usize;
    for message in messages.iter_mut() {
        message.content.retain(|part| {
            let keep = !matches!(part, ContentPart::Thinking { .. });
            removed += usize::from(!keep);
            keep
        });
    }
    messages.retain(|message| !message.content.is_empty());
    removed
}

struct SummaryToolRoundReplacement {
    tool_use: Option<(usize, usize, ContentPart)>,
    tool_result: (usize, usize, ContentPart),
    saved_bytes: usize,
}

fn summary_tool_round_replacements(
    messages: &[agent_types::ModelDialogueItem],
) -> rua_core::Result<Vec<SummaryToolRoundReplacement>> {
    let mut tool_uses = HashMap::<String, (usize, usize)>::new();
    for (message_index, message) in messages.iter().enumerate() {
        for (part_index, part) in message.content.iter().enumerate() {
            if let ContentPart::ToolUse { call } = part {
                tool_uses.insert(call.id.clone(), (message_index, part_index));
            }
        }
    }

    let mut replacements = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        for (part_index, part) in message.content.iter().enumerate() {
            let ContentPart::ToolResult { result } = part else {
                continue;
            };
            let recall_key = result
                .truncation
                .as_ref()
                .and_then(|truncation| truncation.recall_key.clone())
                .unwrap_or_else(|| rua_tool::result_store::recall_key(&result.call_id));
            let tool_result = ContentPart::ToolResult {
                result: compact_summary_tool_result(result, &recall_key),
            };
            let mut original_bytes = render_content_part(part)?.len();
            let mut replacement_bytes = render_content_part(&tool_result)?.len();
            let tool_use = if let Some(&(tool_message_index, tool_part_index)) =
                tool_uses.get(&result.call_id)
            {
                let part = &messages[tool_message_index].content[tool_part_index];
                let ContentPart::ToolUse { call } = part else {
                    unreachable!("tool-use index must identify a ToolUse part");
                };
                let replacement = ContentPart::ToolUse {
                    call: compact_summary_tool_call(call, &recall_key),
                };
                original_bytes = original_bytes.saturating_add(render_content_part(part)?.len());
                replacement_bytes =
                    replacement_bytes.saturating_add(render_content_part(&replacement)?.len());
                Some((tool_message_index, tool_part_index, replacement))
            } else {
                None
            };
            replacements.push(SummaryToolRoundReplacement {
                tool_use,
                tool_result: (message_index, part_index, tool_result),
                saved_bytes: original_bytes.saturating_sub(replacement_bytes),
            });
        }
    }
    Ok(replacements)
}

fn compact_summary_tool_call(call: &ToolCall, recall_key: &str) -> ToolCall {
    ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        arguments: serde_json::json!({
            "_compacted": "arguments omitted from compaction input",
            "recall_key": recall_key,
        }),
        raw_arguments: None,
        provider_call_id: call.provider_call_id.clone(),
        metadata: serde_json::Map::new(),
    }
}

fn compact_summary_tool_result(result: &ToolResult, recall_key: &str) -> ToolResult {
    ToolResult {
        call_id: result.call_id.clone(),
        name: result.name.clone(),
        content: vec![ContentPart::Text {
            text: format!(
                "Tool result omitted from compaction input. For full output, call \
                 recall_tool_result(recall_key=\"{recall_key}\")."
            ),
        }],
        is_error: result.is_error,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    }
}

fn render_content_part(part: &ContentPart) -> rua_core::Result<String> {
    serde_json::to_string(part).map_err(|error| rua_core::RuaError::Runtime {
        message: format!("failed to encode projected summary content part: {error}"),
    })
}

fn estimate_text_tokens(text: &str, divisor: f64) -> u64 {
    ((text.len() as f64 / divisor.max(1.0)).ceil() as u64).max(1)
}
