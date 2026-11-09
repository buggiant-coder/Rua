use agent_types::{AgentEvent, ToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagParseMode {
    #[default]
    Lenient,

    Strict,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagSegment {
    Text(String),

    Thinking(String),

    ToolCall {
        name: String,
        arguments: serde_json::Value,
    },

    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Text,
    InsideThink,
    InsideToolCall,

    PendingTag,
}

#[derive(Debug, Clone)]
pub struct TagParser {
    state: State,
    buffer: String,
    tag_buffer: String,
    segments: Vec<TagSegment>,
    mode: TagParseMode,
    suppress_leading_whitespace: bool,
    consumed_structured_tag: bool,
}

impl Default for TagParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TagParser {
    pub fn new() -> Self {
        Self::with_mode(TagParseMode::default())
    }

    pub fn with_mode(mode: TagParseMode) -> Self {
        Self {
            state: State::Text,
            buffer: String::new(),
            tag_buffer: String::new(),
            segments: Vec::new(),
            mode,
            suppress_leading_whitespace: false,
            consumed_structured_tag: false,
        }
    }

    pub fn push(&mut self, text: &str) {
        for ch in text.chars() {
            match self.state {
                State::Text => {
                    if self.suppress_leading_whitespace {
                        if ch.is_whitespace() {
                            continue;
                        }
                        self.suppress_leading_whitespace = false;
                    }
                    if ch == '<' {
                        self.state = State::PendingTag;
                        self.tag_buffer.clear();
                        self.tag_buffer.push(ch);
                    } else {
                        self.buffer.push(ch);
                    }
                }
                State::PendingTag => {
                    self.tag_buffer.push(ch);
                    if ch == '>' {
                        let tag = &self.tag_buffer;
                        if tag.eq_ignore_ascii_case("<think>") {
                            self.consumed_structured_tag = true;
                            self.flush_text();
                            self.state = State::InsideThink;
                        } else if tag.eq_ignore_ascii_case("<tool_call>") {
                            self.consumed_structured_tag = true;
                            self.flush_text();
                            self.state = State::InsideToolCall;
                        } else {
                            self.buffer.push_str(&self.tag_buffer);
                            self.state = State::Text;
                        }
                        self.tag_buffer.clear();
                    } else if self.tag_buffer.len() > 20 {
                        self.buffer.push_str(&self.tag_buffer);
                        self.tag_buffer.clear();
                        self.state = State::Text;
                    }
                }
                State::InsideThink => {
                    self.buffer.push(ch);
                    if self.buffer.ends_with("</think>") {
                        let content_len = self.buffer.len() - "</think>".len();
                        let content = self.buffer[..content_len].trim().to_string();
                        self.buffer.clear();
                        if !content.is_empty() {
                            self.segments.push(TagSegment::Thinking(content));
                        }
                        self.state = State::Text;
                    }
                }
                State::InsideToolCall => {
                    self.buffer.push(ch);
                    if self.buffer.ends_with("</tool_call>") {
                        let content_len = self.buffer.len() - "</tool_call>".len();
                        let content = self.buffer[..content_len].to_string();
                        self.buffer.clear();
                        self.parse_tool_call(&content);
                        self.state = State::Text;
                    }
                }
            }
        }
    }

    pub fn drain(&mut self) -> Vec<TagSegment> {
        if self.state == State::Text {
            self.flush_text();
        }
        std::mem::take(&mut self.segments)
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
    }

    pub fn is_buffering_structured_segment(&self) -> bool {
        matches!(
            self.state,
            State::PendingTag | State::InsideThink | State::InsideToolCall
        )
    }

    pub fn take_consumed_structured_tag(&mut self) -> bool {
        let consumed = self.consumed_structured_tag;
        self.consumed_structured_tag = false;
        consumed
    }

    pub fn finish(mut self) -> Vec<TagSegment> {
        match self.state {
            State::Text => {
                self.flush_text();
            }
            State::PendingTag => {
                self.buffer.push_str(&self.tag_buffer);
                self.flush_text();
            }
            State::InsideThink => {
                let content = self.buffer.trim().to_string();
                if !content.is_empty() {
                    self.segments.push(TagSegment::Text(content));
                }
            }
            State::InsideToolCall => {
                let content = std::mem::take(&mut self.buffer);
                if let Some((name, arguments)) = extract_tool_call(content.trim()) {
                    self.segments.push(TagSegment::ToolCall { name, arguments });
                }
            }
        }
        self.segments
    }

    pub fn segments_to_events(segments: &[TagSegment], tc_offset: u32) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let mut tc_counter = tc_offset;

        for seg in segments {
            match seg {
                TagSegment::Text(text) => {
                    if !text.is_empty() {
                        events.push(AgentEvent::TextDelta {
                            delta: text.clone(),
                            metadata: serde_json::Value::Null,
                        });
                    }
                }
                TagSegment::Thinking(content) => {
                    events.push(AgentEvent::ThinkingDelta {
                        delta: content.clone(),
                        signature: None,
                        metadata: serde_json::Value::Null,
                    });
                }
                TagSegment::ToolCall { name, arguments } => {
                    tc_counter += 1;
                    events.push(AgentEvent::ToolCall {
                        call: ToolCall {
                            id: format!("local-tc-{tc_counter}"),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            raw_arguments: None,
                            provider_call_id: None,
                            metadata: serde_json::Map::new(),
                        },
                    });
                }
                TagSegment::Error(msg) => {
                    events.push(AgentEvent::Error {
                        code: None,
                        message: msg.clone(),
                        retryable: false,
                        metadata: serde_json::Value::Null,
                    });
                }
            }
        }
        events
    }

    fn flush_text(&mut self) {
        if !self.buffer.is_empty() {
            self.segments
                .push(TagSegment::Text(std::mem::take(&mut self.buffer)));
        }
    }

    fn parse_tool_call(&mut self, content: &str) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some((name, arguments)) = extract_tool_call(trimmed) {
            self.segments.push(TagSegment::ToolCall { name, arguments });
            return;
        }
        match self.mode {
            TagParseMode::Lenient => {
                self.segments.push(TagSegment::Text(format!(
                    "<tool_call>{content}</tool_call>"
                )));
            }
            TagParseMode::Strict => {
                self.segments.push(TagSegment::Error(format!(
                    "malformed tool_call content: {}",
                    &trimmed[..trimmed.len().min(200)]
                )));
            }
        }
    }
}

