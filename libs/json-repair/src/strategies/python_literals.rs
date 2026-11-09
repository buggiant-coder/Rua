use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

pub fn fix_python_literals(input: &str) -> Option<(String, RepairDiagnostic)> {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut found = false;
    let mut count: usize = 0;

    while i < len {
        let ch = bytes[i];

        if escape {
            result.push(ch as char);
            escape = false;
            i += 1;
            continue;
        }

        if in_string {
            result.push(ch as char);
            if ch == b'\\' {
                escape = true;
            } else if ch == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if ch == b'"' {
            in_string = true;
            result.push('"');
            i += 1;
            continue;
        }

        if let Some((replacement, skip)) = check_python_literal(bytes, i) {
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);

            let after_ok = i + skip >= len || !is_ident_char(bytes[i + skip]);

            if before_ok && after_ok {
                result.push_str(replacement);
                i += skip;
                found = true;
                count += 1;
                continue;
            }
        }

        if ch >= 0x80 {
            let c = input[i..].chars().next().unwrap();
            result.push(c);
            i += c.len_utf8();
        } else {
            result.push(ch as char);
            i += 1;
        }
    }

    if !found {
        return None;
    }

    Some((
        result,
        RepairDiagnostic {
            strategy: RepairStrategyKind::FixPythonLiterals,
            message: format!("fixed {} Python literal(s)", count),
            offset: None,
        },
    ))
}

fn check_python_literal(bytes: &[u8], i: usize) -> Option<(&'static str, usize)> {
    let remaining = &bytes[i..];

    if remaining.len() >= 4 && &remaining[..4] == b"True" {
        Some(("true", 4))
    } else if remaining.len() >= 5 && &remaining[..5] == b"False" {
        Some(("false", 5))
    } else if remaining.len() >= 4 && &remaining[..4] == b"None" {
        Some(("null", 4))
    } else {
        None
    }
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
