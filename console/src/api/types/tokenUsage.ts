export interface TokenUsageRecord {
  date: string;
  provider_id: string;
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  call_count: number;
}

export interface TokenUsageStats {
  provider_id?: string;
  model?: string;
  prompt_tokens: number;
  completion_tokens: number;
  call_count: number;
}

export interface TokenUsageSummary {
  total_prompt_tokens: number;
  total_completion_tokens: number;
  total_calls: number;
  by_model: Record<string, TokenUsageStats>;
  by_date: Record<string, TokenUsageStats>;
}