pub fn extract_tool_call(content: &str) -> Option<(String, serde_json::Value)> {
    let trimmed = content.trim();
    extract_tool_call_json(trimmed)
        .or_else(|| extract_tool_call_strict_xml(trimmed))
        .or_else(|| extract_tool_call_lenient_xml(trimmed))
}

fn extract_tool_call_json(content: &str) -> Option<(String, serde_json::Value)> {
    let val: serde_json::Value = serde_json::from_str(content).ok()?;
    let name = val.get("name").and_then(|n| n.as_str())?;
    if name.is_empty() {
        return None;
    }
    let arguments = val
        .get("arguments")
        .or_else(|| val.get("parameters"))
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let arguments = match arguments {
        serde_json::Value::String(s) => serde_json::from_str(&s).ok()?,
        other => other,
    };
    Some((name.to_string(), arguments))
}

fn extract_tool_call_strict_xml(content: &str) -> Option<(String, serde_json::Value)> {
    let func_start = content.find("<function=")?;
    let after_eq = func_start + "<function=".len();
    let gt_pos = content[after_eq..].find('>')? + after_eq;
    let name = content[after_eq..gt_pos].trim();
    if name.is_empty() {
        return None;
    }
    let name = name.to_string();
    let body_start = gt_pos + 1;

    let func_close_rel = content[body_start..].find("</function>")?;
    let body = &content[body_start..body_start + func_close_rel];

    let mut args = serde_json::Map::new();
    let mut pos = 0;
    while let Some(param_rel) = body[pos..].find("<parameter=") {
        let abs = pos + param_rel;
        let key_start = abs + "<parameter=".len();
        let Some(key_end_rel) = body[key_start..].find('>') else {
            break;
        };
        let key_end = key_start + key_end_rel;
        let key = body[key_start..key_end].trim();

        let val_start = key_end + 1;
        let Some(close_rel) = body[val_start..].find("</parameter>") else {
            break;
        };
        let val_end = val_start + close_rel;
        let value = body[val_start..val_end].trim();

        if !key.is_empty() {
            args.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        pos = val_end + "</parameter>".len();
    }

    if args.is_empty() && body.contains("<parameter=") {
        return None;
    }

    Some((name, serde_json::Value::Object(args)))
}

fn extract_tool_call_lenient_xml(content: &str) -> Option<(String, serde_json::Value)> {
    let func_start = content.find("<function=")?;
    let after_eq = func_start + "<function=".len();
    let gt_pos = content[after_eq..].find('>')? + after_eq;
    let name = content[after_eq..gt_pos].trim();
    if name.is_empty() {
        return None;
    }
    let name = name.to_string();
    let body_start = gt_pos + 1;

    let body_end = [
        content[body_start..].find("<function="),
        content[body_start..].find("</function>"),
    ]
    .iter()
    .filter_map(|x| *x)
    .min()
    .map(|i| body_start + i)
    .unwrap_or(content.len());
    let body = &content[body_start..body_end];

    let mut args = serde_json::Map::new();
    let mut pos = 0;
    while let Some(param_rel) = body[pos..].find("<parameter=") {
        let abs = pos + param_rel;
        let key_start = abs + "<parameter=".len();
        let Some(key_end_rel) = body[key_start..].find('>') else {
            break;
        };
        let key_end = key_start + key_end_rel;
        let key = body[key_start..key_end].trim();

        let val_start = key_end + 1;
        let val_end = [
            body[val_start..].find("<parameter="),
            body[val_start..].find("</parameter>"),
            body[val_start..].find("<function="),
            body[val_start..].find("</function>"),
        ]
        .iter()
        .filter_map(|x| *x)
        .min()
        .map(|i| val_start + i)
        .unwrap_or(body.len());
        let value = body[val_start..val_end].trim();

        if !key.is_empty() {
            args.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        if body[val_end..].starts_with("</parameter>") {
            pos = val_end + "</parameter>".len();
        } else {
            pos = val_end;
        }
    }

    if args.is_empty() {
        return None;
    }

    Some((name, serde_json::Value::Object(args)))
}

pub fn extract_thinking_from_text(text: &str) -> (String, String) {
    let mut parser = TagParser::new();
    parser.push(text);
    let segments = parser.finish();

    let mut thinking = String::new();
    let mut plain = String::new();

    for seg in segments {
        match seg {
            TagSegment::Thinking(t) => thinking.push_str(&t),
            TagSegment::Text(t) => plain.push_str(&t),
            TagSegment::ToolCall { .. } | TagSegment::Error(_) => {}
        }
    }

    (thinking, plain)
}
