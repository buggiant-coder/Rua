use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,

    #[serde(default)]
    pub completion_tokens: u64,

    #[serde(default)]
    pub reasoning_tokens: Option<u64>,

    #[serde(default)]
    pub cached_tokens: Option<u64>,

    #[serde(default)]
    pub cache_write_tokens: Option<u64>,

    #[serde(default)]
    pub total_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn new(prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            ..Default::default()
        }
    }

    pub fn total(&self) -> u64 {
        self.total_tokens
            .unwrap_or(self.prompt_tokens + self.completion_tokens)
    }

    pub fn is_empty(&self) -> bool {
        self.prompt_tokens == 0
            && self.completion_tokens == 0
            && self.reasoning_tokens.unwrap_or(0) == 0
            && self.cached_tokens.unwrap_or(0) == 0
            && self.cache_write_tokens.unwrap_or(0) == 0
    }

    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        if let Some(r) = other.reasoning_tokens {
            *self.reasoning_tokens.get_or_insert(0) += r;
        }
        if let Some(c) = other.cached_tokens {
            *self.cached_tokens.get_or_insert(0) += c;
        }
        if let Some(w) = other.cache_write_tokens {
            *self.cache_write_tokens.get_or_insert(0) += w;
        }

        self.total_tokens = None;
    }
}

impl std::ops::Add for TokenUsage {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self.accumulate(&rhs);
        self
    }
}
