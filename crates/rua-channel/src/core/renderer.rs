use crate::core::content::{OutgoingContentPart, ToolStatus};
use crate::core::sse::SseFrame;
use rua_core::RuntimeEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderStyle {
    Markdown,

    PlainText,

    Structured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    DingTalk,

    Weixin,

    WeCom,

    XiaoYi,

    Console,

    Chat,

    Discord,

    Telegram,

    Mattermost,
}

impl ChannelKind {
    pub fn is_im(&self) -> bool {
        matches!(
            self,
            Self::DingTalk
                | Self::Weixin
                | Self::WeCom
                | Self::XiaoYi
                | Self::Discord
                | Self::Telegram
                | Self::Mattermost
        )
    }
}

#[derive(Debug, Clone)]
pub struct MessageRenderOptions {
    pub show_thinking: bool,

    pub show_tool_details: bool,

    pub markdown: bool,

    pub max_length: Option<usize>,
    pub show_tool_calls: bool,
    pub show_tool_results: bool,
    pub tool_call_max_length: usize,
    pub tool_result_max_length: usize,
}

impl Default for MessageRenderOptions {
    fn default() -> Self {
        Self {
            show_thinking: true,
            show_tool_details: true,
            markdown: true,
            max_length: None,
            show_tool_calls: true,
            show_tool_results: true,
            tool_call_max_length: 0,
            tool_result_max_length: 0,
        }
    }
}

impl MessageRenderOptions {
    pub fn from_config(config: &crate::core::config::BaseChannelConfig) -> Self {
        let mut options = Self {
            show_thinking: config.show_thinking,
            show_tool_details: config.show_tool_details,
            markdown: true,
            max_length: None,
            show_tool_calls: config.show_tool_calls,
            show_tool_results: config.show_tool_results,
            tool_call_max_length: 0,
            tool_result_max_length: 0,
        };
        options.apply_metadata(&serde_json::Value::Object(config.extra.clone()));
        options
    }

    /// Apply explicit request-level overrides independently.
    pub fn apply_metadata(&mut self, metadata: &serde_json::Value) {
        if let Some(show) = metadata["show_thinking"].as_bool() {
            self.show_thinking = show;
        }
        if let Some(show) = metadata["show_tool_calls"].as_bool() {
            self.show_tool_calls = show;
        }
        if let Some(show) = metadata["show_tool_results"].as_bool() {
            self.show_tool_results = show;
        }
        if let Some(limit) = metadata["tool_call_max_length"].as_u64() {
            self.tool_call_max_length = usize::try_from(limit).unwrap_or(usize::MAX);
        }
        if let Some(limit) = metadata["tool_result_max_length"].as_u64() {
            self.tool_result_max_length = usize::try_from(limit).unwrap_or(usize::MAX);
        }
    }

    pub fn for_channel_kind(kind: ChannelKind) -> Self {
        if kind.is_im() {
            Self {
                show_thinking: false,
                show_tool_details: false,
                markdown: true,
                max_length: None,
                show_tool_calls: false,
                show_tool_results: false,
                tool_call_max_length: 0,
                tool_result_max_length: 0,
            }
        } else {
            Self::default()
        }
    }
}

pub struct MessageRenderer {
    pub style: RenderStyle,

    pub options: MessageRenderOptions,
}

impl MessageRenderer {
    pub fn new(style: RenderStyle) -> Self {
        Self {
            style,
            options: MessageRenderOptions::default(),
        }
    }

    pub fn with_options(style: RenderStyle, options: MessageRenderOptions) -> Self {
        Self { style, options }
    }

    pub fn from_config(
        style: RenderStyle,
        config: &crate::core::config::BaseChannelConfig,
    ) -> Self {
        Self {
            style,
            options: MessageRenderOptions::from_config(config),
        }
    }

