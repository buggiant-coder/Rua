import type { AgentsRunningConfig } from "./agent";
import type { TaskChatAddress } from "./task";

export type ChatStatus = "idle" | "running";
export type ConversationActiveTurnPhase =
  | "preparing"
  | "streaming"
  | "waiting_approval"
  | "running_tool"
  | "committing"
  | "stopping";
export type ConversationDriverKind = "rua" | "acp";
export type ConversationAuthorityEpoch = string;

export interface ConversationActiveTurn {
  turn_id: string;
  user_message_id?: string | null;
  submit_device_id?: string | null;
  phase: ConversationActiveTurnPhase;
  authority_device_id: string;
  authority_epoch: ConversationAuthorityEpoch;
  driver: ConversationDriverKind;
  started_at: string;
  queued_count?: number;
  driver_detail?: Record<string, unknown> | null;
}

export type ConversationInputEntryState = "queued";

export interface ConversationInputEntrySnapshot {
  input_id: string;
  submission_id: string;
  state: ConversationInputEntryState;
  text: string;
  created_at: string;
  updated_at: string;
  submit_device_id?: string | null;
  device_delivery_id?: string | null;
  editable?: boolean;
  deletable?: boolean;
}

export interface ConversationInputQueueView {
  entries: ConversationInputEntrySnapshot[];
  paused: boolean;
}

export interface ConversationAcpControlView {
  active: boolean;
  binding?: Record<string, unknown> | null;
}

export type ConversationAuthorityPlacementLockReason =
  | "active_turn"
  | "acp_passthrough";

export interface ConversationAuthorityPlacementView {
  authority_device_id?: string | null;
  authority_epoch?: ConversationAuthorityEpoch | null;
  switchable: boolean;
  lock_reason?: ConversationAuthorityPlacementLockReason | null;
}

export interface ConversationAllowedControls {
  stop: boolean;
  queue: boolean;
  interject: boolean;
  edit_queued_input: boolean;
  delete_queued_input: boolean;
  switch_authority: boolean;
  enter_acp: boolean;
  exit_acp: boolean;
}

export type ConversationBackgroundWorkKind = "task" | "collaboration";

/**
 * One pending background work item reported by the conversation control
 * snapshot. `status` is the producer's own state token (Task state for `task`
 * items, collaboration request state for `collaboration` items) and is never
 * enumerated exhaustively here so unknown values still render.
 */
export interface ConversationBackgroundWorkItem {
  kind: ConversationBackgroundWorkKind;
  id: string;
  status: string;
  chat: TaskChatAddress;
}

export interface ConversationBackgroundWorkView {
  /** True when automatic resume of this conversation is stopped. */
  suppressed: boolean;
  items: ConversationBackgroundWorkItem[];
  /** Present when the background work projection itself is unavailable. */
  error?: string | null;
}

export interface ChatAgentSelection {
  agent_id: string;
  selected_at: string;
  revision: number;
}

export type ChatAgentSwitchBlocker =
  | "active_turn"
  | "queued_input"
  | "unresolved_approval"
  | "unanswered_prompt"
  | "active_acp_binding"
  | "fixed_collaboration_executor"
  | "archived_chat"
  | "archived_project";

export interface ChatAgentState {
  selection: ChatAgentSelection;
  selected_agent_available: boolean;
  switchable: boolean;
  blockers: ChatAgentSwitchBlocker[];
}

export interface ConversationControlView {
  revision: number;
  authority_device_id?: string | null;
  placement: ConversationAuthorityPlacementView;
  active_turn?: ConversationActiveTurn | null;
  queue: ConversationInputQueueView;
  acp: ConversationAcpControlView;
  model?: ModelSlot | null;
  running_config?: AgentsRunningConfig | null;
  chat_agent_state?: ChatAgentState | null;
  allowed_controls: ConversationAllowedControls;
  background_work?: ConversationBackgroundWorkView | null;
}

export interface QueueInputDeleteResponse {
  deleted_input_id: string;
  control: ConversationControlView;
}

export interface QueueInputEditResponse {
  entry: ConversationInputEntrySnapshot;
  control: ConversationControlView;
}

