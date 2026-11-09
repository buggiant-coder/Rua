#[derive(Debug, Clone)]
pub struct RepairConfig {
    pub strip_fences: bool,

    pub fix_quotes: bool,

    pub remove_trailing_commas: bool,

    pub remove_comments: bool,

    pub quote_unquoted_keys: bool,

    pub close_brackets: bool,

    pub fix_control_chars: bool,

    pub handle_truncated: bool,

    pub fix_python_literals: bool,

    pub extract_first_json: bool,

    pub max_depth: usize,

    pub max_input_bytes: usize,

    pub max_output_bytes: usize,

    pub allow_low_confidence: bool,
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self {
            strip_fences: true,
            fix_quotes: true,
            remove_trailing_commas: true,
            remove_comments: true,
            quote_unquoted_keys: true,
            close_brackets: true,
            fix_control_chars: true,
            handle_truncated: true,
            fix_python_literals: true,
            extract_first_json: false,
            max_depth: 64,
            max_input_bytes: 256 * 1024,
            max_output_bytes: 256 * 1024,
            allow_low_confidence: false,
        }
    }
}

impl RepairConfig {
    pub fn tool_call() -> Self {
        Self {
            extract_first_json: true,
            ..Default::default()
        }
    }

    pub fn strict() -> Self {
        Self {
            extract_first_json: false,
            handle_truncated: false,
            fix_python_literals: false,
            allow_low_confidence: false,
            ..Default::default()
        }
    }
}
