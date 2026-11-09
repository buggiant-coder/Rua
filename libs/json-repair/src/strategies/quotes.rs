use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn fix_quotes(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len());
    let mut state = QuoteState::Normal;
    let mut found = false;
    let mut count: usize = 0;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match state {
            QuoteState::Normal => match ch {
                '\'' => {
                    result.push('"');
                    state = QuoteState::InSingleString;
                    found = true;
                    count += 1;
                }
                '"' => {
                    result.push('"');
                    state = QuoteState::InDoubleString;
                }
                _ => {
                    result.push(ch);
                }
            },
            QuoteState::InSingleString => match ch {
                '\\' => {
                    if let Some(&next) = chars.peek() {
                        if next == '\'' {
                            result.push('\'');
                            chars.next();
                        } else if next == '"' {
                            result.push('\\');
                            result.push('"');
                            chars.next();
                        } else {
                            result.push('\\');
                            result.push(next);
                            chars.next();
                        }
                    } else {
                        result.push('\\');
                    }
                }
                '\'' => {
                    result.push('"');
                    state = QuoteState::Normal;
                }
                '"' => {
                    result.push('\\');
                    result.push('"');
                }
                _ => {
                    result.push(ch);
                }
            },
            QuoteState::InDoubleString => match ch {
                '\\' => {
                    result.push('\\');
                    if let Some(next) = chars.next() {
                        result.push(next);
                    }
                }
                '"' => {
                    result.push('"');
                    state = QuoteState::Normal;
                }
                _ => {
                    result.push(ch);
                }
            },
        }
    }

    if !found {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::FixQuotes,
            message: format!("fixed {} single-quoted string(s)", count),
            offset: None,
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum QuoteState {
    Normal,
    InSingleString,
    InDoubleString,
}
