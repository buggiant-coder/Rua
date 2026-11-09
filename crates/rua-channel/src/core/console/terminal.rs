use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const DEFAULT_HISTORY_CAPACITY: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub role: String,

    pub content: String,

    pub timestamp: DateTime<Utc>,
}

impl HistoryEntry {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            timestamp: Utc::now(),
        }
    }
}

pub struct TerminalHistory {
    entries: VecDeque<HistoryEntry>,
    capacity: usize,
}

impl TerminalHistory {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_HISTORY_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push_back(entry);
        if self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn entries(&self) -> &VecDeque<HistoryEntry> {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn last(&self) -> Option<&HistoryEntry> {
        self.entries.back()
    }

    pub fn recent(&self, n: usize) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

impl Default for TerminalHistory {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AnsiRenderer;

impl AnsiRenderer {
    pub fn bold(text: &str) -> String {
        format!("\x1b[1m{text}\x1b[0m")
    }

    pub fn color(text: &str, code: u8) -> String {
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    pub fn green(text: &str) -> String {
        Self::color(text, 32)
    }

    pub fn yellow(text: &str) -> String {
        Self::color(text, 33)
    }

    pub fn red(text: &str) -> String {
        Self::color(text, 31)
    }

    pub fn cyan(text: &str) -> String {
        Self::color(text, 36)
    }

    pub fn dim(text: &str) -> String {
        format!("\x1b[2m{text}\x1b[0m")
    }

    pub fn strip_ansi(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                if chars.peek() == Some(&'[') {
                    chars.next();

                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            } else {
                result.push(ch);
            }
        }

        result
    }

    pub fn format_user_prompt(text: &str) -> String {
        format!("{} {text}", Self::bold("You:"))
    }

    pub fn format_assistant_response(text: &str) -> String {
        format!("{} {}", Self::bold(&Self::green("Assistant:")), text)
    }

    pub fn format_error(text: &str) -> String {
        format!("{} {text}", Self::bold(&Self::red("Error:")))
    }

    pub fn format_thinking(text: &str) -> String {
        format!("{} {text}", Self::dim("Thinking:"))
    }
}

pub struct TerminalInput;

impl TerminalInput {
    pub fn parse_line(line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_string())
    }

    pub fn is_exit(input: &str) -> bool {
        let lower = input.trim().to_lowercase();
        matches!(lower.as_str(), "exit" | "quit" | "q" | ":q" | ":quit")
    }

    pub fn is_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }
}

pub struct TerminalOutput<W: std::io::Write> {
    writer: W,
}

impl<W: std::io::Write> TerminalOutput<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn write_user_input(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{}", AnsiRenderer::format_user_prompt(text))
    }

    pub fn write_assistant_response(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(
            self.writer,
            "{}",
            AnsiRenderer::format_assistant_response(text)
        )
    }

    pub fn write_error(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{}", AnsiRenderer::format_error(text))
    }

    pub fn write_thinking(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{}", AnsiRenderer::format_thinking(text))
    }

    pub fn write_raw(&mut self, text: &str) -> std::io::Result<()> {
        write!(self.writer, "{text}")
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
