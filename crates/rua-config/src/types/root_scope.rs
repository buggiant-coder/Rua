//! Retired Agent settings are discarded at the root, never inherited by an Agent.
//! Unrecognized extension fields still round-trip unchanged.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

macro_rules! extensions {
    ($module:ident, [$($retired:literal),* $(,)?]) => {
        pub(super) mod $module {
            use super::*;

            fn retained(key: &str) -> bool {
                !matches!(key, $($retired)|*)
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Map<String, Value>, D::Error> {
                let mut fields = Map::<String, Value>::deserialize(deserializer)?;
                fields.retain(|key, _| retained(key));
                Ok(fields)
            }

            pub fn serialize<S: Serializer>(fields: &Map<String, Value>, serializer: S) -> Result<S::Ok, S::Error> {
                fields.iter().filter(|(key, _)| retained(key))
                    .collect::<std::collections::BTreeMap<_, _>>().serialize(serializer)
            }
        }
    };
}

extensions!(
    root,
    [
        "channels",
        "mcp",
        "tools",
        "acp",
        "last_dispatch",
        "show_tool_details",
        "claude_code"
    ]
);
extensions!(
    agents,
    [
        "defaults",
        "running",
        "language",
        "system_prompt_files",
        "heartbeat",
        "active_hours"
    ]
);
extensions!(process_security, ["tool_guard", "file_guard"]);
