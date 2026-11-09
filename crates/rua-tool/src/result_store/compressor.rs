use agent_types::TruncationMeta;

use super::tool_call_logger::recall_key;
use super::truncation::{format_truncation_warning, hard_cap_meta, recall_meta};

pub fn estimate_text_tokens(text: &str, divisor: f64) -> usize {
    estimate_tokens_for_bytes(text.len(), divisor)
}

fn estimate_tokens_for_bytes(byte_count: usize, divisor: f64) -> usize {
    let d = if divisor.is_finite() && divisor >= 1.0 {
        divisor
    } else {
        4.0
    };
    ((byte_count as f64) / d).ceil().max(1.0) as usize
}

pub fn compress_output_tokens(
    output: &str,
    max_tokens: usize,
    tool_name: &str,
    tool_use_id: Option<&str>,
    token_divisor: f64,
) -> (String, Option<TruncationMeta>) {
    if estimate_text_tokens(output, token_divisor) <= max_tokens {
        return (output.to_string(), None);
    }

    let divisor = if token_divisor.is_finite() && token_divisor >= 1.0 {
        token_divisor
    } else {
        4.0
    };
    let mut body_bytes = ((max_tokens as f64) * divisor).floor() as usize;
    body_bytes = body_bytes.max(1);
    loop {
        let (decorated, meta) =
            compress_output(output, body_bytes, tool_name, tool_use_id, divisor);
        let visible_tokens = estimate_text_tokens(&decorated, divisor);
        if visible_tokens <= max_tokens || body_bytes == 1 {
            return (decorated, meta);
        }
        let excess_tokens = visible_tokens.saturating_sub(max_tokens).max(1);
        let reduction = ((excess_tokens as f64) * divisor).ceil() as usize + 64;
        body_bytes = body_bytes.saturating_sub(reduction).max(1);
    }
}

fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    index = index.min(s.len());
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(s: &str, mut index: usize) -> usize {
    index = index.min(s.len());
    while index < s.len() && !s.is_char_boundary(index) {
        index += 1;
    }
    index
}

pub fn format_byte_size(n: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;
    const GB: usize = 1024 * 1024 * 1024;
    if n >= GB {
        format!("{:.1}GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1}MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1}KB", n as f64 / KB as f64)
    } else {
        format!("{n}B")
    }
}

pub fn format_token_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn compress_output(
    output: &str,
    threshold: usize,
    tool_name: &str,
    tool_use_id: Option<&str>,
    token_divisor: f64,
) -> (String, Option<TruncationMeta>) {
    if output.len() <= threshold {
        return (output.to_string(), None);
    }

    let total_bytes = output.len();
    let lines: Vec<&str> = output.lines().collect();
    let total_lines = lines.len();

    let head_budget = threshold * 30 / 100;
    let tail_budget = threshold * 70 / 100;

    let mut head_end = 0;
    let mut head_line_count = 0;
    for line in &lines {
        let next = head_end + line.len() + 1;
        if next > head_budget && head_line_count > 0 {
            break;
        }
        head_end = next.min(output.len());
        head_line_count += 1;
    }

    let mut tail_start = output.len();
    let mut tail_line_count = 0;
    for line in lines.iter().rev() {
        let prev = tail_start.saturating_sub(line.len() + 1);
        if output.len() - prev > tail_budget && tail_line_count > 0 {
            break;
        }
        tail_start = prev;
        tail_line_count += 1;
    }

    let body = if tail_start <= head_end {
        let head_cut = floor_char_boundary(output, head_budget);
        let tail_cut = ceil_char_boundary(
            output,
            output.len().saturating_sub(tail_budget).max(head_cut),
        );
        let mut s = output[..head_cut].to_string();
        s.push_str(&format!(
            "\n\n... [truncated: {total_lines} lines, {} bytes total] ...\n\n",
            total_bytes
        ));
        if tail_cut < output.len() {
            s.push_str(&output[tail_cut..]);
        }
        s
    } else {
        let omitted = total_lines - head_line_count - tail_line_count;
        let mut s = String::with_capacity(threshold + 64);
        s.push_str(&output[..head_end]);
        s.push_str(&format!(
            "\n... [{omitted} lines omitted, {total_lines} total, {} bytes] ...\n\n",
            total_bytes
        ));
        if tail_start < output.len() {
            s.push_str(&output[tail_start..]);
        }
        s
    };

    let est_tokens = estimate_tokens_for_bytes(total_bytes, token_divisor) as u64;
    let meta = match tool_use_id {
        Some(id) => recall_meta(
            tool_name,
            Some(total_lines as u64),
            total_bytes as u64,
            est_tokens,
            recall_key(id),
        ),

        None => hard_cap_meta(
            tool_name,
            Some(total_lines as u64),
            head_line_count as u64 + tail_line_count as u64,
            "Output was not archived; full content is unavailable.",
        ),
    };

    let mut decorated = format_truncation_warning(&meta);
    decorated.push_str(&body);
    (decorated, Some(meta))
}
