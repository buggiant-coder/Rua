pub struct BuiltinPrompt {
    pub path: &'static str,
    pub content: &'static str,
}

pub static BUILTIN_PROMPTS: &[BuiltinPrompt] = &[
    BuiltinPrompt {
        path: "10_AGENT_SAFETY.md",
        content: include_str!("builtin/10_AGENT_SAFETY.md"),
    },
    BuiltinPrompt {
        path: "20_AGENT_WORKFLOW.md",
        content: include_str!("builtin/20_AGENT_WORKFLOW.md"),
    },
    BuiltinPrompt {
        path: "30_EXECUTION_REPORTING.md",
        content: include_str!("builtin/30_EXECUTION_REPORTING.md"),
    },
    BuiltinPrompt {
        path: "40_STRUCTURED_MEMORY.md",
        content: include_str!("builtin/40_STRUCTURED_MEMORY.md"),
    },
    BuiltinPrompt {
        path: "50_AGENT_RESPONSE.md",
        content: include_str!("builtin/50_AGENT_RESPONSE.md"),
    },
    BuiltinPrompt {
        path: "60_MARKDOWN_PLUS.md",
        content: include_str!("builtin/60_MARKDOWN_PLUS.md"),
    },
    BuiltinPrompt {
        path: "90_DEFAULT_PROFILE.md",
        content: include_str!("builtin/90_DEFAULT_PROFILE.md"),
    },
    BuiltinPrompt {
        path: "91_QA_PROFILE.md",
        content: include_str!("builtin/91_QA_PROFILE.md"),
    },
    BuiltinPrompt {
        path: "92_QA_WORKFLOW.md",
        content: include_str!("builtin/92_QA_WORKFLOW.md"),
    },
    BuiltinPrompt {
        path: "93_RESEARCH_PROFILE.md",
        content: include_str!("builtin/93_RESEARCH_PROFILE.md"),
    },
];
