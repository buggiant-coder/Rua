use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("invalid SKILL.md frontmatter: {0}")]
    InvalidFrontmatter(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("invalid skill name: {0}")]
    InvalidSkillName(String),

    #[error("skill conflict: {0}")]
    Conflict(String),

    #[error("skill source conflict: skill '{skill_name}' owned by {existing_source}, cannot be claimed by {requested_source}")]
    SourceConflict {
        skill_name: String,
        existing_source: String,
        requested_source: String,
    },

    #[error("skill consistency error: {0}")]
    Consistency(String),

    #[error("skill '{skill_name}' blocked by security scan ({max_severity}): {summary}")]
    ScanBlocked {
        skill_name: String,
        max_severity: String,
        summary: String,
        findings: Vec<String>,
    },
}
