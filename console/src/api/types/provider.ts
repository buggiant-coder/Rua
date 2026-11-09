export interface ModelInfo {
  id: string;
  name: string;
  supports_multimodal: boolean | null;
  supports_image: boolean | null;
  supports_video: boolean | null;
  context_window: number | null;
  max_output_tokens?: number | null;
  is_free?: boolean;
  generate_kwargs: Record<string, unknown>;
  thinking_enabled?: boolean | null;
  thinking_budget?: number | null;
  reasoning_effort?: string | null;
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;
  relay_reasoning?: boolean | null;
}

export interface ProviderBaseUrlOption {
  label: string;
  value: string;
}

export interface ProviderInfo {
  id: string;
  name: string;
  api_key_prefix: string;
  api_key_prefixes?: string[];
  chat_model: string;

  models: ModelInfo[];

  extra_models: ModelInfo[];
  is_custom: boolean;
  is_local: boolean;

  support_model_discovery: boolean;

  support_connection_check: boolean;

  freeze_url: boolean;

  require_api_key: boolean;
  api_key: string;
  base_url: string;
  generate_kwargs: Record<string, unknown>;
  custom_headers?: Record<string, unknown>;
  provider_group?: string;
  provider_group_name?: string;
  provider_variant?: string;
  provider_adapter?: string;
  base_url_options?: ProviderBaseUrlOption[];
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;

  supports_oauth?: boolean;

  oauth_connected?: boolean;

  is_free_tier?: boolean;
}

export interface ProviderConfigRequest {
  api_key?: string;
  base_url?: string;
  chat_model?: string;
  api_key_prefixes?: string[];
  custom_headers?: Record<string, unknown>;
  generate_kwargs?: Record<string, unknown>;
  provider_group?: string;
  provider_group_name?: string;
  provider_variant?: string;
  provider_adapter?: string;
  base_url_options?: ProviderBaseUrlOption[];
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;
}

export interface ModelSlotConfig {
  provider_id: string;
  model: string;
}

export interface ActiveModelsInfo {
  default_llm?: ModelSlotConfig;
}

export type ActiveModelScope = "effective" | "global" | "agent" | "session";

export interface GetActiveModelsRequest {
  scope?: ActiveModelScope;
  agent_id?: string;
}

export interface ModelSlotRequest {
  provider_id: string;
  model: string;
  scope: Exclude<ActiveModelScope, "effective">;
  agent_id?: string;
  chat_id?: string;
}

export interface CreateCustomProviderRequest {
  id: string;
  template_id?: string;
  name: string;
  base_url?: string;
  api_key_prefix?: string;
  api_key_prefixes?: string[];
  chat_model?: string;
  models?: ModelInfo[];
  custom_headers?: Record<string, unknown>;
  provider_group?: string;
  provider_group_name?: string;
  provider_variant?: string;
  provider_adapter?: string;
  base_url_options?: ProviderBaseUrlOption[];
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;
}

export interface AddModelRequest {
  id: string;
  name: string;
}

export interface ModelConfigRequest {
  generate_kwargs?: Record<string, unknown>;
  context_window?: number | null;
  thinking_enabled?: boolean | null;
  thinking_budget?: number | null;
  reasoning_effort?: string | null;
  thinking_param_style?: string | null;
  reasoning_effort_options?: string[];
  thinking_budget_range?: [number, number] | null;
  relay_reasoning?: boolean | null;
}

export interface LocalModelInfo {
  id: string;
  name: string;
  size_bytes: number;
  downloaded: boolean;
  source: LocalDownloadSource;
}

export type LocalDownloadSource = "huggingface" | "modelscope" | "auto";

export interface LocalServerStatus {
  available: boolean;
  installable: boolean;
  installed: boolean;
  port: number | null;
  model_name: string | null;
  message: string | null;
}

export interface LocalServerUpdateStatus {
  has_update: boolean;
}

export interface LocalDownloadProgress {
  status:
    | "idle"
    | "pending"
    | "downloading"
    | "canceling"
    | "completed"
    | "failed"
    | "cancelled";
  model_name: string | null;
  downloaded_bytes: number;
  total_bytes: number | null;
  speed_bytes_per_sec: number;
  source: LocalDownloadSource | null;
  error: string | null;
  local_path: string | null;
}

export interface LocalActionResponse {
  status: string;
  message: string;
}

export interface StartLocalServerRequest {
  model_id: string;
}

export interface TestConnectionResponse {
  success: boolean;
  message: string;
}

export interface TestProviderRequest {
  api_key?: string;
  base_url?: string;
  chat_model?: string;
  custom_headers?: Record<string, unknown>;
  generate_kwargs?: Record<string, unknown>;
  include_extended?: boolean;
}

export interface TestModelRequest {
  model_id: string;
}

export interface DiscoverModelsResponse {
  success: boolean;
  message: string;
  models: ModelInfo[];
  added_count: number;
}

export interface ProbeMultimodalResponse {
  supports_image: boolean | null;
  supports_video: boolean | null;
  supports_multimodal: boolean | null;
  message: string | null;
}

export interface ExtendedModelInfo {
  id: string;
  name: string;
  provider: string;
  input_modalities: string[];
  output_modalities: string[];
  pricing: Record<string, string>;
  is_free?: boolean;
}

export interface FilterModelsRequest {
  providers?: string[];
  input_modalities?: string[];
  output_modalities?: string[];
  max_prompt_price?: number;
}

export interface SeriesResponse {
  series: string[];
}

export interface DiscoverExtendedResponse {
  success: boolean;
  models: ExtendedModelInfo[];
  providers: string[];
  total_count: number;
}

export interface FilterModelsResponse {
  success: boolean;
  models: ExtendedModelInfo[];
  total_count: number;
}
