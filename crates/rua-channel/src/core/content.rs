use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::{AgentId, ChannelId, ConversationId, MessageId, SenderId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncomingContentPart {
    Text {
        text: String,
    },

    Image {
        file_id: String,
        path: PathBuf,
        mime: Option<String>,
    },

    File {
        file_id: String,
        path: PathBuf,
        mime: Option<String>,
        name: Option<String>,
    },

    Audio {
        file_id: String,
        path: PathBuf,
        mime: Option<String>,
        name: Option<String>,
    },

    Video {
        file_id: String,
        path: PathBuf,
        mime: Option<String>,
        name: Option<String>,
    },

    Url {
        url: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub channel_id: ChannelId,

    pub conversation_id: ConversationId,

    pub sender_id: SenderId,

    pub message_id: MessageId,

    pub agent_id: AgentId,

    pub content: Vec<IncomingContentPart>,

    #[serde(default)]
    pub raw: serde_json::Value,

    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,

    Success,

    Error,

    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutgoingContentPart {
    Text {
        text: String,
    },

    Markdown {
        text: String,
    },

    Image {
        url: Option<String>,
        path: Option<PathBuf>,
        alt: Option<String>,
    },

    File {
        url: Option<String>,
        path: Option<PathBuf>,
        name: Option<String>,
    },

    Audio {
        url: Option<String>,
        path: Option<PathBuf>,
        name: Option<String>,
    },

    Video {
        url: Option<String>,
        path: Option<PathBuf>,
        name: Option<String>,
    },

    Tool {
        name: String,
        status: ToolStatus,
        input: Option<serde_json::Value>,
        output: Option<serde_json::Value>,
    },

    Thinking {
        text: String,
    },

    Refusal {
        text: String,
    },

    Raw {
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub channel_id: ChannelId,

    pub conversation_id: ConversationId,

    pub receiver_id: Option<SenderId>,

    pub content: Vec<OutgoingContentPart>,

    pub reply_to: Option<MessageId>,

    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingFile {
    pub path: PathBuf,

    pub name: String,

    pub mime: Option<String>,

    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelTarget {
    pub agent_id: AgentId,

    pub conversation_id: ConversationId,

    pub sender_id: Option<SenderId>,

    pub reply_to: Option<MessageId>,

    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChannelRequest {
    pub agent_id: AgentId,

    pub user_id: String,

    pub session_id: String,

    pub input: Vec<IncomingContentPart>,

    pub channel_id: ChannelId,

    pub conversation_id: ConversationId,

    pub message_id: Option<MessageId>,

    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub fn split_text(text: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 || text.len() <= max_len {
        return vec![text.to_string()];
    }
    if has_markdown_structure(text) {
        return split_markdown_aware(text, max_len);
    }
    split_plain(text, max_len)
}

fn split_plain(text: &str, max_len: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            result.push(remaining.to_string());
            break;
        }

        let boundary = utf8_safe_boundary(remaining, max_len);

        let split_at = if let Some(pos) = remaining[..boundary].rfind('\n') {
            pos + 1
        } else if let Some(pos) = remaining[..boundary].rfind(' ') {
            pos + 1
        } else {
            boundary
        };

        if split_at == 0 {
            let mut end = 1;
            while end < remaining.len() && !remaining.is_char_boundary(end) {
                end += 1;
            }
            result.push(remaining[..end].to_string());
            remaining = &remaining[end..];
        } else {
            result.push(remaining[..split_at].to_string());
            remaining = &remaining[split_at..];
        }
    }

    result
}

fn utf8_safe_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    let mut i = max;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn table_separator_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*\|?\s*:?-{3,}:?(\s*\|\s*:?-{3,}:?)*\s*\|?\s*$")
            .expect("table separator regex is valid")
    })
}

fn is_fence(stripped: &str) -> bool {
    stripped.starts_with("```") || stripped.starts_with("~~~")
}

fn is_table_separator(line: &str) -> bool {
    line.contains('-') && table_separator_re().is_match(line)
}

fn fence_marker(fence_open: &str) -> String {
    let marker_char = fence_open.chars().next().unwrap_or('`');
    fence_open
        .chars()
        .take_while(|&c| c == marker_char)
        .collect()
}

fn has_markdown_structure(text: &str) -> bool {
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        if is_fence(line.trim()) {
            return true;
        }
        if line.contains('|') && i + 1 < lines.len() && is_table_separator(lines[i + 1]) {
            return true;
        }
    }
    false
}

fn split_markdown_aware(text: &str, max_len: usize) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut buf = SplitBuffer::new(max_len);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let stripped = line.trim();

        if is_fence(stripped) {
            buf.fence_open = if buf.fence_open.is_empty() {
                stripped.to_string()
            } else {
                String::new()
            };
            buf.emit_line(line);
            i += 1;
            continue;
        }

        let is_table_start = buf.fence_open.is_empty()
            && line.contains('|')
            && i + 1 < lines.len()
            && is_table_separator(lines[i + 1]);
        if is_table_start {
            let (table_lines, next_i) = collect_table_lines(&lines, i);
            for chunk in split_table_block(&table_lines, max_len) {
                buf.emit_table_chunk(&chunk);
            }
            i = next_i;
            continue;
        }

        buf.emit_line(line);
        i += 1;
    }
    buf.finalize()
}

