use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn safe_truncate(input: &str) -> Option<(String, RepairDiagnostic)> {
    if serde_json::from_str::<serde_json::Value>(input).is_ok() {
        return None;
    }

    if input.is_empty() {
        return None;
    }

    let candidates = find_truncation_candidates(input);

    for truncation_point in candidates {
        if truncation_point == 0 {
            continue;
        }

        let truncated = &input[..truncation_point];

        let result = close_structure(truncated);

        if serde_json::from_str::<serde_json::Value>(&result).is_ok() {
            return Some((
                result,
                RepairDiagnostic {
                    strategy: RepairStrategyKind::SafeTruncation,
                    message: format!(
                        "safely truncated at byte {} (original {} bytes)",
                        truncation_point,
                        input.len()
                    ),
                    offset: Some(truncation_point),
                },
            ));
        }
    }

    None
}

fn close_structure(input: &str) -> String {
    let mut result = input.to_string();
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if let Some(&top) = stack.last() {
                    if top == ch {
                        stack.pop();
                    }
                }
            }
            _ => {}
        }
    }

    if in_string || escape {
        result.push('"');
    }

    for close in stack.into_iter().rev() {
        result.push(close);
    }

    result
}

fn find_truncation_candidates(input: &str) -> Vec<usize> {
    let mut candidates = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut depth: i32 = 0;
    let mut after_colon = false;

    for (i, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;

                if after_colon || depth == 0 {
                    candidates.push(i + 1);
                }
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
            }
            '{' | '[' => {
                depth += 1;
                after_colon = false;
            }
            '}' | ']' => {
                depth -= 1;
                if depth >= 0 {
                    candidates.push(i + 1);
                }
                after_colon = false;
            }
            ',' => {
                candidates.push(i);
                after_colon = false;
            }
            ':' => {
                after_colon = true;
            }
            c if !c.is_whitespace() => {}
            _ => {}
        }
    }

    for (i, ch) in input.char_indices() {
        if ch == '{' || ch == '[' {
            candidates.push(i + 1);
            break;
        }
    }

    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.dedup();
    candidates
}
