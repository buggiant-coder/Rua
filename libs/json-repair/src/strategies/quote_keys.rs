use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn quote_unquoted_keys(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len() + 32);
    let mut state = KeyState::Normal;
    let mut found = false;
    let mut count: usize = 0;
    let mut key_buf = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match state {
            KeyState::Normal => match ch {
                '"' => {
                    result.push('"');
                    state = KeyState::InDoubleString;
                }
                '\'' => {
                    result.push('\'');
                    state = KeyState::InSingleString;
                }
                '{' | ',' => {
                    result.push(ch);
                    state = KeyState::ExpectingKey;
                }
                _ => {
                    result.push(ch);
                }
            },
            KeyState::ExpectingKey => {
                if ch.is_whitespace() {
                    result.push(ch);
                } else if ch == '"' || ch == '\'' {
                    result.push(ch);
                    state = if ch == '"' {
                        KeyState::InDoubleString
                    } else {
                        KeyState::InSingleString
                    };
                } else if ch == '}' || ch == ']' {
                    result.push(ch);
                    state = KeyState::Normal;
                } else if is_key_start_char(ch) {
                    key_buf.clear();
                    key_buf.push(ch);
                    state = KeyState::InUnquotedKey;
                } else {
                    result.push(ch);
                    state = KeyState::Normal;
                }
            }
            KeyState::InUnquotedKey => {
                if is_key_char(ch) {
                    key_buf.push(ch);
                } else if ch == ':' {
                    result.push('"');
                    result.push_str(&key_buf);
                    result.push('"');
                    result.push(':');
                    key_buf.clear();
                    found = true;
                    count += 1;
                    state = KeyState::Normal;
                } else {
                    result.push_str(&key_buf);
                    result.push(ch);
                    key_buf.clear();
                    state = KeyState::Normal;
                }
            }
            KeyState::InDoubleString => match ch {
                '\\' => {
                    result.push('\\');
                    if let Some(next) = chars.next() {
                        result.push(next);
                    }
                }
                '"' => {
                    result.push('"');
                    state = KeyState::AfterValue;
                }
                _ => {
                    result.push(ch);
                }
            },
            KeyState::InSingleString => match ch {
                '\\' => {
                    result.push('\\');
                    if let Some(next) = chars.next() {
                        result.push(next);
                    }
                }
                '\'' => {
                    result.push('\'');
                    state = KeyState::AfterValue;
                }
                _ => {
                    result.push(ch);
                }
            },
            KeyState::AfterValue => match ch {
                ':' => {
                    result.push(':');
                    state = KeyState::Normal;
                }
                ',' => {
                    result.push(',');
                    state = KeyState::ExpectingKey;
                }
                '}' | ']' => {
                    result.push(ch);
                    state = KeyState::Normal;
                }
                c if c.is_whitespace() => {
                    result.push(c);
                }
                _ => {
                    result.push(ch);
                    state = KeyState::Normal;
                }
            },
        }
    }

    if !key_buf.is_empty() {
        result.push_str(&key_buf);
    }

    if !found {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::QuoteUnquotedKeys,
            message: format!("quoted {} unquoted key(s)", count),
            offset: None,
        },
    ))
}

fn is_key_start_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '$'
}

fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' || ch == '.' || ch == '-'
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyState {
    Normal,
    ExpectingKey,
    InUnquotedKey,
    InDoubleString,
    InSingleString,
    AfterValue,
}
