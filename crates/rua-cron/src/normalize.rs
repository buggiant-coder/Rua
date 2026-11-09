use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum NormalizeError {
    TooFewFields { count: usize },

    SixFieldsNotSupported,

    TooManyFields { count: usize },

    Empty,
}

impl fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewFields { count } => {
                write!(
                    f,
                    "cron expression has too few fields ({count}), need at least 3"
                )
            }
            Self::SixFieldsNotSupported => {
                write!(
                    f,
                    "6-field cron expressions (with seconds) are not supported"
                )
            }
            Self::TooManyFields { count } => {
                write!(f, "cron expression has too many fields ({count})")
            }
            Self::Empty => write!(f, "empty cron expression"),
        }
    }
}

impl std::error::Error for NormalizeError {}

fn dow_num_to_name(n: u8) -> Option<&'static str> {
    match n {
        0 | 7 => Some("sun"),
        1 => Some("mon"),
        2 => Some("tue"),
        3 => Some("wed"),
        4 => Some("thu"),
        5 => Some("fri"),
        6 => Some("sat"),
        _ => None,
    }
}

fn normalize_dow_token(token: &str) -> String {
    let token = token.trim();
    if token.is_empty() {
        return token.to_string();
    }

    let (base, step_suffix) = if let Some(idx) = token.rfind('/') {
        let (b, s) = token.split_at(idx);
        (b, s.to_string())
    } else {
        (token, String::new())
    };

    if base == "*" {
        return format!("*{step_suffix}");
    }

    if base.contains('-') {
        let parts: Vec<&str> = base.splitn(2, '-').collect();
        if parts.len() == 2 {
            let start = normalize_dow_single(parts[0]);
            let end = normalize_dow_single(parts[1]);
            return format!("{start}-{end}{step_suffix}");
        }
    }

    if base.contains(',') {
        let items: Vec<String> = base
            .split(',')
            .map(|s| normalize_dow_single(s.trim()))
            .collect();
        return format!("{}{step_suffix}", items.join(","));
    }

    let normalized = normalize_dow_single(base);
    format!("{normalized}{step_suffix}")
}

fn normalize_dow_single(s: &str) -> String {
    let s = s.trim();

    if let Ok(n) = s.parse::<u8>() {
        if let Some(name) = dow_num_to_name(n) {
            return name.to_string();
        }
    }

    s.to_lowercase()
}

pub fn normalize_cron_expr(expr: &str) -> Result<String, NormalizeError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(NormalizeError::Empty);
    }

    let fields: Vec<&str> = expr.split_whitespace().collect();
    let count = fields.len();

    match count {
        0 => Err(NormalizeError::Empty),
        1 | 2 => Err(NormalizeError::TooFewFields { count }),
        3 => {
            let dow = normalize_dow_token(fields[2]);
            Ok(format!("0 0 {} {} {}", fields[0], fields[1], dow))
        }
        4 => {
            let dow = normalize_dow_token(fields[3]);
            Ok(format!(
                "0 {} {} {} {}",
                fields[0], fields[1], fields[2], dow
            ))
        }
        5 => {
            let dow = normalize_dow_token(fields[4]);
            Ok(format!(
                "{} {} {} {} {}",
                fields[0], fields[1], fields[2], fields[3], dow
            ))
        }
        6 => Err(NormalizeError::SixFieldsNotSupported),
        _ => Err(NormalizeError::TooManyFields { count }),
    }
}
