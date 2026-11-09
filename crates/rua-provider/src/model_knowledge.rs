use crate::config_loader::ProviderModelEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningStyle {
    Effort,
    Budget,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelKnowledge {
    pub provider_ids: &'static [&'static str],
    pub model_id: &'static str,
    pub reasoning_style: Option<ReasoningStyle>,
    pub reasoning_effort_options: &'static [&'static str],
    pub thinking_budget_range: Option<[i64; 2]>,
}

static MODEL_KNOWLEDGE: &[ModelKnowledge] = &[
    ModelKnowledge {
        provider_ids: OPENAI_PROVIDER_IDS,
        model_id: "gpt-5.6",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["none", "low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: OPENAI_PROVIDER_IDS,
        model_id: "gpt-5.6-sol",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["none", "low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: OPENAI_PROVIDER_IDS,
        model_id: "gpt-5.6-terra",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["none", "low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: OPENAI_PROVIDER_IDS,
        model_id: "gpt-5.6-luna",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["minimal", "low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-sonnet-5",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-opus-4-8",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-opus-4-7",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-opus-4-6",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-sonnet-4-6",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-fable-5",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: ANTHROPIC_PROVIDER_IDS,
        model_id: "claude-mythos-5",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high", "xhigh", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: KIMI_PROVIDER_IDS,
        model_id: "kimi-k3",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "high", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "qwen3.5-plus",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([1, 81920]),
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "qwen3.6-flash",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([1, 81920]),
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "qwen3.6-plus",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([1, 81920]),
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "qwen3.7-plus",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([1, 81920]),
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "qwen3.7-max",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([1, 81920]),
    },
    ModelKnowledge {
        provider_ids: GLM_5_2_PROVIDER_IDS,
        model_id: "glm-5.2",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["high", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: DASHSCOPE_FAMILY_PROVIDER_IDS,
        model_id: "deepseek-v4-pro",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["high", "max"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: &["google", "idealab-gemini"],
        model_id: "gemini-3.1-pro-preview",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["low", "medium", "high"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: &["google", "idealab-gemini"],
        model_id: "gemini-3-flash-preview",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["minimal", "low", "medium", "high"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: &["google", "idealab-gemini"],
        model_id: "gemini-3.5-flash",
        reasoning_style: Some(ReasoningStyle::Effort),
        reasoning_effort_options: &["minimal", "low", "medium", "high"],
        thinking_budget_range: None,
    },
    ModelKnowledge {
        provider_ids: &["google"],
        model_id: "gemini-2.5-pro",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([128, 32768]),
    },
    ModelKnowledge {
        provider_ids: &["google"],
        model_id: "gemini-2.5-flash",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([0, 24576]),
    },
    ModelKnowledge {
        provider_ids: &["google"],
        model_id: "gemini-2.5-flash-lite",
        reasoning_style: Some(ReasoningStyle::Budget),
        reasoning_effort_options: &[],
        thinking_budget_range: Some([0, 24576]),
    },
];

const OPENAI_PROVIDER_IDS: &[&str] = &["openai", "openai-response", "idealab", "idealab-responses"];

const ANTHROPIC_PROVIDER_IDS: &[&str] = &["anthropic", "idealab-claude"];

const KIMI_PROVIDER_IDS: &[&str] = &["kimi-cn", "kimi-intl"];

const DASHSCOPE_FAMILY_PROVIDER_IDS: &[&str] = &[
    "dashscope",
    "aliyun-codingplan",
    "aliyun-codingplan-intl",
    "aliyun-tokenplan",
    "aliyun-tokenplan-intl",
];

const GLM_5_2_PROVIDER_IDS: &[&str] = &[
    "dashscope",
    "aliyun-codingplan",
    "aliyun-codingplan-intl",
    "aliyun-tokenplan",
    "aliyun-tokenplan-intl",
    "zhipu-cn",
    "zhipu-cn-codingplan",
    "zhipu-intl",
    "zhipu-intl-codingplan",
];

pub fn lookup(provider_id: &str, model_id: &str) -> Option<&'static ModelKnowledge> {
    let provider_id = provider_id.trim();
    let model_id = model_id.trim();
    MODEL_KNOWLEDGE
        .iter()
        .find(|entry| entry.model_id == model_id && entry.provider_ids.contains(&provider_id))
}

pub fn apply_missing_defaults(provider_id: &str, model: &mut ProviderModelEntry) -> bool {
    let mut changed = false;
    if let Some(knowledge) = lookup(provider_id, &model.id) {
        if model.thinking_param_style.is_none() {
            model.thinking_param_style = knowledge.reasoning_style.map(|style| match style {
                ReasoningStyle::Effort => "effort".to_string(),
                ReasoningStyle::Budget => "budget".to_string(),
            });
            changed |= model.thinking_param_style.is_some();
        }
        if model.reasoning_effort_options.is_empty()
            && !knowledge.reasoning_effort_options.is_empty()
        {
            model.reasoning_effort_options = knowledge
                .reasoning_effort_options
                .iter()
                .map(|option| (*option).to_string())
                .collect();
            changed = true;
        }
        if model.thinking_budget_range.is_none() {
            model.thinking_budget_range = knowledge.thinking_budget_range;
            changed |= model.thinking_budget_range.is_some();
        }
    }

    let provider = crate::catalog::provider(provider_id);
    let catalog_model =
        provider.and_then(|provider| provider.models.iter().find(|entry| entry.id == model.id));
    if let Some(source) = catalog_model {
        if model.thinking_enabled.is_none() && source.thinking_enabled.is_some() {
            model.thinking_enabled = source.thinking_enabled;
            changed = true;
        }
        if model.thinking_budget.is_none() && source.thinking_budget.is_some() {
            model.thinking_budget = source.thinking_budget;
            changed = true;
        }
        if model.reasoning_effort.is_none() && source.reasoning_effort.is_some() {
            model.reasoning_effort = source.reasoning_effort.clone();
            changed = true;
        }
        if model.thinking_param_style.is_none() {
            model.thinking_param_style = source
                .thinking_param_style
                .clone()
                .or_else(|| (!source.reasoning_effort_options.is_empty()).then(|| "effort".into()));
            if model.thinking_param_style.is_none() {
                let protocol = provider.and_then(|provider| provider.chat_model.as_deref());
                model.thinking_param_style = match protocol {
                    Some("DashScopeChatModel") => Some("budget".into()),
                    Some("OpenAIChatModel" | "OpenAIResponseModel")
                        if is_openai_reasoning_model(&model.id) =>
                    {
                        Some("effort".into())
                    }
                    _ if source.thinking_enabled.is_some() => Some("effort".into()),
                    _ => None,
                };
            }
            changed |= model.thinking_param_style.is_some();
        }
        if model.reasoning_effort_options.is_empty() && !source.reasoning_effort_options.is_empty()
        {
            model.reasoning_effort_options = source.reasoning_effort_options.clone();
            changed = true;
        }
        if model.thinking_param_style.as_deref() == Some("budget")
            && model.thinking_budget_range.is_none()
        {
            model.thinking_budget_range = source
                .thinking_budget_range
                .or_else(|| provider.and_then(|provider| provider.thinking_budget_range));
            changed |= model.thinking_budget_range.is_some();
        }
    }

    // Knowledge is materialized at configuration ingress; adapters only read
    // these explicit values and never consult the catalog during a model call.
    if model.thinking_param_style.as_deref() == Some("effort")
        && model.reasoning_effort_options.is_empty()
        && is_openai_reasoning_model(&model.id)
    {
        model.reasoning_effort_options = vec!["low".into(), "medium".into(), "high".into()];
        let leaf = model.id.rsplit('/').next().unwrap_or(&model.id);
        if crate::catalog::rules()
            .openai_none_effort_models
            .iter()
            .any(|id| id == leaf)
        {
            model.reasoning_effort_options.insert(0, "none".into());
        } else if matches!(leaf, "gpt-5" | "gpt-5-mini" | "gpt-5-nano") {
            model.reasoning_effort_options.insert(0, "minimal".into());
        }
        changed = true;
    }
    if model.thinking_param_style.is_some() && model.thinking_enabled.is_none() {
        model.thinking_enabled = Some(true);
        changed = true;
    }
    if model.thinking_enabled == Some(true) {
        match model.thinking_param_style.as_deref() {
            Some("effort") if model.reasoning_effort.is_none() => {
                model.reasoning_effort = model
                    .reasoning_effort_options
                    .iter()
                    .find(|option| option.as_str() == "medium")
                    .or_else(|| {
                        model
                            .reasoning_effort_options
                            .iter()
                            .find(|option| !matches!(option.as_str(), "none" | "minimal"))
                    })
                    .cloned();
                changed |= model.reasoning_effort.is_some();
            }
            Some("budget") if model.thinking_budget.is_none() => {
                model.thinking_budget = crate::catalog::rules()
                    .reasoning_budgets
                    .get("medium")
                    .copied()
                    .map(|budget| match model.thinking_budget_range {
                        Some([min, max]) if min <= max => budget.clamp(min, max),
                        _ => budget,
                    });
                changed |= model.thinking_budget.is_some();
            }
            _ => {}
        }
    }
    if model.thinking_param_style.is_some() && !model.extra.contains_key("reasoning_format") {
        let format = match provider_id {
            "openrouter" => "openrouter",
            "kimi-cn" | "kimi-intl" if model.id == "kimi-k3" => "effort",
            "kimi-cn"
            | "kimi-intl"
            | "kimi-codingplan"
            | "zhipu-cn"
            | "zhipu-cn-codingplan"
            | "zhipu-intl"
            | "zhipu-intl-codingplan"
            | "deepseek"
            | "mimo"
            | "mimo-tokenplan"
            | "volcengine-cn"
            | "volcengine-cn-codingplan"
            | "volcengine-cn-agentplan" => "thinking",
            "dashscope"
            | "aliyun-codingplan"
            | "aliyun-codingplan-intl"
            | "aliyun-tokenplan"
            | "aliyun-tokenplan-intl" => "dashscope",
            _ => "protocol",
        };
        model
            .extra
            .insert("reasoning_format".into(), serde_json::json!(format));
        changed = true;
    }
    if model.thinking_param_style.is_some()
        && (is_openai_reasoning_model(&model.id)
            || (matches!(provider_id, "kimi-cn" | "kimi-intl") && model.id == "kimi-k3"))
        && !model.extra.contains_key("reasoning_off_effort")
    {
        let leaf = model
            .id
            .rsplit('/')
            .next()
            .unwrap_or(&model.id)
            .to_lowercase();
        let effort = if !model.reasoning_effort_options.is_empty() {
            model.reasoning_effort_options[0].as_str()
        } else if crate::catalog::rules()
            .openai_none_effort_models
            .contains(&leaf)
        {
            "none"
        } else if leaf.starts_with("gpt-5") {
            "minimal"
        } else {
            "low"
        };
        model
            .extra
            .insert("reasoning_off_effort".into(), serde_json::json!(effort));
        changed = true;
    }
    if matches!(provider_id, "google" | "idealab-gemini") {
        if model.thinking_param_style.as_deref() == Some("effort") {
            if let Some(effort) = model.reasoning_effort_options.first() {
                if !model.extra.contains_key("reasoning_off_effort") {
                    model
                        .extra
                        .insert("reasoning_off_effort".into(), serde_json::json!(effort));
                    changed = true;
                }
            }
        } else if model.id == "gemini-2.5-pro" && !model.extra.contains_key("reasoning_off_budget")
        {
            model
                .extra
                .insert("reasoning_off_budget".into(), serde_json::json!(128));
            changed = true;
        }
    }

    changed
}

fn is_openai_reasoning_model(id: &str) -> bool {
    let name = id.rsplit('/').next().unwrap_or(id).to_lowercase();
    name.starts_with("gpt-5")
        || (name.starts_with('o') && name.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
}
