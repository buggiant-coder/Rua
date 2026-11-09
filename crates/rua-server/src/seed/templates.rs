use std::path::Path;

use rua_core::{Result, RuaError};

use super::{SeedCoordinator, SeedProducer, SeedRunReport};

pub fn ensure_builtin_templates_seeded(data_dir: &Path) -> SeedRunReport {
    SeedCoordinator::new(data_dir)
        .with_producer(Box::new(BuiltinTemplateSeedProducer))
        .run()
}

struct BuiltinTemplateSeedProducer;

impl SeedProducer for BuiltinTemplateSeedProducer {
    fn entry_id(&self) -> String {
        "agent_templates:builtin".into()
    }
    fn producer_name(&self) -> &str {
        "agent_template_seed"
    }
    fn target_paths(&self) -> Vec<String> {
        vec!["agent_templates.json".into()]
    }
    fn materialize(&self, data_dir: &Path) -> Result<Vec<String>> {
        rua_config::AgentTemplateStore::new(data_dir.to_path_buf())
            .save_file(&rua_config::agent_template::builtin_templates())
            .map_err(|error| RuaError::Io {
                message: format!("seed Agent templates: {error}"),
            })?;
        Ok(self.target_paths())
    }
}
