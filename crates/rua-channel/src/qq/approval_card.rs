use crate::core::approval_card::{ApprovalCardAction, ApprovalCardParseError};

pub const ACTION_DATA_PREFIX: &str = "tg_";

pub fn build_approval_message_body(
    request_id: &str,
    tool_name: &str,
    severity: &str,
    body_text: &str,
    session_ctx: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let severity = if severity.is_empty() {
        "medium"
    } else {
        severity
    }
    .to_ascii_lowercase();
    Ok(serde_json::json!({
        "markdown": {
            "content": if body_text.trim().is_empty() {
                "🛡️ Tool Approval Required"
            } else {
                body_text
            },
        },
        "keyboard": build_keyboard(request_id, tool_name, &severity, session_ctx)?,
        "msg_type": 2,
    }))
}

pub fn parse_interaction_event(
    event: &serde_json::Value,
) -> Result<QqApprovalAction, ApprovalCardParseError> {
    let data_str = event
        .pointer("/data/resolved/button_data")
        .or_else(|| event.get("button_data"))
        .and_then(|value| value.as_str())
        .ok_or(ApprovalCardParseError::Malformed)?;
    let ctx: serde_json::Value =
        serde_json::from_str(data_str).map_err(|_| ApprovalCardParseError::Malformed)?;
    if ctx.get("p").and_then(|value| value.as_str()) != Some(ACTION_DATA_PREFIX) {
        return Err(ApprovalCardParseError::UnknownAction);
    }
    let action = match ctx.get("a").and_then(|value| value.as_str()).unwrap_or("") {
        "approve" => ApprovalCardAction::Approve,
        "deny" => ApprovalCardAction::Deny,
        _ => return Err(ApprovalCardParseError::UnknownAction),
    };
    let request_id = ctx
        .get("rid")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ApprovalCardParseError::MissingRequestId)?
        .to_string();
    let session_ctx = ctx
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| !matches!(key.as_str(), "p" | "a" | "rid" | "tool" | "sev"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .map(serde_json::Value::Object)
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(QqApprovalAction {
        request_id,
        action,
        tool_name: ctx
            .get("tool")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        severity: ctx
            .get("sev")
            .and_then(|value| value.as_str())
            .unwrap_or("medium")
            .to_string(),
        session_ctx,
        operator_id: event
            .get("group_member_openid")
            .or_else(|| event.get("user_openid"))
            .or_else(|| event.pointer("/data/resolved/user_id"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        interaction_id: event
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

pub fn build_resolved_text(
    tool_name: &str,
    action: ApprovalCardAction,
    operator_display: &str,
) -> String {
    let by = if operator_display.is_empty() {
        String::new()
    } else {
        format!(" by **{operator_display}**")
    };
    match action {
        ApprovalCardAction::Approve => format!("✅ **Approved**{by}\nTool: `{tool_name}`"),
        ApprovalCardAction::Deny => format!("🚫 **Denied**{by}\nTool: `{tool_name}`"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QqApprovalAction {
    pub request_id: String,
    pub action: ApprovalCardAction,
    pub tool_name: String,
    pub severity: String,
    pub session_ctx: serde_json::Value,
    pub operator_id: String,
    pub interaction_id: String,
}

impl QqApprovalAction {
    pub fn command_text(&self) -> String {
        let action = match self.action {
            ApprovalCardAction::Approve => "approve",
            ApprovalCardAction::Deny => "deny",
        };
        format!("/approval {action} {}", self.request_id)
    }
}

fn build_keyboard(
    request_id: &str,
    tool_name: &str,
    severity: &str,
    session_ctx: serde_json::Value,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "content": {
            "rows": [
                {
                    "buttons": [
                        {
                            "id": format!("approve_{}", request_id.chars().take(8).collect::<String>()),
                            "render_data": {
                                "label": "✅ Approve",
                                "visited_label": "✅ Approved",
                                "style": 1,
                            },
                            "action": {
                                "type": 1,
                                "permission": { "type": 2 },
                                "data": action_data(ApprovalCardAction::Approve, request_id, tool_name, severity, session_ctx.clone())?,
                                "unsupport_tips": "Please use /approval approve",
                            },
                        },
                        {
                            "id": format!("deny_{}", request_id.chars().take(8).collect::<String>()),
                            "render_data": {
                                "label": "🚫 Deny",
                                "visited_label": "🚫 Denied",
                                "style": 0,
                            },
                            "action": {
                                "type": 1,
                                "permission": { "type": 2 },
                                "data": action_data(ApprovalCardAction::Deny, request_id, tool_name, severity, session_ctx)?,
                                "unsupport_tips": "Please use /approval deny",
                            },
                        },
                    ],
                },
            ],
        },
    }))
}

fn action_data(
    action: ApprovalCardAction,
    request_id: &str,
    tool_name: &str,
    severity: &str,
    session_ctx: serde_json::Value,
) -> Result<String, String> {
    let action = match action {
        ApprovalCardAction::Approve => "approve",
        ApprovalCardAction::Deny => "deny",
    };
    let mut payload = serde_json::Map::new();
    payload.insert(
        "p".into(),
        serde_json::Value::String(ACTION_DATA_PREFIX.into()),
    );
    payload.insert("a".into(), serde_json::Value::String(action.into()));
    payload.insert("rid".into(), serde_json::Value::String(request_id.into()));
    payload.insert("tool".into(), serde_json::Value::String(tool_name.into()));
    payload.insert("sev".into(), serde_json::Value::String(severity.into()));
    if let Some(ctx) = session_ctx.as_object() {
        for (key, value) in ctx {
            payload.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_string(&serde_json::Value::Object(payload))
        .map_err(|err| format!("serialize QQ approval action data: {err}"))
}
