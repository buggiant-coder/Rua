#[cfg(feature = "openai-compatible")]
pub mod openai_compatible;

#[cfg(feature = "openai-compatible")]
pub mod openai_responses;

#[cfg(feature = "openai-compatible")]
pub mod dashscope;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "gemini")]
pub mod gemini;

#[cfg(feature = "gemini")]
mod gemini_schema;

#[cfg(feature = "ollama")]
pub mod ollama;
#[cfg(any(
    feature = "openai-compatible",
    feature = "anthropic",
    feature = "gemini",
    feature = "ollama"
))]
mod reasoning;
