use mcp_manager::types::{AutoConnectPolicy, McpServerConfig, McpServerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformProfile {
    Desktop,

    MobileDefault,

    MobileFull,

    MobileDev,
}

impl PlatformProfile {
    pub fn is_mobile(&self) -> bool {
        !matches!(self, Self::Desktop)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobilePolicyDecision {
    Allow,

    DeferToLazy { reason: String },

    Deny { reason: String },
}

#[derive(Debug, Clone)]
pub struct McpMobilePolicy {
    profile: PlatformProfile,

    max_mobile_subprocesses: usize,

    mobile_idle_timeout_secs: u64,
}

impl McpMobilePolicy {
    pub fn new(profile: PlatformProfile) -> Self {
        Self {
            profile,
            max_mobile_subprocesses: 2,
            mobile_idle_timeout_secs: 300,
        }
    }

    pub fn profile(&self) -> PlatformProfile {
        self.profile
    }

    pub fn mobile_idle_timeout_secs(&self) -> u64 {
        self.mobile_idle_timeout_secs
    }

    pub fn check_auto_connect(
        &self,
        _server_id: &McpServerId,
        config: &McpServerConfig,
    ) -> MobilePolicyDecision {
        if !self.profile.is_mobile() {
            return MobilePolicyDecision::Allow;
        }

        match config.auto_connect {
            AutoConnectPolicy::OnStartup => {
                if config.transport.is_subprocess() {
                    MobilePolicyDecision::DeferToLazy {
                        reason: "stdio MCP servers cannot auto-start on mobile; deferred to lazy connect".to_string(),
                    }
                } else {
                    MobilePolicyDecision::Allow
                }
            }
            AutoConnectPolicy::DisabledOnMobile => MobilePolicyDecision::Deny {
                reason: "server is explicitly disabled on mobile".to_string(),
            },
            AutoConnectPolicy::LazyOnFirstUse | AutoConnectPolicy::Manual => {
                MobilePolicyDecision::Allow
            }
        }
    }

    pub fn check_on_demand_start(
        &self,
        _server_id: &McpServerId,
        config: &McpServerConfig,
        current_subprocess_count: usize,
    ) -> MobilePolicyDecision {
        if !self.profile.is_mobile() {
            return MobilePolicyDecision::Allow;
        }

        if config.auto_connect == AutoConnectPolicy::DisabledOnMobile {
            return MobilePolicyDecision::Deny {
                reason: "server is disabled on mobile".to_string(),
            };
        }

        if config.transport.is_subprocess()
            && current_subprocess_count >= self.max_mobile_subprocesses
        {
            return MobilePolicyDecision::Deny {
                reason: format!(
                    "mobile subprocess limit reached ({}/{})",
                    current_subprocess_count, self.max_mobile_subprocesses
                ),
            };
        }

        MobilePolicyDecision::Allow
    }

    pub fn check_background_reconnect(
        &self,
        _server_id: &McpServerId,
        config: &McpServerConfig,
    ) -> MobilePolicyDecision {
        if !self.profile.is_mobile() {
            return MobilePolicyDecision::Allow;
        }

        if config.transport.is_subprocess() {
            return MobilePolicyDecision::Deny {
                reason: "stdio servers cannot background-reconnect on mobile".to_string(),
            };
        }

        MobilePolicyDecision::Allow
    }

    pub fn effective_auto_connect(&self, config: &McpServerConfig) -> AutoConnectPolicy {
        if !self.profile.is_mobile() {
            return config.auto_connect;
        }

        match config.auto_connect {
            AutoConnectPolicy::OnStartup if config.transport.is_subprocess() => {
                AutoConnectPolicy::LazyOnFirstUse
            }
            AutoConnectPolicy::DisabledOnMobile => AutoConnectPolicy::DisabledOnMobile,
            other => other,
        }
    }
}
