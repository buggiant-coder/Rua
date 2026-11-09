use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn close_brackets(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut stack: Vec<char> = Vec::new();
    let mut state = BracketState::Normal;

    for ch in input.chars() {
        match state {
            BracketState::Normal => match ch {
                '"' => state = BracketState::InDoubleString,
                '\'' => state = BracketState::InSingleString,
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
            },
            BracketState::InDoubleString => match ch {
                '\\' => state = BracketState::DoubleEscape,
                '"' => state = BracketState::Normal,
                _ => {}
            },
            BracketState::InSingleString => match ch {
                '\\' => state = BracketState::SingleEscape,
                '\'' => state = BracketState::Normal,
                _ => {}
            },
            BracketState::DoubleEscape => state = BracketState::InDoubleString,
            BracketState::SingleEscape => state = BracketState::InSingleString,
        }
    }

    if stack.is_empty() {
        return None;
    }

    let mut result = input.to_string();
    if matches!(
        state,
        BracketState::InDoubleString | BracketState::DoubleEscape
    ) {
        result.push('"');
    } else if matches!(
        state,
        BracketState::InSingleString | BracketState::SingleEscape
    ) {
        result.push('\'');
    }

    let bracket_count = stack.len();

    for close in stack.into_iter().rev() {
        result.push(close);
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::CloseBrackets,
            message: format!("closed {} unclosed bracket(s)", bracket_count),
            offset: Some(input.len()),
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BracketState {
    Normal,
    InDoubleString,
    InSingleString,
    DoubleEscape,
    SingleEscape,
}
