use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn strip_fences(input: &str) -> Option<(String, RepairDiagnostic)> {
    let trimmed = input.trim();

    let (opening_end, fence_len) = find_opening_fence(trimmed)?;

    let closing_start = find_closing_fence(trimmed, opening_end, fence_len)?;

    let content = &trimmed[opening_end..closing_start];
    let result = content.trim().to_string();

    if result.is_empty() {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::StripFences,
            message: "stripped markdown code fences".to_string(),
            offset: Some(0),
        },
    ))
}

fn find_opening_fence(input: &str) -> Option<(usize, usize)> {
    let backtick_count = input.chars().take_while(|&c| c == '`').count();
    if backtick_count < 3 {
        return None;
    }

    let rest = &input[backtick_count..];
    let line_end = rest.find('\n').map(|i| backtick_count + i + 1)?;

    let lang_tag = rest[..line_end - backtick_count - 1].trim();
    if !lang_tag.is_empty()
        && !lang_tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }

    Some((line_end, backtick_count))
}

fn find_closing_fence(input: &str, start: usize, fence_len: usize) -> Option<usize> {
    let fence_str = "`".repeat(fence_len);
    let search_area = &input[start..];

    for (i, line) in search_area.lines().rev().enumerate() {
        let trimmed = line.trim();
        if trimmed == fence_str
            || (trimmed.starts_with(&fence_str) && trimmed.chars().all(|c| c == '`'))
        {
            let total_lines: Vec<&str> = search_area.lines().collect();
            let line_idx = total_lines.len() - 1 - i;
            let mut offset = 0;
            for l in &total_lines[..line_idx] {
                offset += l.len() + 1;
            }
            return Some(start + offset);
        }
    }

    None
}
