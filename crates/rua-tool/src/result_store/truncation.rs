use agent_types::TruncationMeta;

use super::compressor::{format_byte_size, format_token_count};

const TAG_OPEN_PREFIX: &str = "<truncated_output tool=\"";
const TAG_CLOSE: &str = "</truncated_output>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationKind {
    Continuation,

    Recall,

    HardCap,
}

pub fn truncation_kind(meta: &TruncationMeta) -> TruncationKind {
    if meta.recall_key.is_some() {
        TruncationKind::Recall
    } else if meta.continuation_hint.is_some() {
        TruncationKind::Continuation
    } else {
        TruncationKind::HardCap
    }
}

pub fn format_truncation_warning(meta: &TruncationMeta) -> String {
    let mut s = String::with_capacity(256);
    s.push_str(TAG_OPEN_PREFIX);
    s.push_str(&xml_escape(&meta.tool));
    s.push_str("\">\n");

    match truncation_kind(meta) {
        TruncationKind::Recall => write_recall_body(&mut s, meta),
        TruncationKind::Continuation => write_continuation_body(&mut s, meta),
        TruncationKind::HardCap => write_hard_cap_body(&mut s, meta),
    }

    s.push_str(TAG_CLOSE);
    s.push_str("\n\n");
    s
}

fn write_continuation_body(s: &mut String, meta: &TruncationMeta) {
    s.push_str("Output too large to return in one call.\n");
    write_total_size(s, meta);
    write_shown_range(s, meta);
    if let Some(hint) = &meta.continuation_hint {
        s.push_str("To continue, call ");
        s.push_str(hint);
        s.push_str(".\n");
    }
}

fn write_recall_body(s: &mut String, meta: &TruncationMeta) {
    s.push_str("Output too long to show inline; full output saved on disk.\n");
    write_total_size(s, meta);
    write_shown_range(s, meta);
    if let Some(key) = &meta.recall_key {
        let key_e = xml_escape(key);
        s.push_str("For full output, call recall_tool_result(recall_key=\"");
        s.push_str(&key_e);
        s.push_str("\").\n");
        s.push_str("For a slice, call recall_tool_result(recall_key=\"");
        s.push_str(&key_e);
        s.push_str("\", start_line=N, end_line=M).\n");
    }
    if let Some(hint) = &meta.continuation_hint {
        s.push_str("To continue from the next omitted line, call ");
        s.push_str(hint);
        s.push_str(".\n");
    }
}

fn write_hard_cap_body(s: &mut String, meta: &TruncationMeta) {
    s.push_str("Results truncated; no continuation available.\n");
    write_total_size(s, meta);
    write_shown_range(s, meta);
    if let Some(hint) = &meta.retry_hint {
        s.push_str(hint);
        if !hint.ends_with('\n') {
            s.push('\n');
        }
    }
}

fn write_total_size(s: &mut String, meta: &TruncationMeta) {
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = meta.total_lines {
        parts.push(format!("{n} lines"));
    }
    if let Some(b) = meta.total_bytes {
        parts.push(format_byte_size(b as usize));
    }
    if let Some(t) = meta.est_tokens {
        parts.push(format!("~{} tokens", format_token_count(t as usize)));
    }
    if !parts.is_empty() {
        s.push_str("Total: ");
        s.push_str(&parts.join(", "));
        s.push_str(".\n");
    }
}

fn write_shown_range(s: &mut String, meta: &TruncationMeta) {
    if let (Some(start), Some(end)) = (meta.shown_start, meta.shown_end) {
        s.push_str(&format!("Showing: lines {start}-{end}.\n"));
    }
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn continuation_meta(
    tool: impl Into<String>,
    total_lines: Option<u64>,
    total_bytes: Option<u64>,
    shown_start: u64,
    shown_end: u64,
    continuation_hint: impl Into<String>,
) -> TruncationMeta {
    let est_tokens = total_bytes.map(|b| (b / 4).max(1));
    TruncationMeta {
        tool: tool.into(),
        total_lines,
        total_bytes,
        est_tokens,
        shown_start: Some(shown_start),
        shown_end: Some(shown_end),
        continuation_hint: Some(continuation_hint.into()),
        recall_key: None,
        retry_hint: None,
    }
}

pub fn recall_meta(
    tool: impl Into<String>,
    total_lines: Option<u64>,
    total_bytes: u64,
    est_tokens: u64,
    recall_key: impl Into<String>,
) -> TruncationMeta {
    TruncationMeta {
        tool: tool.into(),
        total_lines,
        total_bytes: Some(total_bytes),
        est_tokens: Some(est_tokens),
        shown_start: None,
        shown_end: None,
        continuation_hint: None,
        recall_key: Some(recall_key.into()),
        retry_hint: None,
    }
}

pub fn hard_cap_meta(
    tool: impl Into<String>,
    total_lines: Option<u64>,
    shown_end: u64,
    retry_hint: impl Into<String>,
) -> TruncationMeta {
    TruncationMeta {
        tool: tool.into(),
        total_lines,
        total_bytes: None,
        est_tokens: None,
        shown_start: Some(1),
        shown_end: Some(shown_end),
        continuation_hint: None,
        recall_key: None,
        retry_hint: Some(retry_hint.into()),
    }
}