export interface QueueInputSendNowRequest {
  command_id: string;
  authority_epoch: ConversationAuthorityEpoch;
}

export interface QueueInputSendNowResponse {
  input_id: string;
  applied: boolean;
  control: ConversationControlView;
}

export interface ModelSlot {
  provider_id: string;
  model: string;
  thinking_strength?: "off" | "auto" | "low" | "medium" | "high" | null;
  thinking_enabled?: boolean | null;
  thinking_budget?: number | null;
  reasoning_effort?: string | null;
  thinking_param_style?: string | null;
  thinking_budget_range?: [number, number] | null;
  reasoning_effort_options?: string[];
}

export type AgentCollaborationMode = "default" | "plan" | "execute";
export type ChatTag = "none" | "red" | "yellow" | "green";
export type ConversationListView = "user" | "collaboration";

export interface CollaborationChatRelation {
  source: Record<string, unknown>;
  executor: { kind: "agent" } | { kind: "acp"; runner: string };
  executor_agent_id: string;
  configuration: {
    desired_source_epoch: string;
    applied_source_epoch: string;
    status: "ready" | "applying" | "failed";
    error?: string;
  };
  source_available?: boolean;
}

export interface ChatSpec {
  agent_id?: string | null;
  project_id?: string | null;
  id: string;
  user_id: string;
  channel: string;
  name?: string;
  created_at: string | null;
  updated_at: string | null;
  meta?: Record<string, unknown> & {
    execution_directory?: ResolvedExecutionDirectory;
  };
  model?: ModelSlot | null;
  collaboration_mode?: AgentCollaborationMode;
  status?: ChatStatus;
  pinned?: boolean;
  archived?: boolean;
  pin_order?: number;
  tag?: ChatTag;
  collaboration?: CollaborationChatRelation;
}

export interface Message {
  role: string;
  content: unknown;
  [key: string]: unknown;
}

export interface ExecutionPosition {
  device_id: string;
  path?: string;
}

export interface ResolvedExecutionDirectory extends ExecutionPosition {
  path: string;
  managed: boolean;
}

export interface ChatHistory {
  agent_id?: string | null;
  project_id?: string | null;
  messages: Message[];
  status?: ChatStatus;
  active_turn?: ConversationActiveTurn | null;
  control?: ConversationControlView | null;
  model?: ModelSlot | null;
  meta?: Record<string, unknown> & {
    chat_agent_state?: ChatAgentState | null;
    execution_directory?: ResolvedExecutionDirectory;
  };
  title?: string | null;
  oldest_turn_ordinal?: number | null;
  newest_turn_ordinal?: number | null;
  has_more_before?: boolean;
  durable_coverage?: DurableConversationCoverage;
}

export interface DurableConversationCoverage {
  latest_message_sequence: number;
  manifest_hash: string;
}

export interface RuntimeNote {
  runtime_note_id: string;
  kind: string;
  created_at: string;
  payload: Record<string, unknown>;
  [key: string]: unknown;
}

export interface ChatUpdateRequest {
  name?: string;
  archived?: boolean;
  model?: ModelSlot | null;
  collaboration_mode?: AgentCollaborationMode;
  pinned?: boolean;
  pin_order?: number;
  tag?: ChatTag;
  turn_id?: string;
  selected_variant_id?: string;
}

export interface ConversationAuthorityPlacementUpdateRequest {
  device_id: string;
  path?: string;
  model?: ModelSlot;
}

export interface AcpHistoryPreviewRequest {
  preview_id: number;
  text: string;
  runner?: string;
}

export interface AcpHistoryPreviewResponse {
  turn_id: string;
  message_id: string;
  created: boolean;
}

export interface ChatDeleteResponse {
  success: boolean;
  chat_id: string;
}

export interface ChatSearchResult {
  chat_id: string;
  chat_name: string;
  card_index: number;
  message_id?: string;
  turn_id?: string;
  sequence?: number;
  role: string;
  snippet: string;
}

export interface ChatSearchResponse {
  results: ChatSearchResult[];
  offset: number;
  limit: number;
  total: number;
  next_offset: number | null;
}
