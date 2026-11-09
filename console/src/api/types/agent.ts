export interface AgentRequest {
  input: unknown;
  session_id?: string | null;
  user_id?: string | null;
  channel?: string | null;
  [key: string]: unknown;
}

export interface ContextCompactConfig {
  token_count_model: string;
  token_count_use_mirror: boolean;
  token_count_estimate_divisor: number;
  context_compact_enabled: boolean;
  blocking_compact_ratio: number;
  history_replacement_enabled: boolean;
  active_work_plan_injection_enabled?: boolean;
  active_work_plan_injection_interval_ratio?: number;
  early_background_compaction_enabled: boolean;
  early_compaction_target_ratio: number;
  context_reserve_ratio: number;
  compact_with_thinking_block: boolean;
  compact_enhanced_prompt: boolean;
  thinking_turns_to_keep?: number;
  min_keep_messages: number;
}

export interface ToolResultCompactConfig {
  enabled: boolean;
  projection_truncation_enabled: boolean;
  projection_truncation_batch_size?: number;
  recent_n: number;
  old_max_bytes: number;
  execution_output_max_tokens: number;
  raw_capture_max_bytes?: number;
  recall_slice_max_tokens?: number;
  retention_days: number;
}

export type MemorySearchMode = "grep" | "precise";

export interface MemorySummaryConfig {
  memory_summary_enabled: boolean;
  dream_cron: string;
  search_mode: MemorySearchMode;
}

export interface EmbeddingConfig {
  backend: string;
  api_key: string;
  base_url: string;
  model_name: string;
  dimensions: number;
  enable_cache: boolean;
  use_dimensions: boolean;
  max_cache_size: number;
  max_input_length: number;
  max_batch_size: number;
}

export interface AgentsRunningConfig {
  max_iters: number;

  auto_continue_on_text_only: boolean;
  show_debug_menu?: boolean;
  llm_retry_enabled: boolean;
  llm_max_retries: number;
  llm_backoff_base: number;
  llm_backoff_cap: number;
  llm_max_concurrent: number;
  llm_max_qpm: number;
  llm_rate_limit_pause: number;
  llm_rate_limit_jitter: number;
  llm_acquire_timeout: number;
  max_input_length: number;
  history_max_length: number;
  context_compact: ContextCompactConfig;
  tool_result_compact: ToolResultCompactConfig;
  memory_summary: MemorySummaryConfig;
  embedding_config: EmbeddingConfig;
  memory_manager_backend: "remelight";
}

export interface ChatRunningConfig
  extends Pick<
    AgentsRunningConfig,
    | "max_iters"
    | "auto_continue_on_text_only"
    | "show_debug_menu"
    | "llm_retry_enabled"
    | "llm_max_retries"
    | "llm_backoff_base"
    | "llm_backoff_cap"
    | "llm_max_concurrent"
    | "llm_max_qpm"
    | "llm_rate_limit_pause"
    | "llm_rate_limit_jitter"
    | "llm_acquire_timeout"
    | "max_input_length"
    | "history_max_length"
    | "context_compact"
    | "tool_result_compact"
    | "memory_summary"
  > {
  [key: string]: unknown;
}
