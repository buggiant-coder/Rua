use rua_provider::model_knowledge::{lookup, ReasoningStyle};

#[test]
fn adaptive_claude_4_models_expose_effort_for_anthropic_and_idealab() {
    for provider in ["anthropic", "idealab-claude"] {
        for model in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
        ] {
            let knowledge = lookup(provider, model).expect("adaptive Claude catalog entry");
            assert_eq!(knowledge.reasoning_style, Some(ReasoningStyle::Effort));
            assert_eq!(knowledge.thinking_budget_range, None);
            let expected: &[&str] = if model.ends_with("4-6") {
                &["low", "medium", "high", "max"]
            } else {
                &["low", "medium", "high", "xhigh", "max"]
            };
            assert_eq!(knowledge.reasoning_effort_options, expected);
        }
        assert!(lookup(provider, "claude-haiku-4-5").is_none());
        assert!(lookup(provider, "claude-unknown").is_none());
    }
}

#[test]
fn gpt_5_6_family_exposes_model_specific_effort_options() {
    let sol = lookup("openai-response", "gpt-5.6-sol").unwrap();
    assert_eq!(sol.reasoning_style, Some(ReasoningStyle::Effort));
    assert_eq!(
        sol.reasoning_effort_options,
        ["none", "low", "medium", "high", "xhigh", "max"]
    );

    let luna = lookup("openai-response", "gpt-5.6-luna").unwrap();
    assert_eq!(
        luna.reasoning_effort_options,
        ["minimal", "low", "medium", "high", "xhigh", "max"]
    );

    assert!(lookup("idealab-responses", "gpt-5.6-sol").is_some());
}

#[test]
fn claude_5_family_exposes_effort_options() {
    for model_id in ["claude-sonnet-5", "claude-fable-5", "claude-mythos-5"] {
        let knowledge = lookup("anthropic", model_id).unwrap();
        assert_eq!(
            knowledge.reasoning_style,
            Some(ReasoningStyle::Effort),
            "{model_id}"
        );
        assert_eq!(
            knowledge.reasoning_effort_options,
            ["low", "medium", "high", "xhigh", "max"],
            "{model_id}"
        );
    }

    assert!(lookup("idealab-claude", "claude-sonnet-5").is_some());
}

#[test]
fn glm_5_2_effort_options_cover_dashscope_and_zhipu_providers() {
    for provider_id in ["dashscope", "zhipu-cn", "zhipu-intl-codingplan"] {
        let knowledge = lookup(provider_id, "glm-5.2").unwrap();
        assert_eq!(
            knowledge.reasoning_style,
            Some(ReasoningStyle::Effort),
            "{provider_id}"
        );
        assert_eq!(
            knowledge.reasoning_effort_options,
            ["high", "max"],
            "{provider_id}"
        );
    }
}

#[test]
fn kimi_k3_effort_options_cover_both_kimi_providers() {
    for provider_id in ["kimi-cn", "kimi-intl"] {
        let knowledge = lookup(provider_id, "kimi-k3").unwrap();
        assert_eq!(
            knowledge.reasoning_style,
            Some(ReasoningStyle::Effort),
            "{provider_id}"
        );
        assert_eq!(
            knowledge.reasoning_effort_options,
            ["low", "high", "max"],
            "{provider_id}"
        );
    }
}
