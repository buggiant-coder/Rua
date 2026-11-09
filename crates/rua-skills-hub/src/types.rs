use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubSkillResult {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub icon_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubInstallResult {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("cancelled")]
    Cancelled,

    #[error("HTTP error {status}: {body}")]
    Http { status: u16, body: String },

    #[error("rate limit exceeded")]
    RateLimit,

    #[error("invalid bundle: {0}")]
    InvalidBundle(String),

    #[error("source not supported: {0}")]
    UnsupportedSource(String),

    #[error("zip too large or too many entries")]
    ZipLimit,

    #[error("transport error: {0}")]
    Transport(String),

    #[error("installer error: {0}")]
    Installer(String),
}

impl HubError {
    pub(crate) fn transport(err: reqwest::Error) -> Self {
        HubError::Transport(err.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubSpec {
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleSource {
    SkillsSh(String),

    GitHub(GithubSpec),

    LobeHub(String),

    ModelScope(String),

    SkillsMp(String),

    SkillHub(String),

    Aliyun(String),

    ClawHub(String),

    Direct(String),
}

impl BundleSource {
    pub fn provider_key(&self) -> &str {
        match self {
            Self::SkillsSh(_) => "skills-sh",
            Self::GitHub(_) => "github",
            Self::LobeHub(_) => "lobehub",
            Self::ModelScope(_) => "modelscope",
            Self::SkillsMp(_) => "skillsmp",
            Self::SkillHub(_) => "skillhub",
            Self::Aliyun(_) => "aliyun",
            Self::ClawHub(_) => "clawhub",
            Self::Direct(_) => "url",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawBundlePayload {
    pub payload: serde_json::Value,
    pub source_url: String,
}
