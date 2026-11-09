use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketResult {
    pub source: String,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub source_url: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub icon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<BTreeMap<String, StatValue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatValue {
    Int(i64),
    Str(String),
}

impl From<i64> for StatValue {
    fn from(v: i64) -> Self {
        StatValue::Int(v)
    }
}

impl From<String> for StatValue {
    fn from(v: String) -> Self {
        StatValue::Str(v)
    }
}

impl From<&str> for StatValue {
    fn from(v: &str) -> Self {
        StatValue::Str(v.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub key: String,
    pub label: String,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketSearchError {
    pub provider: String,
    pub message: String,
}

impl MarketSearchError {
    pub fn new(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for MarketSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.provider, self.message)
    }
}

impl std::error::Error for MarketSearchError {}
