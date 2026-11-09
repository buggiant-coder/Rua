use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    Agent,
    Project,
    Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookBinding {
    pub definition_revision: String,
    pub event: String,
    pub source: SelectionSource,
    #[serde(default)]
    pub configured_order: u32,
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompositionInput {
    #[serde(default)]
    pub agent: Vec<HookBinding>,
    #[serde(default)]
    pub project: Vec<HookBinding>,
    #[serde(default)]
    pub chat: Vec<HookBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedHookBinding {
    pub definition_revision: String,
    pub event: String,
    pub resolved_order: usize,
    pub parameters: BTreeMap<String, serde_json::Value>,
    pub selected_by: Vec<SelectionSource>,
    pub overridden_parameters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompositionWarning {
    pub definition_revision: String,
    pub relationship: String,
    pub missing_target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompositionPreview {
    pub hooks: Vec<ResolvedHookBinding>,
    pub warnings: Vec<CompositionWarning>,
}

pub fn compose(input: CompositionInput) -> Result<CompositionPreview, String> {
    let mut grouped: BTreeMap<String, Vec<HookBinding>> = BTreeMap::new();
    for binding in input
        .agent
        .into_iter()
        .chain(input.project)
        .chain(input.chat)
    {
        grouped
            .entry(binding.definition_revision.clone())
            .or_default()
            .push(binding);
    }

    let mut merged = BTreeMap::new();
    for (identity, mut bindings) in grouped {
        let event = bindings[0].event.clone();
        if bindings.iter().any(|binding| binding.event != event) {
            return Err(format!("event mismatch for {identity}"));
        }
        let mut seen_sources = BTreeSet::new();
        for binding in &bindings {
            if !seen_sources.insert(binding.source) {
                return Err(format!("ambiguous binding for {identity}"));
            }
        }
        bindings.sort_by_key(|binding| binding.source);
        let mut effective = bindings.remove(0);
        let mut selected_by = vec![effective.source];
        let mut overridden = BTreeSet::new();
        for overlay in bindings {
            selected_by.push(overlay.source);
            for (key, value) in overlay.parameters {
                if effective.parameters.insert(key.clone(), value).is_some() {
                    overridden.insert(key);
                }
            }
            effective.before.extend(overlay.before);
            effective.after.extend(overlay.after);
            effective.requires.extend(overlay.requires);
            effective.configured_order = effective.configured_order.min(overlay.configured_order);
        }
        selected_by.sort();
        selected_by.dedup();
        merged.insert(
            identity,
            (
                effective,
                selected_by,
                overridden.into_iter().collect::<Vec<_>>(),
            ),
        );
    }

    let identities = merged.keys().cloned().collect::<BTreeSet<_>>();
    let mut warnings = Vec::new();
    let mut edges: BTreeMap<String, BTreeSet<String>> = identities
        .iter()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    let mut indegree: BTreeMap<String, usize> =
        identities.iter().map(|id| (id.clone(), 0)).collect();
    for (id, (binding, _, _)) in &merged {
        for target in &binding.requires {
            if !identities.contains(target) {
                return Err(format!("missing required dependency {target} for {id}"));
            }
            if merged[target].0.event != binding.event {
                return Err(format!("event mismatch relationship {id} -> {target}"));
            }
        }
        for (relationship, target, from, to) in binding
            .before
            .iter()
            .map(|target| ("before", target, id, target))
            .chain(
                binding
                    .after
                    .iter()
                    .map(|target| ("after", target, target, id)),
            )
        {
            if !identities.contains(target) {
                warnings.push(CompositionWarning {
                    definition_revision: id.clone(),
                    relationship: relationship.to_string(),
                    missing_target: target.clone(),
                });
                continue;
            }
            if merged[target].0.event != binding.event {
                return Err(format!("event mismatch relationship {id} -> {target}"));
            }
            if edges
                .get_mut(from)
                .expect("known source")
                .insert(to.clone())
            {
                *indegree.get_mut(to).expect("known target") += 1;
            }
        }
    }

    let mut ready = merged
        .iter()
        .filter(|(id, _)| indegree[*id] == 0)
        .map(|(id, (binding, _, _))| (binding.configured_order, id.clone()))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::new();
    while let Some((order, id)) = ready.pop_first() {
        let _ = order;
        ordered.push(id.clone());
        for target in edges[&id].clone() {
            let degree = indegree.get_mut(&target).expect("known target");
            *degree -= 1;
            if *degree == 0 {
                ready.insert((merged[&target].0.configured_order, target));
            }
        }
    }
    if ordered.len() != merged.len() {
        return Err("Hook dependency cycle".to_string());
    }

    let hooks = ordered
        .into_iter()
        .enumerate()
        .map(|(resolved_order, id)| {
            let (binding, selected_by, overridden_parameters) = &merged[&id];
            ResolvedHookBinding {
                definition_revision: id,
                event: binding.event.clone(),
                resolved_order,
                parameters: binding.parameters.clone(),
                selected_by: selected_by.clone(),
                overridden_parameters: overridden_parameters.clone(),
            }
        })
        .collect();
    Ok(CompositionPreview { hooks, warnings })
}
