use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, ToolCall as AcpToolCall, ToolCallContent, ToolCallId, ToolCallStatus,
    ToolCallUpdate,
};
use agent_types::content::ContentPart;
use agent_types::tool::{ToolCall, ToolResult};
use rua_core::event::RuntimeEvent;
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq)]
pub struct AcpToolSnapshot {
    pub index: usize,
    pub tool_call: AcpToolCall,
}

#[derive(Debug, Clone)]
pub struct AcpToolProjectionUpdate {
    snapshot: AcpToolSnapshot,
    emit_tool_call: bool,
    became_terminal: bool,
}

impl AcpToolProjectionUpdate {
    pub fn snapshot(&self) -> &AcpToolSnapshot {
        &self.snapshot
    }

    pub fn became_terminal(&self) -> bool {
        self.became_terminal
    }

    pub fn should_emit_tool_call(&self) -> bool {
        self.emit_tool_call
    }

    pub fn result_preview(&self) -> Option<String> {
        if !self.became_terminal {
            return None;
        }
        result_content(&self.snapshot.tool_call)
            .into_iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text),
                _ => None,
            })
            .reduce(|mut combined, text| {
                if !combined.is_empty() && !text.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&text);
                combined
            })
    }

    pub fn into_runtime_events(self) -> Vec<RuntimeEvent> {
        let mut events = Vec::with_capacity(2);
        if self.emit_tool_call {
            events.push(RuntimeEvent::ToolCall {
                call: projected_tool_call(&self.snapshot),
            });
        }
        if self.became_terminal {
            events.push(RuntimeEvent::ToolResult {
                result: projected_tool_result(&self.snapshot),
            });
        }
        events
    }
}

#[derive(Debug, Default)]
pub struct AcpToolProjection {
    calls: HashMap<ToolCallId, AcpToolSnapshot>,
    next_index: usize,
}

#[derive(Debug, Default)]
pub struct SharedAcpToolProjection {
    projection: Mutex<AcpToolProjection>,
    changed: Notify,
}

impl SharedAcpToolProjection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_tool_call(&self, tool_call: AcpToolCall) -> AcpToolProjectionUpdate {
        let update = self.projection.lock().unwrap().apply_tool_call(tool_call);
        self.changed.notify_waiters();
        update
    }

    pub fn apply_tool_call_update(
        &self,
        update: ToolCallUpdate,
    ) -> Option<AcpToolProjectionUpdate> {
        let projected = self
            .projection
            .lock()
            .unwrap()
            .apply_tool_call_update(update);
        if projected.is_some() {
            self.changed.notify_waiters();
        }
        projected
    }

    pub async fn wait_for_enriched_tool_call_update(
        &self,
        update: &ToolCallUpdate,
        timeout: Duration,
    ) -> Option<AcpToolCall> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            if let Some(tool_call) = self
                .projection
                .lock()
                .unwrap()
                .enrich_tool_call_update(update)
            {
                return Some(tool_call);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }
}

impl AcpToolProjection {
    pub fn apply_tool_call(&mut self, tool_call: AcpToolCall) -> AcpToolProjectionUpdate {
        let tool_call_id = tool_call.tool_call_id.clone();
        let previous = self.calls.get(&tool_call_id).cloned();
        let index = previous
            .as_ref()
            .map(|snapshot| snapshot.index)
            .unwrap_or_else(|| self.allocate_index());
        let became_terminal = is_terminal(tool_call.status)
            && previous
                .as_ref()
                .is_none_or(|snapshot| !is_terminal(snapshot.tool_call.status));
        let emit_tool_call = previous
            .as_ref()
            .is_none_or(|snapshot| tool_projection_fields_changed(&snapshot.tool_call, &tool_call))
            || became_terminal;
        let snapshot = AcpToolSnapshot { index, tool_call };
        self.calls.insert(tool_call_id, snapshot.clone());
        AcpToolProjectionUpdate {
            snapshot,
            emit_tool_call,
            became_terminal,
        }
    }

    pub fn apply_tool_call_update(
        &mut self,
        update: ToolCallUpdate,
    ) -> Option<AcpToolProjectionUpdate> {
        let tool_call_id = update.tool_call_id.clone();
        let previous = self.calls.get(&tool_call_id).cloned();
        let mut tool_call = match previous.as_ref() {
            Some(snapshot) => snapshot.tool_call.clone(),
            None => AcpToolCall::try_from(update.clone()).ok()?,
        };
        if previous.is_some() {
            tool_call.update(update.fields);
        }

        let index = previous
            .as_ref()
            .map(|snapshot| snapshot.index)
            .unwrap_or_else(|| self.allocate_index());
        let became_terminal = is_terminal(tool_call.status)
            && previous
                .as_ref()
                .is_none_or(|snapshot| !is_terminal(snapshot.tool_call.status));
        let emit_tool_call = previous
            .as_ref()
            .is_none_or(|snapshot| tool_projection_fields_changed(&snapshot.tool_call, &tool_call))
            || became_terminal;
        let snapshot = AcpToolSnapshot { index, tool_call };
        self.calls.insert(tool_call_id, snapshot.clone());
        Some(AcpToolProjectionUpdate {
            snapshot,
            emit_tool_call,
            became_terminal,
        })
    }

