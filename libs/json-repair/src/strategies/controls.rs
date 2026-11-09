use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

#[derive(Debug, Clone, Copy, PartialEq)]
enum ControlState {
    Normal,
    InString,
    Escape,
}

pub fn fix_control_chars(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len());
    let mut state = ControlState::Normal;
    let mut found = false;
    let mut count: usize = 0;

    for ch in input.chars() {
        match state {
            ControlState::Normal => match ch {
                '"' => {
                    result.push('"');
                    state = ControlState::InString;
                }
                _ => {
                    result.push(ch);
                }
            },
            ControlState::InString => match ch {
                '\\' => {
                    result.push('\\');
                    state = ControlState::Escape;
                }
                '"' => {
                    result.push('"');
                    state = ControlState::Normal;
                }
                c if c.is_control() => {
                    found = true;
                    count += 1;
                    match c {
                        '\n' => result.push_str("\\n"),
                        '\r' => result.push_str("\\r"),
                        '\t' => result.push_str("\\t"),
                        '\u{08}' => result.push_str("\\b"),
                        '\u{0C}' => result.push_str("\\f"),
                        other => {
                            result.push_str(&format!("\\u{:04x}", other as u32));
                        }
                    }
                }
                _ => {
                    result.push(ch);
                }
            },
            ControlState::Escape => {
                result.push(ch);
                state = ControlState::InString;
            }
        }
    }

    if !found {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::FixControlChars,
            message: format!("escaped {} control character(s)", count),
            offset: None,
        },
    ))
}
