#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairDiagnostic {
    pub strategy: RepairStrategyKind,

    pub message: String,

    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairStrategyKind {
    StripFences,

    ExtractFirstJson,

    RemoveComments,

    FixPythonLiterals,

    FixQuotes,

    RemoveTrailingCommas,

    QuoteUnquotedKeys,

    FixControlChars,

    CloseBrackets,

    SafeTruncation,
}

impl std::fmt::Display for RepairStrategyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StripFences => write!(f, "strip_fences"),
            Self::ExtractFirstJson => write!(f, "extract_first_json"),
            Self::RemoveComments => write!(f, "remove_comments"),
            Self::FixPythonLiterals => write!(f, "fix_python_literals"),
            Self::FixQuotes => write!(f, "fix_quotes"),
            Self::RemoveTrailingCommas => write!(f, "remove_trailing_commas"),
            Self::QuoteUnquotedKeys => write!(f, "quote_unquoted_keys"),
            Self::FixControlChars => write!(f, "fix_control_chars"),
            Self::CloseBrackets => write!(f, "close_brackets"),
            Self::SafeTruncation => write!(f, "safe_truncation"),
        }
    }
}