    pub fn get(&self, tool_call_id: &ToolCallId) -> Option<&AcpToolSnapshot> {
        self.calls.get(tool_call_id)
    }

    pub fn enrich_tool_call_update(&self, update: &ToolCallUpdate) -> Option<AcpToolCall> {
        match self.calls.get(&update.tool_call_id) {
            Some(snapshot) => {
                let mut tool_call = snapshot.tool_call.clone();
                tool_call.update(update.fields.clone());
                Some(tool_call)
            }
            None => AcpToolCall::try_from(update.clone()).ok(),
        }
    }

    fn allocate_index(&mut self) -> usize {
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        index
    }
}

fn is_terminal(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

fn tool_projection_fields_changed(previous: &AcpToolCall, current: &AcpToolCall) -> bool {
    previous.title != current.title
        || previous.kind != current.kind
        || previous.raw_input != current.raw_input
        || previous.locations != current.locations
}

fn projected_tool_call(snapshot: &AcpToolSnapshot) -> ToolCall {
    let acp_call = &snapshot.tool_call;
    let arguments = acp_call
        .raw_input
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut call = ToolCall::new(
        acp_call.tool_call_id.to_string(),
        acp_call.title.clone(),
        arguments.clone(),
    );
    call.raw_arguments = serde_json::to_string(&arguments).ok();
    call.provider_call_id = Some(acp_call.tool_call_id.to_string());
    call.metadata.insert(
        "acp_tool_index".to_string(),
        serde_json::json!(snapshot.index),
    );
    call.metadata.insert(
        "acp_kind".to_string(),
        serde_json::to_value(acp_call.kind).unwrap_or(serde_json::Value::Null),
    );
    call.metadata.insert(
        "acp_status".to_string(),
        serde_json::to_value(acp_call.status).unwrap_or(serde_json::Value::Null),
    );
    if !acp_call.locations.is_empty() {
        call.metadata.insert(
            "acp_locations".to_string(),
            serde_json::to_value(&acp_call.locations).unwrap_or(serde_json::Value::Null),
        );
    }
    call
}

fn projected_tool_result(snapshot: &AcpToolSnapshot) -> ToolResult {
    let acp_call = &snapshot.tool_call;
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "acp_tool_index".to_string(),
        serde_json::json!(snapshot.index),
    );
    metadata.insert(
        "acp_kind".to_string(),
        serde_json::to_value(acp_call.kind).unwrap_or(serde_json::Value::Null),
    );
    metadata.insert(
        "acp_status".to_string(),
        serde_json::to_value(acp_call.status).unwrap_or(serde_json::Value::Null),
    );
    if let Some(raw_output) = &acp_call.raw_output {
        metadata.insert("acp_raw_output".to_string(), raw_output.clone());
    }
    if !acp_call.locations.is_empty() {
        metadata.insert(
            "acp_locations".to_string(),
            serde_json::to_value(&acp_call.locations).unwrap_or(serde_json::Value::Null),
        );
    }

    ToolResult {
        call_id: acp_call.tool_call_id.to_string(),
        name: Some(acp_call.title.clone()),
        content: result_content(acp_call),
        is_error: matches!(acp_call.status, ToolCallStatus::Failed),
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata,
    }
}

fn result_content(tool_call: &AcpToolCall) -> Vec<ContentPart> {
    let mut content = extract_tool_call_content(&tool_call.content);
    if let Some(raw_output) = &tool_call.raw_output {
        let raw_text = display_json(raw_output);
        let already_present = content
            .iter()
            .any(|part| matches!(part, ContentPart::Text { text } if text == &raw_text));
        if !raw_text.is_empty() && !already_present {
            content.push(ContentPart::Text { text: raw_text });
        }
    }
    content
}

fn display_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn extract_tool_call_content(items: &[ToolCallContent]) -> Vec<ContentPart> {
    let mut out = Vec::new();
    for item in items {
        match item {
            ToolCallContent::Content(content) => {
                if let ContentBlock::Text(text) = &content.content {
                    out.push(ContentPart::Text {
                        text: text.text.clone(),
                    });
                } else {
                    out.push(ContentPart::Text {
                        text: "[non-text ACP content omitted]".to_string(),
                    });
                }
            }
            ToolCallContent::Diff(diff) => out.push(ContentPart::Text {
                text: format!("```diff\n--- {}\n{}```", diff.path.display(), diff.new_text),
            }),
            ToolCallContent::Terminal(terminal) => out.push(ContentPart::Text {
                text: format!("[acp terminal: {}]", terminal.terminal_id),
            }),
            _ => out.push(ContentPart::Text {
                text: "[unknown ACP content variant]".to_string(),
            }),
        }
    }
    out
}
