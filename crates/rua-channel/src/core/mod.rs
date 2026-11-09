#![forbid(unsafe_code)]

pub mod access_token;
pub mod acl_store;
pub mod allowlist;
pub mod approval_card;
pub mod channel;
pub mod command_registry;
pub mod config;
pub mod console;
pub mod content;
pub mod control_handlers;
pub mod debouncer;
pub mod event_dedup;
pub mod health;
pub mod im;
pub mod manager;
pub mod media;
pub mod mention;
pub mod pipeline;
pub mod queue;
pub mod renderer;
pub mod sse;
pub mod streaming;

pub use access_token::{AccessTokenCache, FetchTokenFn};
pub use command_registry::CommandRegistry;
pub use config::{BaseChannelConfig, ConsoleChannelConfig};
pub use content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, IncomingMessage, OutgoingContentPart,
    OutgoingFile, OutgoingMessage, ToolStatus,
};
pub use control_handlers::{
    ApprovalService, ControlContext, ControlResult, SessionCanceller, WorkspaceReloader,
};
pub use health::{ChannelHealth, ChannelLifecycle, ChannelStatus};
pub use manager::{
    extract_query_text, extract_query_text_from_request, ChannelManager, RoutingProcess,
};
pub use queue::{QueueConsumer, QueueKey, UnifiedQueueManager};
pub use renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
pub use sse::SseFrame;

pub use acl_store::{acl_message, AccessControlStore};
pub use allowlist::{AllowlistMode, AllowlistPolicy, ChannelAcl};
pub use approval_card::{
    build_callback_data, parse_callback_data, ApprovalCardAction, ApprovalCardCallback,
    ApprovalCardMetadata, ApprovalCardParseError, ApprovalCardResolution, ApprovalCardResolveError,
    ApprovalCardResolver,
};
pub use channel::{AgentRequest, Channel, OutMessage, Process, ToHandle};
pub use debouncer::{content_allows_immediate, BufferDebouncer};
pub use event_dedup::{EventDedupDecision, EventDeduper};
pub use media::{resolve_outgoing_media, ResolvedMedia};
pub use mention::should_respond;
pub use pipeline::{
    run_inbound, run_inbound_acl, run_inbound_acl_with_indicator,
    run_inbound_streaming_with_indicator, SendFn,
};
pub use streaming::{collect_reply, ProcessingIndicator};

pub type ChannelId = String;

pub type ConversationId = String;

pub type SenderId = String;

pub type MessageId = String;

pub type AgentId = String;
