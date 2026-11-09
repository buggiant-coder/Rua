use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySignalKind {
    ExplicitMemory,
    MissedRecall,
    Correction,
    Preference,
    Convention,
    Frustration,
}

impl MemorySignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitMemory => "explicit_memory",
            Self::MissedRecall => "missed_recall",
            Self::Correction => "correction",
            Self::Preference => "preference",
            Self::Convention => "convention",
            Self::Frustration => "frustration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySignalConfidence {
    High,
    Medium,
    Low,
}

impl MemorySignalConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySignalState {
    #[serde(default)]
    pub frustration_hits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySignal {
    pub kind: MemorySignalKind,
    pub confidence: MemorySignalConfidence,
    pub suggested_memory_kind: String,
    pub suggested_scope: String,
    pub evidence: String,
}

pub fn detect_memory_signal(
    user_input: &str,
    memory_write_available: bool,
    state: &mut MemorySignalState,
) -> Option<MemorySignal> {
    if !memory_write_available {
        return None;
    }

    let cleaned = strip_ignored_regions(user_input);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || matches_exclude(trimmed) {
        return None;
    }

    if matches_any(
        trimmed,
        &[
            r"记住(一下|这个|这件事)?",
            r"以后.*记得",
            r"remember this",
            r"save this to memory",
        ],
    ) {
        return Some(signal(
            MemorySignalKind::ExplicitMemory,
            MemorySignalConfidence::High,
            "preference",
            "user",
            trimmed,
        ));
    }

    if matches_any(
        trimmed,
        &[
            r"我之前说过",
            r"我已经告诉你了",
            r"上次不是说过了吗",
            r"我不是说过",
            r"我提到过",
            r"i already told you",
            r"i mentioned before",
            r"i said before",
            r"we talked about this",
        ],
    ) {
        return Some(signal(
            MemorySignalKind::MissedRecall,
            MemorySignalConfidence::High,
            "preference",
            "user",
            trimmed,
        ));
    }

    if matches_any(
        trimmed,
        &[
            r"你理解错了",
            r"我刚刚说的是",
            r"不是.*而是",
            r"以后不要.*要.*",
        ],
    ) {
        return Some(signal(
            MemorySignalKind::Correction,
            MemorySignalConfidence::High,
            "preference",
            "user",
            trimmed,
        ));
    }

    if matches_any(
        trimmed,
        &[
            r"我喜欢",
            r"我不喜欢",
            r"以后.*回答",
            r"别再",
            r"i prefer",
            r"i don't like",
        ],
    ) {
        return Some(signal(
            MemorySignalKind::Preference,
            MemorySignalConfidence::Medium,
            "preference",
            "user",
            trimmed,
        ));
    }

    if matches_any(
        trimmed,
        &[r"我们约定", r"这个项目.*使用", r"统一.*", r"默认.*"],
    ) {
        return Some(signal(
            MemorySignalKind::Convention,
            MemorySignalConfidence::Medium,
            "convention",
            "project",
            trimmed,
        ));
    }

    if matches_any(trimmed, &[r"你总是", r"又.*错", r"不要再"]) {
        state.frustration_hits = state.frustration_hits.saturating_add(1);
        if state.frustration_hits >= 2 {
            return Some(signal(
                MemorySignalKind::Frustration,
                MemorySignalConfidence::Low,
                "preference",
                "user",
                trimmed,
            ));
        }
    }

    None
}

fn signal(
    kind: MemorySignalKind,
    confidence: MemorySignalConfidence,
    suggested_memory_kind: &str,
    suggested_scope: &str,
    evidence: &str,
) -> MemorySignal {
    MemorySignal {
        kind,
        confidence,
        suggested_memory_kind: suggested_memory_kind.to_string(),
        suggested_scope: suggested_scope.to_string(),
        evidence: evidence.chars().take(240).collect(),
    }
}

fn matches_exclude(input: &str) -> bool {
    matches_any(
        input,
        &[
            r"不要记",
            r"别记",
            r"不要保存",
            r"只这次",
            r"临时",
            r"暂时",
            r"do not remember",
            r"don't remember",
        ],
    )
}

fn matches_any(input: &str, patterns: &[&str]) -> bool {
    let lower = input.to_lowercase();
    patterns.iter().any(|pattern| {
        Regex::new(&format!("(?i){pattern}"))
            .map(|regex| regex.is_match(&lower))
            .unwrap_or(false)
    })
}

fn strip_ignored_regions(input: &str) -> String {
    let mut out = Vec::new();
    let mut in_code_block = false;
    for line in input.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || trimmed.starts_with('>') {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}
