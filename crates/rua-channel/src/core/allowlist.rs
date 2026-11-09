use std::collections::HashSet;

use crate::core::config::BaseChannelConfig;

#[derive(Debug, Clone, PartialEq)]
pub enum AllowlistMode {
    AllowAll,

    Allowlist,

    Denylist,
}

impl AllowlistMode {
    fn from_str_lossy(s: &str) -> Self {
        match s {
            "allowlist" => Self::Allowlist,
            "denylist" => Self::Denylist,
            _ => Self::AllowAll,
        }
    }
}

pub struct AllowlistPolicy {
    mode: AllowlistMode,
    entries: HashSet<String>,
}

impl AllowlistPolicy {
    pub fn allow_all() -> Self {
        Self {
            mode: AllowlistMode::AllowAll,
            entries: HashSet::new(),
        }
    }

    pub fn allowlist(entries: impl IntoIterator<Item = String>) -> Self {
        Self {
            mode: AllowlistMode::Allowlist,
            entries: entries.into_iter().collect(),
        }
    }

    pub fn denylist(entries: impl IntoIterator<Item = String>) -> Self {
        Self {
            mode: AllowlistMode::Denylist,
            entries: entries.into_iter().collect(),
        }
    }

    pub fn is_allowed(&self, sender_id: &str) -> bool {
        match self.mode {
            AllowlistMode::AllowAll => true,
            AllowlistMode::Allowlist => self.entries.contains(sender_id),
            AllowlistMode::Denylist => !self.entries.contains(sender_id),
        }
    }

    pub fn mode(&self) -> &AllowlistMode {
        &self.mode
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub struct ChannelAcl {
    dm: AllowlistPolicy,
    group: AllowlistPolicy,
}

impl ChannelAcl {
    pub fn from_config(cfg: &BaseChannelConfig) -> Self {
        let dm_mode = AllowlistMode::from_str_lossy(&cfg.dm_policy);
        let group_mode = AllowlistMode::from_str_lossy(&cfg.group_policy);

        let dm = Self::build_policy(dm_mode, &cfg.allow_from, &cfg.deny_from);
        let group = Self::build_policy(group_mode, &cfg.allow_from, &cfg.deny_from);

        Self { dm, group }
    }

    fn build_policy(
        mode: AllowlistMode,
        allow_from: &[String],
        deny_from: &[String],
    ) -> AllowlistPolicy {
        match mode {
            AllowlistMode::AllowAll => AllowlistPolicy::allow_all(),
            AllowlistMode::Allowlist => AllowlistPolicy::allowlist(allow_from.iter().cloned()),
            AllowlistMode::Denylist => AllowlistPolicy::denylist(deny_from.iter().cloned()),
        }
    }

    pub fn is_allowed(&self, sender_id: &str, is_group: bool) -> bool {
        if is_group {
            self.group.is_allowed(sender_id)
        } else {
            self.dm.is_allowed(sender_id)
        }
    }
}
