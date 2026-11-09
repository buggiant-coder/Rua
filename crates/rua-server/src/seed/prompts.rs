use std::path::Path;

use rua_core::Result;

use super::{SeedCoordinator, SeedProducer, SeedRunReport};

const PROMPT_LIBRARY_ENTRY_ID: &str = "prompt_library:builtin:v2";
const PROMPT_LIBRARY_PRODUCER: &str = "builtin_prompt_library";
pub fn ensure_builtin_prompt_library_seeded(data_dir: &Path) -> SeedRunReport {
    SeedCoordinator::new(data_dir)
        .with_producer(Box::new(BuiltinPromptLibrarySeedProducer))
        .run()
}

struct BuiltinPromptLibrarySeedProducer;

impl SeedProducer for BuiltinPromptLibrarySeedProducer {
    fn entry_id(&self) -> String {
        PROMPT_LIBRARY_ENTRY_ID.to_string()
    }

    fn producer_name(&self) -> &str {
        PROMPT_LIBRARY_PRODUCER
    }

    fn target_paths(&self) -> Vec<String> {
        Vec::new()
    }

    fn materialize(&self, data_dir: &Path) -> Result<Vec<String>> {
        let lib = rua_prompts::PromptLibrary::new(data_dir.to_path_buf());
        let result = lib.restore_builtin_now()?;
        Ok(result
            .seeded
            .into_iter()
            .map(|entry| entry.relative_path)
            .collect())
    }
}
