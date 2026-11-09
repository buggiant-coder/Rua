#![forbid(unsafe_code)]

pub mod capability_baseline;
pub mod debug_capture;
pub mod error;
pub mod event_parser;
pub mod key_selector;
pub mod probing;
pub mod providers;
pub mod rate_limiter;
pub mod request;
pub mod stream_sanitizer;
pub mod tag_parser;
pub mod traits;
pub mod transport;

pub use capability_baseline::CapabilityBaseline;
pub use error::LlmError;
pub use key_selector::KeySelector;
pub use probing::{MultimodalProber, ProbeConfig, ProbeResult};
pub use rate_limiter::{LlmRateLimiter, RateLimitChatModel, RateLimitConfig, RateLimiterStats};
pub use stream_sanitizer::SanitizedStream;
pub use traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
pub use transport::{HttpTransport, TransportConfig};

#[cfg(feature = "openai-compatible")]
pub use providers::openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

#[cfg(feature = "openai-compatible")]
pub use providers::openai_responses::OpenAiResponsesProvider;

#[cfg(feature = "openai-compatible")]
pub use providers::dashscope;

#[cfg(feature = "anthropic")]
pub use providers::anthropic::{AnthropicConfig, AnthropicProvider};

#[cfg(feature = "gemini")]
pub use providers::gemini::{GeminiConfig, GeminiProvider};

#[cfg(feature = "ollama")]
pub use providers::ollama::{OllamaConfig, OllamaProvider};
