use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSlotConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub provider_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub model: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingConfig {
    #[serde(
        default = "default_openai",
        deserialize_with = "deserialize_null_default"
    )]
    pub backend: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub model_name: String,
    #[serde(
        default = "default_dimensions",
        deserialize_with = "deserialize_null_default"
    )]
    pub dimensions: u32,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enable_cache: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub use_dimensions: bool,
    #[serde(
        default = "default_cache_size",
        deserialize_with = "deserialize_null_default"
    )]
    pub max_cache_size: u32,
    #[serde(
        default = "default_input_length",
        deserialize_with = "deserialize_null_default"
    )]
    pub max_input_length: u32,
    #[serde(
        default = "default_batch_size",
        deserialize_with = "deserialize_null_default"
    )]
    pub max_batch_size: u32,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_openai() -> String {
    "openai".into()
}
fn default_dimensions() -> u32 {
    1024
}
fn default_true() -> bool {
    true
}
fn default_cache_size() -> u32 {
    3000
}
fn default_input_length() -> u32 {
    8192
}
fn default_batch_size() -> u32 {
    10
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            backend: default_openai(),
            api_key: String::new(),
            base_url: String::new(),
            model_name: String::new(),
            dimensions: default_dimensions(),
            enable_cache: true,
            use_dimensions: false,
            max_cache_size: default_cache_size(),
            max_input_length: default_input_length(),
            max_batch_size: default_batch_size(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextCompactConfig {
    #[serde(
        default = "default_token_model",
        deserialize_with = "deserialize_null_default"
    )]
    pub token_count_model: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub token_count_use_mirror: bool,
    #[serde(
        default = "default_divisor",
        deserialize_with = "deserialize_null_default"
    )]
    pub token_count_estimate_divisor: f64,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub context_compact_enabled: bool,
    #[serde(
        default = "default_blocking_compact_ratio",
        deserialize_with = "deserialize_null_default"
    )]
    pub blocking_compact_ratio: f64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub history_replacement_enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub active_work_plan_injection_enabled: bool,
    #[serde(
        default = "default_active_work_plan_injection_interval_ratio",
        deserialize_with = "deserialize_null_default"
    )]
    pub active_work_plan_injection_interval_ratio: f64,
    #[serde(
        default,
        alias = "perpetual_conversation_enabled",
        deserialize_with = "deserialize_null_default"
    )]
    pub early_background_compaction_enabled: bool,
    #[serde(
        default = "default_early_compaction_target_ratio",
        alias = "perpetual_sweet_spot_ratio",
        deserialize_with = "deserialize_null_default"
    )]
    pub early_compaction_target_ratio: f64,
    #[serde(
        default = "default_reserve_ratio",
        deserialize_with = "deserialize_null_default"
    )]
    pub context_reserve_ratio: f64,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub compact_with_thinking_block: bool,
    #[serde(
        default = "default_one_i32",
        deserialize_with = "deserialize_null_default"
    )]
    pub thinking_turns_to_keep: i32,
    #[serde(
        default = "default_min_keep",
        deserialize_with = "deserialize_null_default"
    )]
    pub min_keep_messages: u32,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub compact_enhanced_prompt: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_token_model() -> String {
    "default".into()
}
fn default_divisor() -> f64 {
    4.0
}
fn default_blocking_compact_ratio() -> f64 {
    0.9
}
fn default_early_compaction_target_ratio() -> f64 {
    0.5
}
fn default_active_work_plan_injection_interval_ratio() -> f64 {
    0.05
}
fn default_reserve_ratio() -> f64 {
    0.1
}
fn default_one_i32() -> i32 {
    1
}
fn default_min_keep() -> u32 {
    5
}

