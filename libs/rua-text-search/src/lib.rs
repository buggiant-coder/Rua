use grep_matcher::Matcher;
use grep_regex::RegexMatcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearchQuery {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
}

impl TextSearchQuery {
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            is_regex: false,
            case_sensitive: false,
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            is_regex: true,
            case_sensitive: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TextSearchError {
    #[error("invalid text search pattern: {0}")]
    InvalidPattern(String),
}

pub struct TextMatcher {
    matcher: RegexMatcher,
}

impl TextMatcher {
    pub fn new(query: TextSearchQuery) -> Result<Self, TextSearchError> {
        let pattern = regex_pattern(&query);
        let matcher = RegexMatcher::new(&pattern)
            .map_err(|e| TextSearchError::InvalidPattern(e.to_string()))?;
        Ok(Self { matcher })
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.matcher.is_match(text.as_bytes()).unwrap_or(false)
    }
}

pub fn regex_pattern(query: &TextSearchQuery) -> String {
    let pattern = if query.is_regex {
        query.pattern.clone()
    } else {
        regex_escape(&query.pattern)
    };
    if query.case_sensitive {
        pattern
    } else {
        format!("(?i){pattern}")
    }
}

pub fn regex_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}
