use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn remove_comments(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    let mut state = CommentState::Normal;
    let mut found_comment = false;
    let mut comment_count: usize = 0;

    while let Some(&(_i, ch)) = chars.peek() {
        match state {
            CommentState::Normal => match ch {
                '"' => {
                    result.push(ch);
                    chars.next();
                    state = CommentState::InString;
                }
                '\'' => {
                    result.push(ch);
                    chars.next();
                    state = CommentState::InSingleString;
                }
                '/' => {
                    chars.next();

                    if let Some(&(_, next_ch)) = chars.peek() {
                        match next_ch {
                            '/' => {
                                found_comment = true;
                                comment_count += 1;
                                chars.next();
                                while let Some(&(_, c)) = chars.peek() {
                                    if c == '\n' {
                                        result.push('\n');
                                        chars.next();
                                        break;
                                    }
                                    chars.next();
                                }
                            }
                            '*' => {
                                found_comment = true;
                                comment_count += 1;
                                chars.next();
                                state = CommentState::InBlockComment;
                            }
                            _ => {
                                result.push('/');
                            }
                        }
                    } else {
                        result.push('/');
                    }
                }
                _ => {
                    result.push(ch);
                    chars.next();
                }
            },
            CommentState::InString => {
                result.push(ch);
                chars.next();
                match ch {
                    '\\' => {
                        if let Some(&(_, esc)) = chars.peek() {
                            result.push(esc);
                            chars.next();
                        }
                    }
                    '"' => {
                        state = CommentState::Normal;
                    }
                    _ => {}
                }
            }
            CommentState::InSingleString => {
                result.push(ch);
                chars.next();
                match ch {
                    '\\' => {
                        if let Some(&(_, esc)) = chars.peek() {
                            result.push(esc);
                            chars.next();
                        }
                    }
                    '\'' => {
                        state = CommentState::Normal;
                    }
                    _ => {}
                }
            }
            CommentState::InBlockComment => {
                chars.next();
                if ch == '*' {
                    if let Some(&(_, '/')) = chars.peek() {
                        chars.next();
                        state = CommentState::Normal;

                        result.push(' ');
                    }
                }
            }
        }
    }

    if !found_comment {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::RemoveComments,
            message: format!("removed {} comment(s)", comment_count),
            offset: None,
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CommentState {
    Normal,
    InString,
    InSingleString,
    InBlockComment,
}
