use std::collections::HashSet;

use agent_types::{ContentPart, ModelDialogueItem, Role};

const READ_TOOLS: &[&str] = &["read"];

const BASH_TOOLS: &[&str] = &["bash"];

const SEARCH_TOOLS: &[&str] = &["grep", "glob", "web_search", "web_fetch"];

const EDIT_TOOLS: &[&str] = &["edit"];

pub fn preprocess_tool_results(
    messages: &mut [ModelDialogueItem],
    recent_n: usize,
    old_max_bytes: usize,
) -> usize {
    if recent_n == 0 || old_max_bytes == 0 {
        return 0;
    }

    let rounds = find_tool_rounds(messages);
    if rounds.len() <= recent_n {
        return 0;
    }

    let stop = rounds.len() - recent_n;
    let mut compressed = 0;
    for round in &rounds[..stop] {
        compressed += usize::from(compress_round(messages, round, old_max_bytes));
    }

    compressed
}

pub fn compact_projected_tool_results(
    messages: &mut [ModelDialogueItem],
    recent_n: usize,
    old_max_bytes: usize,
    batch_size: usize,
) -> usize {
    let mut rounds = find_tool_rounds(messages);
    let batch_size = batch_size.max(1);
    if batch_size > 1 {
        rounds.retain(|round| round.result_loc.is_some());
    }
    // Derive a stable boundary from completed results, not model-call count or
    // wall time. Rebuilding, retrying, or restarting the same history is idempotent.
    // Results after the last full batch stay intact along with its recent window.
    let batch_end = rounds.len() - rounds.len() % batch_size;
    let recent_start = batch_end.saturating_sub(recent_n);
    let mut truncated = 0;

    if old_max_bytes == 0 {
        return truncated;
    }
    for round in &rounds[..recent_start] {
        truncated += usize::from(compress_round(messages, round, old_max_bytes));
    }

    truncated
}

#[derive(Debug)]
struct ToolRound {
    tool_name: String,

    result_loc: Option<(usize, usize)>,
}

fn find_tool_rounds(messages: &[ModelDialogueItem]) -> Vec<ToolRound> {
    let mut rounds: Vec<ToolRound> = Vec::new();
    for (msg_idx, msg) in messages.iter().enumerate() {
        if msg.role != Role::Assistant {
            continue;
        }
        for block in &msg.content {
            if let ContentPart::ToolUse { call } = block {
                let result_loc = locate_matching_result(messages, msg_idx, &call.id);
                rounds.push(ToolRound {
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

fn compress_round(
    messages: &mut [ModelDialogueItem],
    round: &ToolRound,
    old_max_bytes: usize,
) -> bool {
    let Some((result_msg_idx, result_block_idx)) = round.result_loc else {
        return false;
    };
    let Some(msg) = messages.get_mut(result_msg_idx) else {
        return false;
    };
    let Some(ContentPart::ToolResult { result }) = msg.content.get_mut(result_block_idx) else {
        return false;
    };

    let output = render_output_text(&result.content);
    if output.len() <= old_max_bytes {
        return false;
    }

    let strategy = select_strategy(&round.tool_name);
    let compressed = strategy(&output, old_max_bytes);
    result.content = vec![ContentPart::Text {
        text: head_byte_trim(&compressed, old_max_bytes),
    }];
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
    parts.join("\n")
}

fn select_strategy(tool_name: &str) -> fn(&str, usize) -> String {
    let read: HashSet<&str> = READ_TOOLS.iter().copied().collect();
    let bash: HashSet<&str> = BASH_TOOLS.iter().copied().collect();
    let search: HashSet<&str> = SEARCH_TOOLS.iter().copied().collect();
    let edit: HashSet<&str> = EDIT_TOOLS.iter().copied().collect();

    if read.contains(tool_name) {
        compress_read
    } else if bash.contains(tool_name) {
        compress_bash
    } else if search.contains(tool_name) {
        compress_search
    } else if edit.contains(tool_name) {
        compress_edit
    } else {
        compress_generic
    }
}

fn compress_read(content: &str, max_bytes: usize) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.len() <= 30 {
        return head_byte_trim(content, max_bytes);
    }
    let head_n = 20;
    let tail_n = 10;
    let truncated = lines.len() - head_n - tail_n;
    let mut out = lines[..head_n].join("\n");
    out.push_str(&format!("\n... ({truncated} lines truncated) ...\n"));
    out.push_str(&lines[lines.len() - tail_n..].join("\n"));
    out
}

fn compress_bash(content: &str, max_bytes: usize) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let keep = 30;
    if lines.len() <= keep {
        return head_byte_trim(content, max_bytes);
    }
    let truncated = lines.len() - keep;
    let mut out = format!("... ({truncated} lines truncated) ...\n");
    out.push_str(&lines[lines.len() - keep..].join("\n"));
    out
}

fn compress_search(content: &str, max_bytes: usize) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.len() <= 30 {
        return head_byte_trim(content, max_bytes);
    }
    let mut kept: Vec<String> = Vec::new();
    let mut total = 0usize;
    for line in &lines {
        let line_bytes = line.len() + 1;
        if total + line_bytes > max_bytes {
            kept.push(format!("... ({} more lines)", lines.len() - kept.len()));
            break;
        }
        kept.push((*line).to_string());
        total += line_bytes;
    }
    kept.join("\n")
}

fn compress_edit(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let head_n = 5;
    let mut out = lines[..head_n.min(lines.len())].join("\n");
    if lines.len() > head_n {
        out.push_str(&format!("\n... ({} lines truncated)", lines.len() - head_n));
    }
    head_byte_trim(&out, max_bytes)
}

fn compress_generic(content: &str, max_bytes: usize) -> String {
    head_byte_trim(content, max_bytes)
}

fn head_byte_trim(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    const MARKER: &str = " ...";
    let mut cut = max_bytes.saturating_sub(MARKER.len());
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = content[..cut].to_string();
    if max_bytes >= MARKER.len() {
        out.push_str(MARKER);
    }
    out
}
