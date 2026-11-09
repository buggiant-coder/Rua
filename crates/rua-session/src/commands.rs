use std::path::PathBuf;

pub mod skills_handler;

pub use skills_handler::{handle_skills_command, SkillsCommandHandler};

pub struct SkillsContext {
    pub workspace_dir: Option<PathBuf>,
    pub channel_id: String,
    pub locale: String,
}

impl SkillsContext {
    pub fn empty() -> Self {
        Self {
            workspace_dir: None,
            channel_id: "console".to_string(),
            locale: "en".to_string(),
        }
    }
}
