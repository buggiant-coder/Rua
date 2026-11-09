use agent_types::{ContentPart, ModelDialogueItem, Role, ToolResult};
use rua_tool::result_store::recall_key;

pub const DEFAULT_MAX_SUMMARY_CHARS: usize = 200;

const SNIPPED_MARKER: &str = "_snipped";

pub fn snip_old_tool_results(
    messages: &mut [ModelDialogueItem],
    keep_recent_n: usize,
    max_summary_chars: usize,
) -> usize {
    let rounds = identify_tool_rounds(messages);
    if rounds.len() <= keep_recent_n {
        return 0;
    }
    let stop = rounds.len() - keep_recent_n;
    let max_chars = if max_summary_chars == 0 {
        DEFAULT_MAX_SUMMARY_CHARS
    } else {
        max_summary_chars
    };

    let mut rewritten = 0;
    for round in &rounds[..stop] {
        if rewrite_round(messages, round, max_chars) {
            rewritten += 1;
        }
    }
    rewritten
}

#[derive(Debug)]
struct Round {
    tool_use_msg_idx: usize,
    tool_use_block_idx: usize,
    tool_use_id: String,
    tool_name: String,

    result_loc: Option<(usize, usize)>,
}

fn identify_tool_rounds(messages: &[ModelDialogueItem]) -> Vec<Round> {
    let mut rounds = Vec::new();
    for (msg_idx, msg) in messages.iter().enumerate() {
        if msg.role != Role::Assistant {
            continue;
        }
        for (block_idx, block) in msg.content.iter().enumerate() {
            if let ContentPart::ToolUse { call } = block {
                let result_loc = locate_matching_result(messages, msg_idx, &call.id);
                rounds.push(Round {
                    tool_use_msg_idx: msg_idx,
                    tool_use_block_idx: block_idx,
                    tool_use_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    result_loc,
                });
            }
        }
    }
    rounds
}

fn locate_matching_result(
    messages: &[ModelDialogueItem],
    after_idx: usize,
    tool_use_id: &str,
) -> Option<(usize, usize)> {
    for (idx, msg) in messages.iter().enumerate().skip(after_idx + 1) {
        if msg.role == Role::Assistant {
            break;
        }
        for (bi, block) in msg.content.iter().enumerate() {
            if let ContentPart::ToolResult { result } = block {
                if result.call_id == tool_use_id {
                    return Some((idx, bi));
                }
            }
        }
    }
    None
}

fn rewrite_round(messages: &mut [ModelDialogueItem], round: &Round, max_chars: usize) -> bool {
    let Some((result_idx, result_bi)) = round.result_loc else {
        return false;
    };

    if let Some(use_msg) = messages.get(round.tool_use_msg_idx) {
        if let Some(ContentPart::ToolUse { call }) = use_msg.content.get(round.tool_use_block_idx) {
            if let Some(obj) = call.arguments.as_object() {
                if obj.contains_key(SNIPPED_MARKER) {
                    return false;
                }
            }
        }
    }

    let args_summary = {
        let use_msg = messages.get(round.tool_use_msg_idx);
        let block = use_msg.and_then(|m| m.content.get(round.tool_use_block_idx));
        if let Some(ContentPart::ToolUse { call }) = block {
            summarize_args(&call.arguments, 80)
        } else {
            String::new()
        }
    };

    let output_summary = {
        let msg = messages.get(result_idx);
        let block = msg.and_then(|m| m.content.get(result_bi));
        if let Some(ContentPart::ToolResult { result }) = block {
            summarize_output(&render_output_text(&result.content), max_chars)
        } else {
            return false;
        }
    };

    if let Some(use_msg) = messages.get_mut(round.tool_use_msg_idx) {
        if let Some(ContentPart::ToolUse { call }) =
            use_msg.content.get_mut(round.tool_use_block_idx)
        {
            call.arguments = serde_json::json!({ SNIPPED_MARKER: args_summary });
            call.raw_arguments = None;
        }
    }

    let recall_hint = format!(
        "For full output, call recall_tool_result(recall_key=\"{}\")",
        recall_key(&round.tool_use_id)
    );
    let result_text = if output_summary.is_empty() {
        recall_hint.clone()
    } else {
        format!("{output_summary} | {recall_hint}")
    };
    let _ = round.tool_name.as_str();

    if let Some(msg) = messages.get_mut(result_idx) {
        if let Some(ContentPart::ToolResult { result }) = msg.content.get_mut(result_bi) {
            let preserved_call_id = result.call_id.clone();
            let preserved_name = result.name.clone();
            *result = ToolResult {
                call_id: preserved_call_id,
                name: preserved_name,
                content: vec![ContentPart::Text { text: result_text }],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            };
        }
    }

    true
}

fn render_output_text(content: &[ContentPart]) -> String {
    let mut parts = Vec::new();
    for block in content {
        match block {
            ContentPart::Text { text } => parts.push(text.as_str()),
            ContentPart::Thinking { text, .. } => parts.push(text.as_str()),
            _ => {}
        }
    }
    parts.join(" ")
}

fn summarize_args(args: &serde_json::Value, max_chars: usize) -> String {
    let raw = match args {
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = Vec::with_capacity(map.len());
            for (k, v) in map {
                let mut v_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if v_str.len() > 30 {
                    truncate_to(&mut v_str, 27);
                    v_str.push_str("...");
                }
                parts.push(format!("{k}={v_str}"));
            }
            parts.join(", ")
        }
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    cap_string(&raw, max_chars)
}

fn summarize_output(output: &str, max_chars: usize) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "(empty)".to_string();
    }
    if trimmed.len() <= max_chars {
        return trimmed.to_string();
    }
    let half = (max_chars.saturating_sub(20)) / 2;
    let head_end = clamp_to_char_boundary(trimmed, half);
    let tail_start = clamp_to_char_boundary_reverse(trimmed, trimmed.len() - half);
    format!(
        "{} ... ({} chars) ... {}",
        &trimmed[..head_end],
        trimmed.len(),
        &trimmed[tail_start..],
    )
}

fn cap_string(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let cut = clamp_to_char_boundary(s, max_chars.saturating_sub(3));
    let mut out = s[..cut].to_string();
    out.push_str("...");
    out
}

fn truncate_to(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let cut = clamp_to_char_boundary(s, max);
    s.truncate(cut);
}

fn clamp_to_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn clamp_to_char_boundary_reverse(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}
