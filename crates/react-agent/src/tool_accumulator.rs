use std::collections::HashMap;

use agent_types::{AgentEvent, ToolCall};
use rua_json_repair::{RepairConfig, ToolArgumentsAccumulator};
use tracing::debug;

#[derive(Debug)]
pub struct FinalizeError {
    pub id: String,
    pub tool_name: String,
    pub raw_arguments: String,
    pub error_message: String,
}

impl std::fmt::Display for FinalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid tool arguments for `{}`: {}",
            self.tool_name, self.error_message
        )
    }
}

impl std::error::Error for FinalizeError {}

#[derive(Debug)]
pub struct ToolCallAccumulator {
    accumulators: HashMap<usize, ToolArgumentsAccumulator>,

    ids: HashMap<usize, String>,

    order: Vec<usize>,

    repair_config: RepairConfig,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self {
            accumulators: HashMap::new(),
            ids: HashMap::new(),
            order: Vec::new(),
            repair_config: RepairConfig::tool_call(),
        }
    }

    pub fn with_repair_config(repair_config: RepairConfig) -> Self {
        Self {
            accumulators: HashMap::new(),
            ids: HashMap::new(),
            order: Vec::new(),
            repair_config,
        }
    }

    pub fn push_delta(
        &mut self,
        index: usize,
        id: &str,
        name: Option<&str>,
        arguments_delta: &str,
    ) {
        debug!(
            tool_call_index = index,
            tool_call_id = %id,
            tool_name = ?name,
            delta_len = arguments_delta.len(),
            delta = %arguments_delta,
            "tool_accumulator: push_delta"
        );

        if !id.is_empty() {
            self.ids.insert(index, id.to_string());
        }

        let acc = self.accumulators.entry(index).or_insert_with(|| {
            self.order.push(index);
            let effective_id = self
                .ids
                .get(&index)
                .cloned()
                .unwrap_or_else(|| format!("tc-{index}"));
            ToolArgumentsAccumulator::new(effective_id)
        });
        if let Some(n) = name {
            if !n.is_empty() {
                acc.set_name(n);
            }
        }
        acc.push_fragment(arguments_delta);
    }

    pub fn push_event(&mut self, event: &AgentEvent) -> bool {
        if let AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } = event
        {
            self.push_delta(*index, id, name.as_deref(), arguments_delta);
            true
        } else {
            false
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.accumulators.is_empty()
    }

    pub fn pending_count(&self) -> usize {
        self.accumulators.len()
    }

    pub fn finalize(self) -> Vec<Result<ToolCall, FinalizeError>> {
        debug!(
            pending_count = self.order.len(),
            "tool_accumulator: finalize start"
        );
        let mut results = Vec::new();
        for &idx in &self.order {
            if let Some(acc) = self.accumulators.get(&idx) {
                let tool_name = acc.name.clone().unwrap_or_default();
                let tool_id = self
                    .ids
                    .get(&idx)
                    .cloned()
                    .unwrap_or_else(|| format!("tc-{idx}"));

                let raw_args = acc.raw_arguments();
                debug!(
                    tool_call_index = idx,
                    tool_call_id = %tool_id,
                    tool_name = %tool_name,
                    raw_args_len = raw_args.len(),
                    raw_args = %raw_args,
                    "tool_accumulator: finalizing tool call"
                );

                if raw_args.trim().is_empty() {
                    results.push(Ok(ToolCall {
                        id: tool_id,
                        name: tool_name,
                        arguments: serde_json::Value::Object(serde_json::Map::new()),
                        raw_arguments: Some(raw_args),
                        provider_call_id: None,
                        metadata: serde_json::Map::new(),
                    }));
                    continue;
                }

                match acc.finalize(&self.repair_config) {
                    Ok(outcome) => {
                        results.push(Ok(ToolCall {
                            id: tool_id,
                            name: tool_name,
                            arguments: outcome.value,
                            raw_arguments: Some(raw_args),
                            provider_call_id: None,
                            metadata: serde_json::Map::new(),
                        }));
                    }
                    Err(repair_err) => {
                        results.push(Err(FinalizeError {
                            id: tool_id,
                            tool_name,
                            raw_arguments: raw_args,
                            error_message: repair_err.to_string(),
                        }));
                    }
                }
            }
        }
        results
    }

    pub fn finalize_lenient(self) -> Vec<ToolCall> {
        let mut results = Vec::new();
        for &idx in &self.order {
            if let Some(acc) = self.accumulators.get(&idx) {
                let tool_name = acc.name.clone().unwrap_or_default();
                let tool_id = self
                    .ids
                    .get(&idx)
                    .cloned()
                    .unwrap_or_else(|| format!("tc-{idx}"));

                let raw_args = acc.raw_arguments();

                let arguments = acc
                    .finalize(&self.repair_config)
                    .map(|o| o.value)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));

                results.push(ToolCall {
                    id: tool_id,
                    name: tool_name,
                    arguments,
                    raw_arguments: Some(raw_args),
                    provider_call_id: None,
                    metadata: serde_json::Map::new(),
                });
            }
        }
        results
    }

    pub fn clear(&mut self) {
        self.accumulators.clear();
        self.ids.clear();
        self.order.clear();
    }
}

impl Default for ToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}
