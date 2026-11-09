#![forbid(unsafe_code)]

pub mod capability_cache;
mod catalog;
pub mod cells;
pub mod config_loader;
mod configured;
pub mod dashscope;
pub mod error;
pub mod fallback;
pub mod idealab;
#[cfg(feature = "anthropic")]
pub mod idealab_anthropic;
#[cfg(feature = "gemini")]
pub mod idealab_vertex;
pub mod local_model_resume;
pub mod manager;
pub mod migrate;
pub mod model_factory;
pub mod model_knowledge;
pub mod openrouter;
pub mod openrouter_extended;
pub mod redaction;
pub mod resolver;
pub mod retry;
pub mod router;
mod secret_field_io;
pub mod usage;
pub mod usage_recording;

pub use cells::{ProviderCell, ProviderCells};
pub use config_loader::{DefaultModel, ProviderConfig, ProviderConfigLoader, ProviderModelEntry};
pub use error::ErrorKind;
pub use fallback::{FallbackChatModel, FallbackMode, FallbackPolicy, FallbackResolver};
pub use llm_multiplex::ProbeResult;
pub use local_model_resume::{resume_local_models, LocalModelStatus, ResumedLocalModel};
pub use manager::{infer_context_window, ProviderManager};
pub use migrate::{migrate_legacy_providers, MigrateError};
pub use model_factory::ModelFactory;
pub use redaction::redact_provider_secret;
pub use resolver::{resolve_default_model, ResolvedModel, Unavailable};
pub use retry::{RetryChatModel, RetryPolicy};
pub use router::{RouteRequest, Router};
pub use usage_recording::UsageRecordingChatModel;
