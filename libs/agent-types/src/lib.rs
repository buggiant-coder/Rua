#![forbid(unsafe_code)]

pub mod attachment;
pub mod content;
pub mod conversation_message_identity;
pub mod event;
pub mod finish;
pub mod model_dialogue_item;
pub mod provider_mapping;
pub mod request;
pub mod response;
pub mod role;
pub mod tool;
pub mod usage;
pub mod webui_mapping;

pub use attachment::AttachmentRef;
pub use content::ContentPart;
pub use event::{AgentEvent, ChatStreamEvent, ToolCallDeltaAccumulator};
pub use finish::FinishReason;
pub use model_dialogue_item::ModelDialogueItem;
pub use request::{
    ChatRequest, ReasoningConfig, ReasoningMode, ReasoningSummary, StructuredOutput, ToolChoice,
};
pub use response::ChatResponse;
pub use role::Role;
pub use tool::{
    ToolAnnotations, ToolAttachment, ToolCall, ToolDefinition, ToolResult, TruncationMeta,
};
pub use usage::TokenUsage;
pub use webui_mapping::message_type;
