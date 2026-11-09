#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatRouteStatus {
    Active,

    Deprecated,
}

#[derive(Debug, Clone, Copy)]
pub struct CompatRouteEntry {
    pub surface: &'static str,
    pub status: CompatRouteStatus,
    pub compat_routes: &'static [&'static str],
    pub canonical_routes: &'static [&'static str],
    pub retention_reason: &'static str,
    pub exit_condition: &'static str,
}

pub const COMPAT_ROUTE_LEDGER: &[CompatRouteEntry] = &[
    CompatRouteEntry {
        surface: "config_channels",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/config/channels/**"],
        canonical_routes: &["/api/agents/{agentId}/config/channels/**"],
        retention_reason: "Desktop WebUI and Python-compatible channel configuration still include root-scoped calls that bind to the request's extracted/default agent.",
        exit_condition: "Remove after channel settings clients send agent-scoped paths only and contract_channels_http has no root route coverage.",
    },
    CompatRouteEntry {
        surface: "mcp",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/mcp/**"],
        canonical_routes: &["/api/agents/{agentId}/mcp/**"],
        retention_reason: "QwenPaw/Python MCP aliases and older clients use root /mcp routes, including /mcp/clients and /mcp/tools/{client_key}.",
        exit_condition: "Remove after MCP clients use /api/agents/{agentId}/mcp/** and contract_mcp_http no longer asserts QwenPaw root aliases.",
    },
    CompatRouteEntry {
        surface: "mcp_oauth",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/mcp/oauth/**"],
        canonical_routes: &["/api/agents/{agentId}/mcp/oauth/**"],
        retention_reason: "OAuth browser callbacks can originate from pre-agent-scoped MCP flows and must still bind to the extracted/default agent.",
        exit_condition: "Remove after MCP OAuth initiation and callback state always carry agent-scoped routing and oauth_boundary tests cover only agent-scoped paths.",
    },
    CompatRouteEntry {
        surface: "acp",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/acp/**"],
        canonical_routes: &["/api/agents/{agentId}/acp/**"],
        retention_reason: "ACP root routes preserve compatibility with clients created before agent-scoped ACP paths existed.",
        exit_condition: "Remove after ACP clients and contract_acp_http use only /api/agents/{agentId}/acp/**.",
    },
    CompatRouteEntry {
        surface: "files",
        status: CompatRouteStatus::Active,
        compat_routes: &[
            "/api/agent/files/**",
            "/api/agent/subdirs-info",
            "/api/agent/subdirs/**",
            "/api/agent/system-prompt-files/**",
        ],
        canonical_routes: &[
            "/api/agents/{agentId}/files/**",
            "/api/agents/{agentId}/subdirs-info",
            "/api/agents/{agentId}/subdirs/**",
            "/api/agents/{agentId}/system-prompt-files/**",
        ],
        retention_reason: "Python/WebUI file-management calls still use root /agent/* file routes and rely on default/header agent context.",
        exit_condition: "Remove after file-management clients and contract_files_http use only agent-scoped file routes.",
    },
    CompatRouteEntry {
        surface: "cron",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/cron/**"],
        canonical_routes: &["/api/agents/{agentId}/cron/**"],
        retention_reason: "Older cron job management clients use root /cron routes and expect the server to bind jobs to the extracted/default agent workspace.",
        exit_condition: "Remove after cron clients and contract_cron_http use only /api/agents/{agentId}/cron/**.",
    },
    CompatRouteEntry {
        surface: "chats",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/chats/**"],
        canonical_routes: &["/api/agents/{agentId}/chats/**"],
        retention_reason: "Python-compatible chat CRUD paths are root-scoped and existing clients rely on header/default-agent resolution.",
        exit_condition: "Remove after chat clients and contract_chats_http use only /api/agents/{agentId}/chats/**.",
    },
    CompatRouteEntry {
        surface: "chat_prompt",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/chats/prompt-state", "/api/chats/prompt-files/**"],
        canonical_routes: &[
            "/api/agents/{agentId}/chats/*/prompt-state",
            "/api/agents/{agentId}/chats/*/prompt-files/**",
        ],
        retention_reason: "Root prompt-state and prompt-file endpoints resolve the agent from request context while carrying the Chat ID as a query parameter.",
        exit_condition: "Remove after chat prompt clients and contract_chats_http/infra chat prompt coverage use only agent-scoped paths.",
    },
    CompatRouteEntry {
        surface: "chat_search",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/chats/search"],
        canonical_routes: &["/api/agents/{agentId}/chats/search"],
        retention_reason: "Card-index chat search keeps the Python-compatible root search route for older WebUI search clients.",
        exit_condition: "Remove after search clients and contract_chats_http chat_search coverage use only /api/agents/{agentId}/chats/search.",
    },
    CompatRouteEntry {
        surface: "config",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/config"],
        canonical_routes: &["/api/agents/{agentId}/config"],
        retention_reason: "Global config get/put is kept for Python-compatible clients while agent-scoped config remains the canonical per-agent route.",
        exit_condition: "Remove after config clients and contract_config_http use only /api/agents/{agentId}/config or explicit global-only endpoints.",
    },
    CompatRouteEntry {
        surface: "security_approvals",
        status: CompatRouteStatus::Active,
        compat_routes: &[
            "/api/security/request-approval",
            "/api/security/pending-approvals/**",
            "/api/security/approvals/**",
        ],
        canonical_routes: &["/api/approvals/**", "/api/approval-stream/**"],
        retention_reason: "Pre-REST approval clients call security-prefixed approval routes; handlers delegate to the approval authority path.",
        exit_condition: "Remove after approval clients and contract_approval_http use only RESTful approval routes.",
    },
    CompatRouteEntry {
        surface: "dingtalk_webhook",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/dingtalk/webhook/**"],
        canonical_routes: &[
            "/api/channel/dingtalk/webhook/**",
            "/api/channels/dingtalk/webhook/**",
        ],
        retention_reason: "Existing DingTalk robot callback URLs are hard to rotate immediately and must continue to reach the channel webhook adapter.",
        exit_condition: "Remove after deployment docs and all configured DingTalk robot callback URLs use the channel- or channels-prefixed webhook route.",
    },
    CompatRouteEntry {
        surface: "voice_asr",
        status: CompatRouteStatus::Active,
        compat_routes: &["/api/voice/asr"],
        canonical_routes: &[
            "/voice/* channel routes",
            "/api/agents/{agentId}/console/* media flows",
        ],
        retention_reason: "Python-compatible ASR clients call /api/voice/asr while newer voice handling lives outside /api or flows through console media handling.",
        exit_condition: "Remove after contract_voice_http no longer asserts /api/voice/asr and mobile/WebUI voice clients use canonical voice or console media routes.",
    },
    CompatRouteEntry {
        surface: "agent_app",
        status: CompatRouteStatus::Deprecated,
        compat_routes: &["/api/agent/stream_query", "/api/agent/query"],
        canonical_routes: &[
            "/api/agents/{agentId}/chats/{chatId}/console/submit",
            "/api/agents/{agentId}/chats/{chatId}/console/live",
        ],
        retention_reason: "Old AgentApp clients receive an explicit deprecated 404 instead of falling through to an unknown-route response.",
        exit_condition: "Remove after telemetry or release notes confirm no AgentApp callers remain and no client depends on the explicit deprecated 404 shape.",
    },
];