impl Default for ContextCompactConfig {
    fn default() -> Self {
        Self {
            token_count_model: default_token_model(),
            token_count_use_mirror: false,
            token_count_estimate_divisor: default_divisor(),
            context_compact_enabled: true,
            blocking_compact_ratio: default_blocking_compact_ratio(),
            history_replacement_enabled: false,
            active_work_plan_injection_enabled: false,
            active_work_plan_injection_interval_ratio:
                default_active_work_plan_injection_interval_ratio(),
            early_background_compaction_enabled: false,
            early_compaction_target_ratio: default_early_compaction_target_ratio(),
            context_reserve_ratio: default_reserve_ratio(),
            compact_with_thinking_block: true,
            thinking_turns_to_keep: 1,
            min_keep_messages: default_min_keep(),
            compact_enhanced_prompt: true,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolResultCompactConfig {
    pub enabled: bool,
    pub projection_truncation_enabled: bool,
    /// Completed tool results per second-pass truncation batch; 1 preserves rolling truncation.
    pub projection_truncation_batch_size: u32,
    pub recent_n: u32,
    pub old_max_bytes: u32,
    pub execution_output_max_tokens: u32,
    pub raw_capture_max_bytes: u32,
    pub recall_slice_max_tokens: u32,
    pub retention_days: u32,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct ToolResultCompactConfigWire {
    enabled: Option<bool>,
    projection_truncation_enabled: Option<bool>,
    projection_truncation_batch_size: Option<u32>,
    recent_n: Option<u32>,
    old_max_bytes: Option<u32>,
    execution_output_max_tokens: Option<u32>,
    raw_capture_max_bytes: Option<u32>,
    recall_slice_max_tokens: Option<u32>,
    recent_max_bytes: Option<u32>,
    retention_days: Option<u32>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

const LEGACY_DEFAULT_TOOL_RECENT_MAX_BYTES: u32 = 10_000;

impl<'de> Deserialize<'de> for ToolResultCompactConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ToolResultCompactConfigWire::deserialize(deserializer)?;
        let execution_output_max_tokens = wire
            .execution_output_max_tokens
            .or_else(|| {
                wire.recent_max_bytes.map(|bytes| {
                    if bytes == 0 || bytes == LEGACY_DEFAULT_TOOL_RECENT_MAX_BYTES {
                        default_execution_output_max_tokens()
                    } else {
                        bytes.div_ceil(4).max(1)
                    }
                })
            })
            .unwrap_or_else(default_execution_output_max_tokens);
        Ok(Self {
            enabled: wire.enabled.unwrap_or_else(default_true),
            projection_truncation_enabled: wire.projection_truncation_enabled.unwrap_or(false),
            projection_truncation_batch_size: wire.projection_truncation_batch_size.unwrap_or(1),
            recent_n: wire.recent_n.unwrap_or_else(default_recent_n),
            old_max_bytes: wire.old_max_bytes.unwrap_or_else(default_old_max),
            execution_output_max_tokens,
            raw_capture_max_bytes: wire
                .raw_capture_max_bytes
                .unwrap_or_else(default_raw_capture_max_bytes),
            recall_slice_max_tokens: wire
                .recall_slice_max_tokens
                .unwrap_or_else(default_recall_slice_max_tokens),
            retention_days: wire.retention_days.unwrap_or_else(default_retention_days),
            extra: wire.extra,
        })
    }
}

fn default_recent_n() -> u32 {
    2
}
fn default_old_max() -> u32 {
    rua_core::tool::DEFAULT_TOOL_OLD_MAX_BYTES as u32
}
fn default_execution_output_max_tokens() -> u32 {
    10_000
}
fn default_raw_capture_max_bytes() -> u32 {
    1024 * 1024
}
fn default_recall_slice_max_tokens() -> u32 {
    10_000
}
fn default_retention_days() -> u32 {
    5
}

impl Default for ToolResultCompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            projection_truncation_enabled: false,
            projection_truncation_batch_size: 1,
            recent_n: default_recent_n(),
            old_max_bytes: default_old_max(),
            execution_output_max_tokens: default_execution_output_max_tokens(),
            raw_capture_max_bytes: default_raw_capture_max_bytes(),
            recall_slice_max_tokens: default_recall_slice_max_tokens(),
            retention_days: default_retention_days(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySearchMode {
    Grep,
    #[default]
    Precise,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySummaryConfig {
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub memory_summary_enabled: bool,
    #[serde(
        default = "default_dream_cron",
        deserialize_with = "deserialize_null_dream_cron"
    )]
    pub dream_cron: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub search_mode: MemorySearchMode,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_dream_cron() -> String {
    "0 23 * * *".into()
}

fn deserialize_null_dream_cron<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_dream_cron))
}

fn default_one_u32() -> u32 {
    1
}

impl Default for MemorySummaryConfig {
    fn default() -> Self {
        Self {
            memory_summary_enabled: true,
            dream_cron: default_dream_cron(),
            search_mode: MemorySearchMode::Precise,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDisclosureEnabled {
    #[default]
    Auto,
    On,
    Off,
}

impl<'de> Deserialize<'de> for ToolDisclosureEnabled {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value.as_deref() {
            Some("on") => Self::On,
            Some("off") => Self::Off,
            Some("auto") | None => Self::Auto,
            Some(_) => Self::Auto,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDisclosureRunningConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub enabled: ToolDisclosureEnabled,
    #[serde(default = "default_tool_disclosure_threshold_pct")]
    pub threshold_pct: f64,
    #[serde(default = "default_tool_disclosure_search_default_limit")]
    pub search_default_limit: u32,
    #[serde(default = "default_tool_disclosure_search_max_limit")]
    pub search_max_limit: u32,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for ToolDisclosureRunningConfig {
    fn default() -> Self {
        Self {
            enabled: ToolDisclosureEnabled::Auto,
            threshold_pct: default_tool_disclosure_threshold_pct(),
            search_default_limit: default_tool_disclosure_search_default_limit(),
            search_max_limit: default_tool_disclosure_search_max_limit(),
            extra: serde_json::Map::new(),
        }
    }
}

impl ToolDisclosureRunningConfig {
    pub fn normalized(&self) -> Self {
        let max_limit = if self.search_max_limit == 0 {
            default_tool_disclosure_search_max_limit()
        } else {
            self.search_max_limit
        };
        let default_limit = if self.search_default_limit == 0 {
            default_tool_disclosure_search_default_limit()
        } else {
            self.search_default_limit.min(max_limit)
        };
        Self {
            enabled: self.enabled,
            threshold_pct: if self.threshold_pct.is_finite() && self.threshold_pct > 0.0 {
                self.threshold_pct
            } else {
                default_tool_disclosure_threshold_pct()
            },
            search_default_limit: default_limit,
            search_max_limit: max_limit,
            extra: self.extra.clone(),
        }
    }
}

fn default_tool_disclosure_threshold_pct() -> f64 {
    10.0
}

fn default_tool_disclosure_search_default_limit() -> u32 {
    5
}

fn default_tool_disclosure_search_max_limit() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentsRunningConfig {
    #[serde(
        default = "default_max_iters",
        deserialize_with = "deserialize_null_max_iters"
    )]
    /// Maximum tool rounds per run; zero leaves the run unlimited.
    pub max_iters: u32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub auto_continue_on_text_only: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub show_debug_menu: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub llm_retry_enabled: bool,
    #[serde(
        default = "default_one_u32",
        deserialize_with = "deserialize_null_one_u32"
    )]
    pub llm_max_retries: u32,
    #[serde(
        default = "default_backoff_base",
        deserialize_with = "deserialize_null_backoff_base"
    )]
    pub llm_backoff_base: f64,
    #[serde(
        default = "default_backoff_cap",
        deserialize_with = "deserialize_null_backoff_cap"
    )]
    pub llm_backoff_cap: f64,
    #[serde(
        default = "default_max_concurrent",
        deserialize_with = "deserialize_null_max_concurrent"
    )]
    pub llm_max_concurrent: u32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub llm_max_qpm: u32,
    #[serde(
        default = "default_rate_pause",
        deserialize_with = "deserialize_null_rate_pause"
    )]
    pub llm_rate_limit_pause: f64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub llm_rate_limit_jitter: f64,
    #[serde(
        default = "default_acquire_timeout",
        deserialize_with = "deserialize_null_acquire_timeout"
    )]
    pub llm_acquire_timeout: f64,
    #[serde(
        default = "default_max_input",
        deserialize_with = "deserialize_null_max_input"
    )]
    pub max_input_length: u32,
    #[serde(
        default = "default_history_max",
        deserialize_with = "deserialize_null_history_max"
    )]
    pub history_max_length: u32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub context_compact: ContextCompactConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_result_compact: ToolResultCompactConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_disclosure: ToolDisclosureRunningConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub memory_summary: MemorySummaryConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub embedding_config: EmbeddingConfig,
    #[serde(
        default = "default_memory_backend",
        deserialize_with = "deserialize_null_memory_backend"
    )]
    pub memory_manager_backend: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_max_iters() -> u32 {
    100
}
fn default_backoff_base() -> f64 {
    1.0
}
fn default_backoff_cap() -> f64 {
    30.0
}
fn default_max_concurrent() -> u32 {
    5
}
fn default_rate_pause() -> f64 {
    30.0
}
fn default_acquire_timeout() -> f64 {
    120.0
}
fn default_max_input() -> u32 {
    131072
}
fn default_history_max() -> u32 {
    10000
}
fn default_memory_backend() -> String {
    "remelight".into()
}

