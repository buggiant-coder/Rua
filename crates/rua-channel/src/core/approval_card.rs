use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalCardAction {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalCardMetadata {
    pub request_id: String,
    pub tool_name: String,
    pub prompt: String,
    pub agent_id: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCardCallback {
    pub request_id: String,
    pub action: ApprovalCardAction,
    pub decided_by: String,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCardParseError {
    MissingRequestId,
    UnknownAction,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCardResolveError {
    MissingRequestId,
    Duplicate,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCardResolution {
    pub request_id: String,
    pub action: ApprovalCardAction,
    pub decided_by: String,
}

impl ApprovalCardResolution {
    pub fn command_text(&self) -> String {
        let action = match self.action {
            ApprovalCardAction::Approve => "approve",
            ApprovalCardAction::Deny => "deny",
        };
        format!("/approval {action} {}", self.request_id)
    }

    pub fn authority_decision_kind(&self) -> &'static str {
        match self.action {
            ApprovalCardAction::Approve => "run_once",
            ApprovalCardAction::Deny => "reject",
        }
    }
}

impl ApprovalCardMetadata {
    pub fn new(
        request_id: impl Into<String>,
        tool_name: impl Into<String>,
        prompt: impl Into<String>,
        agent_id: impl Into<String>,
        chat_id: impl Into<String>,
    ) -> Result<Self, ApprovalCardParseError> {
        let request_id = request_id.into();
        if request_id.trim().is_empty() {
            return Err(ApprovalCardParseError::MissingRequestId);
        }
        Ok(Self {
            request_id,
            tool_name: tool_name.into(),
            prompt: prompt.into(),
            agent_id: agent_id.into(),
            chat_id: chat_id.into(),
        })
    }

    pub fn fallback_text(&self) -> String {
        format!(
            "{}\n\nTool: {}\nRequest: {}\n\nReply with /approve or /deny.",
            self.prompt, self.tool_name, self.request_id
        )
    }
}

pub fn build_callback_data(action: ApprovalCardAction, request_id: &str) -> String {
    match action {
        ApprovalCardAction::Approve => format!("tga:{request_id}"),
        ApprovalCardAction::Deny => format!("tgd:{request_id}"),
    }
}

pub fn parse_callback_data(
    platform: impl Into<String>,
    data: &str,
    decided_by: impl Into<String>,
) -> Result<ApprovalCardCallback, ApprovalCardParseError> {
    let (action, request_id) = if let Some(id) = data.strip_prefix("tga:") {
        (ApprovalCardAction::Approve, id)
    } else if let Some(id) = data.strip_prefix("tgd:") {
        (ApprovalCardAction::Deny, id)
    } else if let Some(rest) = data.strip_prefix("approval:") {
        let (raw_action, id) = rest
            .split_once(':')
            .ok_or(ApprovalCardParseError::Malformed)?;
        let action = match raw_action {
            "approve" => ApprovalCardAction::Approve,
            "deny" => ApprovalCardAction::Deny,
            _ => return Err(ApprovalCardParseError::UnknownAction),
        };
        (action, id)
    } else {
        return Err(ApprovalCardParseError::UnknownAction);
    };
    if request_id.trim().is_empty() {
        return Err(ApprovalCardParseError::MissingRequestId);
    }
    Ok(ApprovalCardCallback {
        request_id: request_id.to_string(),
        action,
        decided_by: decided_by.into(),
        platform: platform.into(),
    })
}

pub struct ApprovalCardResolver {
    ttl: Duration,
    seen: HashMap<String, SystemTime>,
    now: Box<dyn Fn() -> SystemTime + Send + Sync>,
}

impl ApprovalCardResolver {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: HashMap::new(),
            now: Box::new(SystemTime::now),
        }
    }

    pub fn with_now(ttl: Duration, now: Box<dyn Fn() -> SystemTime + Send + Sync>) -> Self {
        Self {
            ttl,
            seen: HashMap::new(),
            now,
        }
    }

    pub fn resolve(
        &mut self,
        callback: ApprovalCardCallback,
    ) -> Result<ApprovalCardResolution, ApprovalCardResolveError> {
        if callback.request_id.trim().is_empty() {
            return Err(ApprovalCardResolveError::MissingRequestId);
        }
        let now = (self.now)();
        self.seen
            .retain(|_, at| now.duration_since(*at).unwrap_or_default() <= self.ttl);
        if self.seen.contains_key(&callback.request_id) {
            return Err(ApprovalCardResolveError::Duplicate);
        }
        self.seen.insert(callback.request_id.clone(), now);
        Ok(ApprovalCardResolution {
            request_id: callback.request_id,
            action: callback.action,
            decided_by: callback.decided_by,
        })
    }

    pub fn reject_stale(&self, created_at: SystemTime) -> Result<(), ApprovalCardResolveError> {
        let now = (self.now)();
        if now.duration_since(created_at).unwrap_or_default() > self.ttl {
            Err(ApprovalCardResolveError::Stale)
        } else {
            Ok(())
        }
    }
}
