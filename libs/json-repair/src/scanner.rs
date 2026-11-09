#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Normal,

    InDoubleString,

    InSingleString,

    Escape,

    SingleEscape,
}

#[derive(Debug, Clone)]
pub struct StringAwareScanner {
    state: ScanState,
}

impl Default for StringAwareScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl StringAwareScanner {
    pub fn new() -> Self {
        Self {
            state: ScanState::Normal,
        }
    }

    pub fn feed(&mut self, ch: char) -> ScanState {
        self.state = match self.state {
            ScanState::Normal => match ch {
                '"' => ScanState::InDoubleString,
                '\'' => ScanState::InSingleString,
                _ => ScanState::Normal,
            },
            ScanState::InDoubleString => match ch {
                '\\' => ScanState::Escape,
                '"' => ScanState::Normal,
                _ => ScanState::InDoubleString,
            },
            ScanState::InSingleString => match ch {
                '\\' => ScanState::SingleEscape,
                '\'' => ScanState::Normal,
                _ => ScanState::InSingleString,
            },
            ScanState::Escape => ScanState::InDoubleString,
            ScanState::SingleEscape => ScanState::InSingleString,
        };
        self.state
    }

    pub fn in_string(&self) -> bool {
        matches!(
            self.state,
            ScanState::InDoubleString
                | ScanState::InSingleString
                | ScanState::Escape
                | ScanState::SingleEscape
        )
    }

    #[allow(dead_code)]
    pub fn state(&self) -> ScanState {
        self.state
    }
}

pub fn check_depth(input: &str, max_depth: usize) -> Result<usize, usize> {
    let mut scanner = StringAwareScanner::new();
    let mut depth: usize = 0;
    let mut max_seen: usize = 0;

    for ch in input.chars() {
        let state = scanner.feed(ch);
        if state == ScanState::Normal {
            match ch {
                '{' | '[' => {
                    depth = depth.saturating_add(1);
                    if depth > max_seen {
                        max_seen = depth;
                    }
                    if max_seen > max_depth {
                        return Err(max_seen);
                    }
                }
                '}' | ']' => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    Ok(max_seen)
}
