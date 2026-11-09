use async_trait::async_trait;

use crate::types::{Finding, SkillFile};

pub mod pattern;

pub use pattern::{PatternAnalyzer, PatternRule};

#[async_trait]
pub trait Analyzer: Send + Sync {
    fn name(&self) -> &str;
    async fn analyze(&self, files: &mut [SkillFile]) -> Vec<Finding>;
}