fn deserialize_null_u32_with<'de, D>(deserializer: D, default: fn() -> u32) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<u32>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default))
}

fn deserialize_null_f64_with<'de, D>(deserializer: D, default: fn() -> f64) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<f64>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default))
}

fn deserialize_null_string_with<'de, D>(
    deserializer: D,
    default: fn() -> String,
) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default))
}

fn deserialize_null_max_iters<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_u32_with(deserializer, default_max_iters)
}

fn deserialize_null_one_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_u32_with(deserializer, default_one_u32)
}

fn deserialize_null_backoff_base<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_f64_with(deserializer, default_backoff_base)
}

fn deserialize_null_backoff_cap<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_f64_with(deserializer, default_backoff_cap)
}

fn deserialize_null_max_concurrent<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_u32_with(deserializer, default_max_concurrent)
}

fn deserialize_null_rate_pause<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_f64_with(deserializer, default_rate_pause)
}

fn deserialize_null_acquire_timeout<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_f64_with(deserializer, default_acquire_timeout)
}

fn deserialize_null_max_input<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_u32_with(deserializer, default_max_input)
}

fn deserialize_null_history_max<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_u32_with(deserializer, default_history_max)
}

fn deserialize_null_memory_backend<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_null_string_with(deserializer, default_memory_backend)
}