fn collect_table_lines(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut table_lines = vec![lines[start].to_string(), lines[start + 1].to_string()];
    let mut cursor = start + 2;
    while cursor < lines.len() && lines[cursor].contains('|') {
        if is_fence(lines[cursor].trim()) {
            break;
        }
        table_lines.push(lines[cursor].to_string());
        cursor += 1;
    }
    (table_lines, cursor)
}

fn split_table_block(table_lines: &[String], max_len: usize) -> Vec<String> {
    if table_lines.len() < 2 {
        return vec![table_lines.join("\n")];
    }
    let header = &table_lines[0];
    let separator = &table_lines[1];
    let data_rows = &table_lines[2..];
    if data_rows.is_empty() {
        return vec![table_lines.join("\n")];
    }

    let preamble = format!("{header}\n{separator}");
    let preamble_len = preamble.len() + 1;

    let mut chunks = Vec::new();
    let mut current_rows: Vec<&str> = Vec::new();
    let mut current_len = preamble_len;
    for row in data_rows {
        let row_len = row.len() + 1;
        if !current_rows.is_empty() && current_len + row_len > max_len {
            chunks.push(format!("{preamble}\n{}", current_rows.join("\n")));
            current_rows.clear();
            current_len = preamble_len;
        }
        current_rows.push(row.as_str());
        current_len += row_len;
    }
    if !current_rows.is_empty() {
        chunks.push(format!("{preamble}\n{}", current_rows.join("\n")));
    }
    chunks
}

struct SplitBuffer {
    max_len: usize,
    chunks: Vec<String>,
    current: Vec<String>,
    length: usize,
    fence_open: String,
}

impl SplitBuffer {
    fn new(max_len: usize) -> Self {
        Self {
            max_len,
            chunks: Vec::new(),
            current: Vec::new(),
            length: 0,
            fence_open: String::new(),
        }
    }

    fn flush(&mut self) {
        let mut body = self.current.concat().trim_end_matches('\n').to_string();
        if !self.fence_open.is_empty() {
            body.push('\n');
            body.push_str(&fence_marker(&self.fence_open));
        }
        self.chunks.push(body);
        self.current.clear();
        self.length = 0;
    }

    fn flush_for(&mut self, incoming_len: usize) {
        if self.current.is_empty() || self.length + incoming_len <= self.max_len {
            return;
        }
        let saved_fence = self.fence_open.clone();
        self.flush();
        if !saved_fence.is_empty() {
            let reopener = format!("{saved_fence}\n");
            self.length = reopener.len();
            self.current.push(reopener);
        }
    }

    fn hard_split_long_line(&mut self, line: &str) {
        let mut start = 0;
        while start < line.len() {
            let end = utf8_chunk_end(line, start, self.max_len);
            self.chunks.push(line[start..end].to_string());
            start = end;
        }
    }

    fn emit_line(&mut self, line: &str) {
        let line_with_nl = format!("{line}\n");
        self.flush_for(line_with_nl.len());
        if line_with_nl.len() > self.max_len {
            self.hard_split_long_line(line);
            return;
        }
        self.length += line_with_nl.len();
        self.current.push(line_with_nl);
    }

    fn emit_table_chunk(&mut self, table_text: &str) {
        let block = format!("{}\n", table_text.trim_end_matches('\n'));
        if !self.current.is_empty() && self.length + block.len() > self.max_len {
            self.flush();
        }
        if block.len() > self.max_len && self.current.is_empty() {
            self.chunks.push(block.trim_end_matches('\n').to_string());
            return;
        }
        self.length += block.len();
        self.current.push(block);
    }

    fn finalize(mut self) -> Vec<String> {
        if !self.current.is_empty() {
            self.chunks
                .push(self.current.concat().trim_end_matches('\n').to_string());
        }
        self.chunks
            .into_iter()
            .filter(|c| !c.trim().is_empty())
            .collect()
    }
}

fn utf8_chunk_end(text: &str, start: usize, max: usize) -> usize {
    let hard = (start + max).min(text.len());
    let mut i = hard;
    while i > start && !text.is_char_boundary(i) {
        i -= 1;
    }
    if i == start {
        let mut end = start + 1;
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        return end;
    }
    i
}

pub fn render_parts_to_string(parts: &[OutgoingContentPart]) -> String {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            OutgoingContentPart::Text { text }
            | OutgoingContentPart::Markdown { text }
            | OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
            OutgoingContentPart::Tool { name, status, .. } => {
                segments.push(format!("[Tool: {name} - {status:?}]"));
            }
            OutgoingContentPart::Image { alt, .. } => {
                segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
            }
            OutgoingContentPart::File { name, .. } => {
                let label = name.as_deref().unwrap_or("file");
                segments.push(format!("[File: {label}]"));
            }
            OutgoingContentPart::Audio { name, .. } => {
                if let Some(name) = name.as_deref().filter(|name| !name.is_empty()) {
                    segments.push(format!("[Audio: {name}]"));
                } else {
                    segments.push("[Audio]".to_string());
                }
            }
            OutgoingContentPart::Video { name, .. } => {
                if let Some(name) = name.as_deref().filter(|name| !name.is_empty()) {
                    segments.push(format!("[Video: {name}]"));
                } else {
                    segments.push("[Video]".to_string());
                }
            }
            OutgoingContentPart::Raw { value } => {
                segments.push(value.to_string());
            }
            OutgoingContentPart::Thinking { .. } => {}
        }
    }
    segments.join("\n")
}
