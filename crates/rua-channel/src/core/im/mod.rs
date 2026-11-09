use rua_core::RuntimeEvent;

const NON_IM_CHANNELS: &[&str] = &["console", "browser", "websocket"];

pub fn is_im_channel(channel_id: &str) -> bool {
    !NON_IM_CHANNELS.contains(&channel_id)
}

pub fn extract_tool_approval(event: &RuntimeEvent) -> Option<ApprovalPrompt> {
    if let RuntimeEvent::ToolApprovalPending {
        request_id,
        tool_name,
        ..
    } = event
    {
        Some(ApprovalPrompt {
            request_id: request_id.clone().unwrap_or_default(),
            tool_name: tool_name.to_string(),
        })
    } else {
        None
    }
}

pub struct ApprovalPrompt {
    pub request_id: String,
    pub tool_name: String,
}

impl ApprovalPrompt {
    pub fn render_text(&self) -> String {
        format!(
            "⚠️ 风险操作：即将执行 `{}`，回复 /approve 批准、/deny 拒绝",
            self.tool_name
        )
    }
}
