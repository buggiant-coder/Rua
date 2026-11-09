use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};
use crate::scanner::StringAwareScanner;

pub fn extract_first_json(input: &str) -> Option<(String, RepairDiagnostic)> {
    let bytes = input.as_bytes();
    let mut scanner = StringAwareScanner::new();

    let mut start = None;
    let mut open_char = ' ';

    for (i, ch) in input.char_indices() {
        let _state = scanner.feed(ch);
        if !scanner.in_string() && (ch == '{' || ch == '[') {
            start = Some(i);
            open_char = ch;
            break;
        }
    }

    let start_idx = start?;
    let close_char = if open_char == '{' { '}' } else { ']' };

    let mut depth: usize = 0;
    let mut scanner2 = StringAwareScanner::new();
    let mut end_idx = None;

    for (i, ch) in input[start_idx..].char_indices() {
        let _state = scanner2.feed(ch);
        if !scanner2.in_string() {
            if ch == open_char {
                depth += 1;
            } else if ch == close_char {
                depth -= 1;
                if depth == 0 {
                    end_idx = Some(start_idx + i + ch.len_utf8());
                    break;
                }
            }
        }
    }

    let end = end_idx?;
    let extracted = &input[start_idx..end];

    if start_idx == 0 && end == bytes.len() {
        return None;
    }

    if input.trim() == extracted.trim() {
        return None;
    }

    Some((
        extracted.to_string(),
        RepairDiagnostic {
            strategy: RepairStrategyKind::ExtractFirstJson,
            message: format!("extracted JSON from offset {} to {}", start_idx, end),
            offset: Some(start_idx),
        },
    ))
}