impl Default for AgentsRunningConfig {
    fn default() -> Self {
        Self {
            max_iters: default_max_iters(),
            auto_continue_on_text_only: false,
            show_debug_menu: false,
            llm_retry_enabled: false,
            llm_max_retries: 1,
            llm_backoff_base: default_backoff_base(),
            llm_backoff_cap: default_backoff_cap(),
            llm_max_concurrent: default_max_concurrent(),
            llm_max_qpm: 0,
            llm_rate_limit_pause: default_rate_pause(),
            llm_rate_limit_jitter: 0.0,
            llm_acquire_timeout: default_acquire_timeout(),
            max_input_length: default_max_input(),
            history_max_length: default_history_max(),
            context_compact: ContextCompactConfig::default(),
            tool_result_compact: ToolResultCompactConfig::default(),
            tool_disclosure: ToolDisclosureRunningConfig::default(),
            memory_summary: MemorySummaryConfig::default(),
            embedding_config: EmbeddingConfig::default(),
            memory_manager_backend: default_memory_backend(),
            extra: serde_json::Map::new(),
        }
    }
}

impl AgentsRunningConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.llm_max_retries < 1 {
            return Err("llm_max_retries must be >= 1".into());
        }
        if !self.llm_backoff_base.is_finite() || self.llm_backoff_base < 0.1 {
            return Err("llm_backoff_base must be >= 0.1".into());
        }
        if !self.llm_backoff_cap.is_finite() || self.llm_backoff_cap < 0.5 {
            return Err("llm_backoff_cap must be >= 0.5".into());
        }
        if self.llm_backoff_cap < self.llm_backoff_base {
            return Err(format!(
                "llm_backoff_cap ({}) must be >= llm_backoff_base ({})",
                self.llm_backoff_cap, self.llm_backoff_base
            ));
        }
        if self.llm_max_concurrent < 1 {
            return Err("llm_max_concurrent must be >= 1".into());
        }
        if !self.llm_rate_limit_pause.is_finite() || self.llm_rate_limit_pause < 1.0 {
            return Err("llm_rate_limit_pause must be >= 1.0".into());
        }
        if !self.llm_rate_limit_jitter.is_finite() || self.llm_rate_limit_jitter < 0.0 {
            return Err("llm_rate_limit_jitter must be >= 0.0".into());
        }
        if !self.llm_acquire_timeout.is_finite() || self.llm_acquire_timeout < 10.0 {
            return Err("llm_acquire_timeout must be >= 10.0".into());
        }
        if self.max_input_length < 1000 {
            return Err("max_input_length must be >= 1000".into());
        }
        if self.tool_result_compact.projection_truncation_batch_size == 0 {
            return Err("tool_result_compact.projection_truncation_batch_size must be >= 1".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatRunningConfig {
    pub max_iters: u32,
    pub auto_continue_on_text_only: bool,
    pub show_debug_menu: bool,
    pub llm_retry_enabled: bool,
    pub llm_max_retries: u32,
    pub llm_backoff_base: f64,
    pub llm_backoff_cap: f64,
    pub llm_max_concurrent: u32,
    pub llm_max_qpm: u32,
    pub llm_rate_limit_pause: f64,
    pub llm_rate_limit_jitter: f64,
    pub llm_acquire_timeout: f64,
    pub max_input_length: u32,
    pub history_max_length: u32,
    pub context_compact: ContextCompactConfig,
    pub tool_result_compact: ToolResultCompactConfig,
    pub tool_disclosure: ToolDisclosureRunningConfig,
    pub memory_summary: MemorySummaryConfig,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatRunningConfig {
    pub fn from_agent_defaults(defaults: &AgentsRunningConfig) -> Self {
        Self {
            max_iters: defaults.max_iters,
            auto_continue_on_text_only: defaults.auto_continue_on_text_only,
            show_debug_menu: defaults.show_debug_menu,
            llm_retry_enabled: defaults.llm_retry_enabled,
            llm_max_retries: defaults.llm_max_retries,
            llm_backoff_base: defaults.llm_backoff_base,
            llm_backoff_cap: defaults.llm_backoff_cap,
            llm_max_concurrent: defaults.llm_max_concurrent,
            llm_max_qpm: defaults.llm_max_qpm,
            llm_rate_limit_pause: defaults.llm_rate_limit_pause,
            llm_rate_limit_jitter: defaults.llm_rate_limit_jitter,
            llm_acquire_timeout: defaults.llm_acquire_timeout,
            max_input_length: defaults.max_input_length,
            history_max_length: defaults.history_max_length,
            context_compact: defaults.context_compact.clone(),
            tool_result_compact: defaults.tool_result_compact.clone(),
            tool_disclosure: defaults.tool_disclosure.clone(),
            memory_summary: defaults.memory_summary.clone(),
            extra: defaults.extra.clone(),
        }
    }

    pub fn resolve(&self) -> Result<AgentsRunningConfig, String> {
        let effective = AgentsRunningConfig {
            max_iters: self.max_iters,
            auto_continue_on_text_only: self.auto_continue_on_text_only,
            show_debug_menu: self.show_debug_menu,
            llm_retry_enabled: self.llm_retry_enabled,
            llm_max_retries: self.llm_max_retries,
            llm_backoff_base: self.llm_backoff_base,
            llm_backoff_cap: self.llm_backoff_cap,
            llm_max_concurrent: self.llm_max_concurrent,
            llm_max_qpm: self.llm_max_qpm,
            llm_rate_limit_pause: self.llm_rate_limit_pause,
            llm_rate_limit_jitter: self.llm_rate_limit_jitter,
            llm_acquire_timeout: self.llm_acquire_timeout,
            max_input_length: self.max_input_length,
            history_max_length: self.history_max_length,
            context_compact: self.context_compact.clone(),
            tool_result_compact: self.tool_result_compact.clone(),
            tool_disclosure: self.tool_disclosure.clone(),
            memory_summary: self.memory_summary.clone(),
            // Shared memory infrastructure is initialized by WorkspaceRuntime, not a Turn.
            embedding_config: EmbeddingConfig::default(),
            memory_manager_backend: default_memory_backend(),
            extra: self.extra.clone(),
        };
        effective.validate()?;
        Ok(effective)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.resolve().map(|_| ())
    }
}

impl Default for ChatRunningConfig {
    fn default() -> Self {
        Self::from_agent_defaults(&AgentsRunningConfig::default())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHoursConfig {
    #[serde(
        default = "default_active_hours_start",
        deserialize_with = "deserialize_null_default"
    )]
    pub start: String,
    #[serde(
        default = "default_active_hours_end",
        deserialize_with = "deserialize_null_default"
    )]
    pub end: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_active_hours_start() -> String {
    "08:00".into()
}
fn default_active_hours_end() -> String {
    "22:00".into()
}

impl Default for ActiveHoursConfig {
    fn default() -> Self {
        Self {
            start: default_active_hours_start(),
            end: default_active_hours_end(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub enabled: bool,
    #[serde(
        default = "default_heartbeat_every",
        deserialize_with = "deserialize_null_default"
    )]
    pub every: String,
    #[serde(
        default = "default_heartbeat_target",
        deserialize_with = "deserialize_null_default"
    )]
    pub target: String,
    #[serde(default, alias = "activeHours")]
    pub active_hours: Option<ActiveHoursConfig>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_heartbeat_every() -> String {
    "6h".into()
}
fn default_heartbeat_target() -> String {
    "main".into()
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            every: default_heartbeat_every(),
            target: default_heartbeat_target(),
            active_hours: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfileRef {
    pub id: String,
    pub workspace_dir: String,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfileConfig {
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub workspace_dir: String,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub language: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub system_prompt_files: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub running: AgentsRunningConfig,
    #[serde(default)]
    pub default_model: Option<ModelSlotConfig>,

    #[serde(default)]
    pub channels: Option<serde_json::Value>,
    #[serde(default)]
    pub mcp: Option<serde_json::Value>,
    #[serde(default)]
    pub heartbeat: Option<serde_json::Value>,
    #[serde(default)]
    pub last_dispatch: Option<serde_json::Value>,
    #[serde(default)]
    pub tools: Option<serde_json::Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(
        default = "default_active_agent",
        deserialize_with = "deserialize_null_default"
    )]
    pub active_agent: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub agent_order: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub profiles: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub active_hours: Option<ActiveHoursConfig>,
    #[serde(default)]
    pub heartbeat: Option<HeartbeatConfig>,

    #[serde(default)]
    pub defaults: Option<serde_json::Value>,
    #[serde(default)]
    pub running: Option<serde_json::Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub system_prompt_files: Option<Vec<String>>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_active_agent() -> String {
    "default".into()
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            active_agent: default_active_agent(),
            agent_order: vec!["default".into()],
            profiles: serde_json::Map::new(),
            active_hours: None,
            heartbeat: None,
            defaults: None,
            running: None,
            language: None,
            system_prompt_files: None,
            extra: serde_json::Map::new(),
        }
    }
}