    pub fn render_event(&self, event: &RuntimeEvent) -> Vec<OutgoingContentPart> {
        match event {
            RuntimeEvent::TextDelta { delta, .. } => {
                let text = if let Some(max_len) = self.options.max_length {
                    if delta.len() > max_len {
                        format!("{}...", &delta[..max_len])
                    } else {
                        delta.clone()
                    }
                } else {
                    delta.clone()
                };
                match self.style {
                    RenderStyle::Markdown => vec![OutgoingContentPart::Markdown { text }],
                    RenderStyle::PlainText | RenderStyle::Structured => {
                        vec![OutgoingContentPart::Text { text }]
                    }
                }
            }

            RuntimeEvent::ThinkingDelta { delta, .. } => {
                if !self.options.show_thinking {
                    return vec![];
                }
                vec![OutgoingContentPart::Thinking {
                    text: delta.clone(),
                }]
            }

            RuntimeEvent::ToolCall { call } => {
                if !self.options.show_tool_calls {
                    return vec![];
                }
                if self.options.show_tool_details {
                    vec![OutgoingContentPart::Tool {
                        name: call.name.clone(),
                        status: ToolStatus::Running,
                        input: Some(truncate_detail(
                            serde_json::to_value(&call.arguments).unwrap_or_default(),
                            self.options.tool_call_max_length,
                        )),
                        output: None,
                    }]
                } else {
                    vec![OutgoingContentPart::Tool {
                        name: call.name.clone(),
                        status: ToolStatus::Running,
                        input: None,
                        output: None,
                    }]
                }
            }

            RuntimeEvent::ToolResult { result } => {
                if !self.options.show_tool_results {
                    return vec![];
                }
                let status = if result.is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                };
                let name = result.call_id.as_str().to_string();
                if self.options.show_tool_details {
                    vec![OutgoingContentPart::Tool {
                        name,
                        status,
                        input: None,
                        output: Some(truncate_detail(
                            serde_json::Value::String(result.text_content().to_string()),
                            self.options.tool_result_max_length,
                        )),
                    }]
                } else {
                    vec![OutgoingContentPart::Tool {
                        name,
                        status,
                        input: None,
                        output: None,
                    }]
                }
            }

            RuntimeEvent::Error { error } => {
                vec![OutgoingContentPart::Text {
                    text: error.to_string(),
                }]
            }

            RuntimeEvent::Start { .. }
            | RuntimeEvent::ModelCallReset { .. }
            | RuntimeEvent::ToolCallDelta { .. }
            | RuntimeEvent::ToolResultDelta { .. }
            | RuntimeEvent::End { .. }
            | RuntimeEvent::Usage { .. }
            | RuntimeEvent::ContextCompaction { .. }
            | RuntimeEvent::ToolApprovalPending { .. }
            | RuntimeEvent::IterationStart { .. }
            | RuntimeEvent::IterationEnd { .. }
            | RuntimeEvent::AskUser { .. }
            | RuntimeEvent::ToolApprovalRequest { .. } => {
                vec![]
            }
        }
    }
}

fn truncate_detail(value: serde_json::Value, limit: usize) -> serde_json::Value {
    if limit == 0 {
        return value;
    }
    let text = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    if text.chars().count() <= limit {
        value
    } else {
        serde_json::Value::String(format!("{}…", text.chars().take(limit).collect::<String>()))
    }
}

pub fn should_send_sse_frame(event: &RuntimeEvent, opts: &MessageRenderOptions) -> bool {
    match event {
        RuntimeEvent::ThinkingDelta { .. } => opts.show_thinking,
        RuntimeEvent::ToolCall { .. } | RuntimeEvent::ToolCallDelta { .. } => opts.show_tool_calls,
        RuntimeEvent::ToolResultDelta { .. } | RuntimeEvent::ToolResult { .. } => {
            opts.show_tool_results
        }
        _ => true,
    }
}

pub fn filter_tool_details_in_frame(frame: &mut SseFrame, show_details: bool) {
    if show_details {
        return;
    }
    if let Some(event_type) = frame.data.get("type").and_then(|v| v.as_str()) {
        match event_type {
            "tool_call" => {
                if let Some(call) = frame.data.get_mut("call") {
                    if let Some(obj) = call.as_object_mut() {
                        obj.remove("arguments");
                        obj.remove("raw_arguments");
                    }
                }
            }
            "tool_result" => {
                if let Some(result) = frame.data.get_mut("result") {
                    if let Some(obj) = result.as_object_mut() {
                        obj.remove("content");
                    }
                }
            }
            _ => {}
        }
    }
}
