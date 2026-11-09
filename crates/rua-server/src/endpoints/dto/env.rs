use std::collections::BTreeMap;

use crate::endpoints::dto::{EnvVar, EnvVarsList};

pub fn env_map_to_list(envs: BTreeMap<String, String>) -> EnvVarsList {
    envs.into_iter()
        .map(|(key, value)| EnvVar {
            key,
            value,
            masked: None,
            extra: serde_json::Map::new(),
        })
        .collect()
}

pub fn list_to_env_map(envs: &EnvVarsList) -> BTreeMap<String, String> {
    envs.iter()
        .map(|env| (env.key.clone(), env.value.clone()))
        .collect()
}
