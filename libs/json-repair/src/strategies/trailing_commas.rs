use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn remove_trailing_commas(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len());
    let mut state = TrailingState::Normal;
    let mut found = false;
    let mut count: usize = 0;

    let mut pending_comma = false;
    let mut pending_whitespace = String::new();

    for ch in input.chars() {
        match state {
            TrailingState::Normal => match ch {
                '"' => {
                    if pending_comma {
                        result.push(',');
                        pending_comma = false;
                    }
                    result.push_str(&pending_whitespace);
                    pending_whitespace.clear();
                    result.push('"');
                    state = TrailingState::InString;
                }
                '\'' => {
                    if pending_comma {
                        result.push(',');
                        pending_comma = false;
                    }
                    result.push_str(&pending_whitespace);
                    pending_whitespace.clear();
                    result.push('\'');
                    state = TrailingState::InSingleString;
                }
                ',' => {
                    if pending_comma {
                        found = true;
                        count += 1;

                        result.push_str(&pending_whitespace);
                        pending_whitespace.clear();
                    }
                    pending_comma = true;
                }
                '}' | ']' => {
                    if pending_comma {
                        found = true;
                        count += 1;
                        pending_comma = false;
                    }
                    result.push_str(&pending_whitespace);
                    pending_whitespace.clear();
                    result.push(ch);
                }
                c if c.is_whitespace() => {
                    pending_whitespace.push(c);
                }
                _ => {
                    if pending_comma {
                        result.push(',');
                        pending_comma = false;
                    }
                    result.push_str(&pending_whitespace);
                    pending_whitespace.clear();
                    result.push(ch);
                }
            },
            TrailingState::InString => match ch {
                '\\' => {
                    result.push('\\');
                    state = TrailingState::Escape;
                }
                '"' => {
                    result.push('"');
                    state = TrailingState::Normal;
                }
                _ => {
                    result.push(ch);
                }
            },
            TrailingState::InSingleString => match ch {
                '\\' => {
                    result.push('\\');
                    state = TrailingState::SingleEscape;
                }
                '\'' => {
                    result.push('\'');
                    state = TrailingState::Normal;
                }
                _ => {
                    result.push(ch);
                }
            },
            TrailingState::Escape => {
                result.push(ch);
                state = TrailingState::InString;
            }
            TrailingState::SingleEscape => {
                result.push(ch);
                state = TrailingState::InSingleString;
            }
        }
    }

    if pending_comma {
        result.push(',');
    }
    result.push_str(&pending_whitespace);

    if !found {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::RemoveTrailingCommas,
            message: format!("removed {} trailing comma(s)", count),
            offset: None,
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TrailingState {
    Normal,
    InString,
    InSingleString,
    Escape,
    SingleEscape,
}
