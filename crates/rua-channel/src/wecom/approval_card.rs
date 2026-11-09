use crate::core::approval_card::{ApprovalCardAction, ApprovalCardParseError};

pub const TASK_ID_PREFIX: &str = "tg_approval_";
const RESOLVED_CARD_URL: &str = "https://qwenpaw.agentscope.io";

pub fn build_approval_card(
    request_id: &str,
    tool_name: &str,
    severity: &str,
    session_ctx: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let task_id = format!("{TASK_ID_PREFIX}{request_id}");
    Ok(serde_json::json!({
        "card_type": "button_interaction",
        "task_id": task_id,
        "main_title": {
            "title": "🛡️ Tool Approval Required",
            "desc": format!("{} | {}", tool_name, severity.to_ascii_lowercase()),
        },
        "button_list": [
            {
                "text": "Approve",
                "style": 1,
                "key": button_key(ApprovalCardAction::Approve, request_id, tool_name, severity, session_ctx.clone())?,
            },
            {
                "text": "Deny",
                "style": 2,
                "key": button_key(ApprovalCardAction::Deny, request_id, tool_name, severity, session_ctx)?,
            }
        ]
    }))
}

pub fn build_resolved_card(
    task_id: &str,
    tool_name: &str,
    action: ApprovalCardAction,
    decided_by: &str,
) -> serde_json::Value {
    let title = match action {
        ApprovalCardAction::Approve => "✅ Approved",
        ApprovalCardAction::Deny => "🚫 Denied",
    };
    let desc = if decided_by.is_empty() {
        tool_name.to_string()
    } else {
        format!("by {decided_by}\n{tool_name}")
    };
    serde_json::json!({
        "card_type": "text_notice",
        "task_id": task_id,
        "main_title": {
            "title": truncate(title, 36),
            "desc": truncate(&desc, 44),
        },
        "card_action": {
            "type": 1,
            "url": RESOLVED_CARD_URL,
        }
    })
}

pub fn parse_card_event(
    body: &serde_json::Value,
) -> Result<WecomApprovalAction, ApprovalCardParseError> {
    let event = body.get("event").unwrap_or(body);
    let tce = event.get("template_card_event").unwrap_or(event);
    let key = tce
        .get("event_key")
        .and_then(|value| value.as_str())
        .ok_or(ApprovalCardParseError::Malformed)?;
    let ctx: serde_json::Value =
        serde_json::from_str(key).map_err(|_| ApprovalCardParseError::Malformed)?;
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
                .filter(|(key, _)| !matches!(key.as_str(), "a" | "rid" | "tool" | "sev"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .map(serde_json::Value::Object)
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(WecomApprovalAction {
        request_id,
        action,
        task_id: tce
            .get("task_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
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
        user_id: body
            .get("from")
            .and_then(|value| value.get("userid"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct WecomApprovalAction {
    pub request_id: String,
    pub action: ApprovalCardAction,
    pub task_id: String,
    pub tool_name: String,
    pub severity: String,
    pub session_ctx: serde_json::Value,
    pub user_id: String,
}

impl WecomApprovalAction {
    pub fn command_text(&self) -> String {
        let action = match self.action {
            ApprovalCardAction::Approve => "approve",
            ApprovalCardAction::Deny => "deny",
        };
        format!("/approval {action} {}", self.request_id)
    }
}

fn button_key(
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
    payload.insert("a".into(), serde_json::Value::String(action.into()));
    payload.insert("rid".into(), serde_json::Value::String(request_id.into()));
    payload.insert("tool".into(), serde_json::Value::String(tool_name.into()));
    payload.insert(
        "sev".into(),
        serde_json::Value::String(
            if severity.is_empty() {
                "medium"
            } else {
                severity
            }
            .into(),
        ),
    );
    if let Some(ctx) = session_ctx.as_object() {
        for (key, value) in ctx {
            payload.insert(key.clone(), value.clone());
        }
    }
    let raw = serde_json::to_string(&serde_json::Value::Object(payload))
        .map_err(|err| format!("serialize button key: {err}"))?;
    if raw.len() > 1024 {
        return Err(format!("button key payload too large: {} bytes", raw.len()));
    }
    Ok(raw)
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
