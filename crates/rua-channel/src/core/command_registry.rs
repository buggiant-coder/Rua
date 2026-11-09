use std::{cmp::Reverse, collections::HashMap};

use tracing::info;

pub struct CommandRegistry {
    command_to_level: HashMap<String, u32>,
    default_level: u32,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            command_to_level: HashMap::new(),
            default_level: 20,
        };
        registry.register_defaults();
        info!("CommandRegistry initialized");
        registry
    }

    fn register_defaults(&mut self) {
        self.register_command("/stop", 0);

        for cmd in [
            "/daemon status",
            "/daemon restart",
            "/daemon reload-config",
            "/daemon version",
            "/daemon logs",
            "/daemon approve",
            "/daemon deny",
            "/status",
            "/restart",
            "/reload-config",
            "/reload_config",
            "/version",
            "/logs",
            "/approve",
            "/deny",
        ] {
            self.register_command(cmd, 10);
        }
    }

    pub fn register_command(&mut self, command_prefix: &str, priority_level: u32) {
        let prefix_lower = command_prefix.to_lowercase();
        self.command_to_level.insert(prefix_lower, priority_level);
        info!(
            "Registered command: {} → level={}",
            command_prefix, priority_level
        );
    }

    pub fn is_control_command(&self, query: &str) -> bool {
        let query_lower = query.trim().to_lowercase();
        if !query_lower.starts_with('/') {
            return false;
        }
        self.find_matching_prefix(&query_lower).is_some()
    }

    pub fn get_priority_level(&self, query: &str) -> u32 {
        let query_lower = query.trim().to_lowercase();
        if !query_lower.starts_with('/') {
            return self.default_level;
        }
        match self.find_matching_prefix(&query_lower) {
            Some(level) => level,
            None => self.default_level,
        }
    }

    fn find_matching_prefix(&self, query_lower: &str) -> Option<u32> {
        let mut prefixes: Vec<_> = self.command_to_level.iter().collect();
        prefixes.sort_by_key(|b| Reverse(b.0.len()));

        for (prefix, &level) in prefixes {
            if query_lower.starts_with(prefix.as_str()) {
                let next_idx = prefix.len();
                if next_idx >= query_lower.len() {
                    return Some(level);
                }
                let next_char = query_lower.as_bytes()[next_idx];
                if matches!(next_char, b' ' | b'\t' | b'\n') {
                    return Some(level);
                }
            }
        }
        None
    }
}
