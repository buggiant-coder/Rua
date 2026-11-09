use crate::config::RepairConfig;
use crate::error::RepairError;
use crate::outcome::RepairOutcome;
use crate::repair::repair_json_with_config;

#[derive(Debug, Clone)]
pub struct ToolArgumentsAccumulator {
    pub call_id: String,

    pub name: Option<String>,

    pub raw_fragments: Vec<String>,
}

impl ToolArgumentsAccumulator {
    pub fn new(call_id: String) -> Self {
        Self {
            call_id,
            name: None,
            raw_fragments: Vec::new(),
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = Some(name.to_string());
    }

    pub fn push_fragment(&mut self, fragment: &str) {
        if !fragment.is_empty() {
            self.raw_fragments.push(fragment.to_string());
        }
    }

    pub fn raw_arguments(&self) -> String {
        self.raw_fragments.concat()
    }

    pub fn finalize(&self, config: &RepairConfig) -> Result<RepairOutcome, RepairError> {
        let raw = self.raw_arguments();
        repair_json_with_config(&raw, config)
    }

    pub fn is_empty(&self) -> bool {
        self.raw_fragments.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.raw_fragments.iter().map(|f| f.len()).sum()
    }
}
