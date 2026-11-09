use crate::core::approval_card::{ApprovalCardAction, ApprovalCardParseError};

pub const TOOL_GUARD_ACTION_TYPE: &str = "tool_guard_approval";

pub fn build_approval_card(
    request_id: &str,
    tool_name: &str,
    severity: &str,
    body_text: &str,
    session_ctx: serde_json::Value,
) -> String {
    let approve_value = action_value(
        ApprovalCardAction::Approve,
        request_id,
        tool_name,
        severity,
        body_text,
        session_ctx.clone(),
    );
    let deny_value = action_value(
        ApprovalCardAction::Deny,
        request_id,
        tool_name,
        severity,
        body_text,
        session_ctx,
    );
    serde_json::json!({
        "config": { "wide_screen_mode": true },
        "header": {
            "template": severity_template(severity),
            "title": {
                "tag": "plain_text",
                "content": "🛡️ Tool Approval Required",
            },
        },
        "elements": [
            { "tag": "markdown", "content": truncate(body_text, 1800) },
            { "tag": "hr" },
            {
                "tag": "action",
                "actions": [
                    {
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": "✅ Approve" },
                        "type": "primary",
                        "value": approve_value,
                    },
                    {
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": "❌ Deny" },
                        "type": "danger",
                        "value": deny_value,
                    },
                ],
            },
        ],
    })
    .to_string()
}

pub fn build_resolved_card(
    tool_name: &str,
    action: ApprovalCardAction,
    decided_by: &str,
    body_text: &str,
) -> String {
    let (title, template, verb) = match action {
        ApprovalCardAction::Approve => ("✅ Approved", "green", "approved"),
        ApprovalCardAction::Deny => ("🚫 Denied", "red", "denied"),
    };
    let by = if decided_by.is_empty() {
        String::new()
    } else {
        format!(" by `{decided_by}`")
    };
    let mut elements = Vec::new();
    if !body_text.trim().is_empty() {
        elements
            .push(serde_json::json!({ "tag": "markdown", "content": truncate(body_text, 1800) }));
        elements.push(serde_json::json!({ "tag": "hr" }));
    }
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": format!("Tool `{tool_name}` {verb}{by}."),
    }));

    serde_json::json!({
        "config": { "wide_screen_mode": true },
        "header": {
            "template": template,
            "title": { "tag": "plain_text", "content": title },
        },
        "elements": elements,
    })
    .to_string()
}

pub fn build_toast(action: ApprovalCardAction, tool_name: &str) -> serde_json::Value {
    match action {
        ApprovalCardAction::Approve => {
            serde_json::json!({ "type": "success", "content": format!("Approved tool {tool_name}") })
        }
        ApprovalCardAction::Deny => {
            serde_json::json!({ "type": "info", "content": format!("Denied tool {tool_name}") })
        }
    }
}

pub fn build_callback_response(
    action: &FeishuApprovalAction,
    decided_by: &str,
) -> Result<serde_json::Value, serde_json::Error> {
    let card = serde_json::from_str::<serde_json::Value>(&build_resolved_card(
        &action.tool_name,
        action.action.clone(),
        decided_by,
        &action.body,
    ))?;
    Ok(serde_json::json!({
        "toast": build_toast(action.action.clone(), &action.tool_name),
        "card": {
            "type": "raw",
            "data": card,
        },
    }))
}

pub fn parse_action_value(
    value: &serde_json::Value,
) -> Result<FeishuApprovalAction, ApprovalCardParseError> {
    if value.get("type").and_then(|v| v.as_str()) != Some(TOOL_GUARD_ACTION_TYPE) {
        return Err(ApprovalCardParseError::UnknownAction);
    }
    let request_id = value
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(ApprovalCardParseError::MissingRequestId)?
        .to_string();
    let action = match value.get("action").and_then(|v| v.as_str()).unwrap_or("") {
        "approve" => ApprovalCardAction::Approve,
        "deny" => ApprovalCardAction::Deny,
        _ => return Err(ApprovalCardParseError::UnknownAction),
    };
    Ok(FeishuApprovalAction {
        request_id,
        action,
        tool_name: value
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        body: value
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        session_ctx: value
            .get("session_ctx")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeishuApprovalAction {
    pub request_id: String,
    pub action: ApprovalCardAction,
    pub tool_name: String,
    pub body: String,
    pub session_ctx: serde_json::Value,
}

impl FeishuApprovalAction {
    pub fn command_text(&self) -> String {
        let action = match self.action {
            ApprovalCardAction::Approve => "approve",
            ApprovalCardAction::Deny => "deny",
        };
        format!("/approval {action} {}", self.request_id)
    }
}

fn action_value(
    action: ApprovalCardAction,
    request_id: &str,
    tool_name: &str,
    severity: &str,
    body_text: &str,
    session_ctx: serde_json::Value,
) -> serde_json::Value {
    let action = match action {
        ApprovalCardAction::Approve => "approve",
        ApprovalCardAction::Deny => "deny",
    };
    serde_json::json!({
        "type": TOOL_GUARD_ACTION_TYPE,
        "action": action,
        "request_id": request_id,
        "tool_name": tool_name,
        "severity": if severity.is_empty() { "medium" } else { severity },
        "body": truncate(body_text, 1500),
        "session_ctx": session_ctx,
    })
}

fn severity_template(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" | "high" => "red",
        "low" => "yellow",
        _ => "orange",
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}
