use crate::dispatch::parse_command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutKind {
    Clear,
    Compact,
}

pub fn input_window_start(msgs: &[(&str, &str)]) -> usize {
    for i in (0..msgs.len()).rev() {
        let (role, text) = msgs[i];
        if role != "user" {
            continue;
        }
        let Some((name, _)) = parse_command(text) else {
            continue;
        };
        match name {
            "clear" => {
                if i + 1 < msgs.len() {
                    return i + 2;
                }
                return i + 1;
            }
            "compact" => {
                return i;
            }
            _ => continue,
        }
    }
    0
}

pub fn is_input_ephemeral_command(name: &str) -> bool {
    !matches!(name, "clear" | "compact")
}

pub fn agent_input_indices(msgs: &[(&str, &str)]) -> Vec<usize> {
    let start = input_window_start(msgs);
    let mut indices = Vec::new();
    let mut i = start;
    while i < msgs.len() {
        let (role, text) = msgs[i];
        if role == "user" {
            if let Some((name, _)) = parse_command(text) {
                if is_input_ephemeral_command(name) {
                    if i + 1 < msgs.len() && msgs[i + 1].0 != "user" {
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        indices.push(i);
        i += 1;
    }
    indices
}
