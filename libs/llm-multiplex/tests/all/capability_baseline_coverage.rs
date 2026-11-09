use llm_multiplex::capability_baseline::CapabilityBaseline;
#[test]
fn baseline_has_at_least_35_entries() {
    let ids = CapabilityBaseline::all_model_ids();
    assert!(
        ids.len() >= 35,
        "expected ≥35 baseline entries, got {}",
        ids.len()
    );
}

#[test]
fn lookup_key_models_hit() {
    let key_models = [
        "gpt-4o",
        "gpt-4.1",
        "gpt-4.1-mini",
        "o3",
        "o4-mini",
        "claude-3-5-sonnet-20241022",
        "claude-sonnet-4-20250514",
        "claude-opus-4-20250514",
        "claude-haiku-4-5-20251001",
        "gemini-2.0-flash",
        "gemini-2.5-pro",
        "gemini-2.5-flash",
        "qwen-max",
        "qwen-turbo",
        "kimi-for-coding",
        "mimo-v2.5",
        "deepseek-v4-flash-free",
        "nemotron-3-ultra-free",
        "deepseek-chat",
        "deepseek-reasoner",
        "grok-3",
    ];
    for model_id in &key_models {
        assert!(
            CapabilityBaseline::lookup(model_id).is_some(),
            "baseline lookup miss for '{}'",
            model_id
        );
    }
}

#[test]
fn unknown_model_returns_none() {
    assert!(CapabilityBaseline::lookup("nonexistent-model-xyz-999").is_none());
}

#[test]
fn kimi_coding_plan_is_text_only_baseline() {
    let info = CapabilityBaseline::lookup("kimi-for-coding").unwrap();

    assert_eq!(info.name, "Kimi for Coding");
    assert_eq!(info.context_window, Some(256_000));
    assert_eq!(info.max_output_tokens, Some(16_384));
    assert_eq!(info.supports_image, Some(false));
    assert_eq!(info.supports_video, Some(false));
    assert_eq!(info.supports_multimodal, Some(false));
    assert_eq!(info.probe_source, Some("baseline".into()));
}

#[test]
fn mimo_token_plan_preserves_model_specific_multimodal_flags() {
    let pro = CapabilityBaseline::lookup("mimo-v2.5-pro").unwrap();
    let standard = CapabilityBaseline::lookup("mimo-v2.5").unwrap();

    assert_eq!(pro.name, "MiMo V2.5 Pro");
    assert_eq!(pro.supports_image, Some(false));
    assert_eq!(pro.supports_video, Some(false));
    assert_eq!(pro.supports_multimodal, Some(false));

    assert_eq!(standard.name, "MiMo V2.5");
    assert_eq!(standard.context_window, Some(131_072));
    assert_eq!(standard.max_output_tokens, Some(8_192));
    assert_eq!(standard.supports_image, Some(true));
    assert_eq!(standard.supports_video, Some(true));
    assert_eq!(standard.supports_multimodal, Some(true));
}

#[test]
fn provider_specific_lookup_overrides_same_model_global_capability() {
    let kimi = CapabilityBaseline::lookup_for_provider("kimi-cn", "kimi-k2.6").unwrap();
    let volcengine =
        CapabilityBaseline::lookup_for_provider("volcengine-cn-codingplan", "kimi-k2.6").unwrap();

    assert_eq!(kimi.supports_image, Some(true));
    assert_eq!(kimi.supports_video, Some(true));
    assert_eq!(volcengine.name, "Kimi-K2.6");
    assert_eq!(volcengine.supports_image, Some(false));
    assert_eq!(volcengine.supports_video, Some(false));
    assert_eq!(volcengine.supports_multimodal, Some(false));
}

#[test]
fn provider_specific_fuzzy_lookup_wins_before_global_fuzzy_lookup() {
    let info = CapabilityBaseline::lookup_fuzzy_for_provider(
        "volcengine-cn-codingplan",
        "kimi-k2.6-2026-06-01",
    )
    .unwrap();

    assert_eq!(info.id, "kimi-k2.6-2026-06-01");
    assert_eq!(info.name, "Kimi-K2.6");
    assert_eq!(info.supports_image, Some(false));
    assert_eq!(info.supports_video, Some(false));
    assert_eq!(info.supports_multimodal, Some(false));
    assert_eq!(info.probe_source.as_deref(), Some("baseline-fuzzy"));
}

#[test]
fn volcengine_preset_models_have_documented_capability_baselines() {
    let multimodal =
        CapabilityBaseline::lookup_for_provider("volcengine-cn", "doubao-seed-2-0-pro-260215")
            .unwrap();
    let text_only =
        CapabilityBaseline::lookup_for_provider("volcengine-cn", "doubao-seed-character-251128")
            .unwrap();

    assert_eq!(multimodal.supports_image, Some(true));
    assert_eq!(multimodal.supports_video, Some(true));
    assert_eq!(multimodal.supports_multimodal, Some(true));
    assert_eq!(text_only.supports_image, Some(false));
    assert_eq!(text_only.supports_video, Some(false));
    assert_eq!(text_only.supports_multimodal, Some(false));
}

#[test]
fn opencode_free_tier_models_are_text_only_baselines() {
    for (model_id, expected_name) in [
        ("deepseek-v4-flash-free", "DeepSeek V4 Flash"),
        ("mimo-v2.5-free", "Mimo V2.5"),
        ("nemotron-3-ultra-free", "Nemotron 3 Ultra"),
        ("nemotron-3-super-free", "Nemotron 3 Super"),
    ] {
        let info = CapabilityBaseline::lookup_for_provider("opencode", model_id).unwrap();
        assert_eq!(info.name, expected_name);
        assert_eq!(info.context_window, Some(131_072));
        assert_eq!(info.max_output_tokens, Some(8_192));
        assert_eq!(info.supports_image, Some(false));
        assert_eq!(info.supports_video, Some(false));
        assert_eq!(info.supports_multimodal, Some(false));
        assert_eq!(info.probe_source, Some("baseline".into()));
    }
}

#[test]
fn kilo_free_tier_models_are_text_only_baselines() {
    for (model_id, expected_name) in [
        ("kilo-auto/free", "Kilo Auto (Free Router)"),
        (
            "nvidia/nemotron-3-ultra-550b-a55b:free",
            "Nemotron 3 Ultra 550B",
        ),
        (
            "nvidia/nemotron-3-super-120b-a12b:free",
            "Nemotron 3 Super 120B",
        ),
        ("poolside/laguna-m.1:free", "Poolside Laguna M.1"),
        ("poolside/laguna-xs.2:free", "Poolside Laguna XS.2"),
        ("stepfun/step-3.7-flash:free", "Step 3.7 Flash"),
        ("nex-agi/nex-n2-pro:free", "Nex N2 Pro"),
    ] {
        let info = CapabilityBaseline::lookup_for_provider("kilo", model_id).unwrap();
        assert_eq!(info.name, expected_name);
        assert_eq!(info.context_window, Some(131_072));
        assert_eq!(info.max_output_tokens, Some(8_192));
        assert_eq!(info.supports_image, Some(false));
        assert_eq!(info.supports_video, Some(false));
        assert_eq!(info.supports_multimodal, Some(false));
        assert_eq!(info.probe_source, Some("baseline".into()));
    }
}

#[test]
fn zhipu_glm_templates_share_documented_baselines_across_regions_and_plans() {
    for provider_id in [
        "zhipu-cn",
        "zhipu-cn-codingplan",
        "zhipu-intl",
        "zhipu-intl-codingplan",
    ] {
        let text = CapabilityBaseline::lookup_for_provider(provider_id, "glm-5").unwrap();
        assert_eq!(text.name, "GLM-5", "{provider_id}");
        assert_eq!(text.context_window, Some(128_000), "{provider_id}");
        assert_eq!(text.max_output_tokens, Some(8_192), "{provider_id}");
        assert_eq!(text.supports_image, Some(false), "{provider_id}");
        assert_eq!(text.supports_video, Some(false), "{provider_id}");
        assert_eq!(text.supports_multimodal, Some(false), "{provider_id}");

        let vision = CapabilityBaseline::lookup_for_provider(provider_id, "glm-5v-turbo").unwrap();
        assert_eq!(vision.name, "GLM-5V Turbo", "{provider_id}");
        assert_eq!(vision.supports_image, Some(true), "{provider_id}");
        assert_eq!(vision.supports_video, Some(false), "{provider_id}");
        assert_eq!(vision.supports_multimodal, Some(true), "{provider_id}");
    }
}

#[test]
fn fuzzy_lookup_dated_variant() {
    let info = CapabilityBaseline::lookup_fuzzy("gpt-4o-2024-11-20").unwrap();
    assert_eq!(info.context_window, Some(128_000));
    assert_eq!(info.probe_source, Some("baseline-fuzzy".into()));
}

#[test]
fn fuzzy_lookup_uses_longest_matching_prefix() {
    let info = CapabilityBaseline::lookup_fuzzy("gpt-4.1-nano-2025-04-14").unwrap();

    assert_eq!(info.id, "gpt-4.1-nano-2025-04-14");
    assert_eq!(
        info.max_output_tokens,
        Some(16_384),
        "dated nano variants must inherit gpt-4.1-nano, not the shorter gpt-4.1 baseline"
    );
    assert_eq!(info.probe_source, Some("baseline-fuzzy".into()));
}

#[test]
fn gpt_5_6_family_has_documented_capabilities() {
    for (model_id, expected_context) in [
        ("gpt-5.6", 1_050_000),
        ("gpt-5.6-sol", 1_050_000),
        ("gpt-5.6-terra", 1_050_000),
        ("gpt-5.6-luna", 400_000),
    ] {
        let info = CapabilityBaseline::lookup(model_id).unwrap();
        assert_eq!(info.context_window, Some(expected_context), "{model_id}");
        assert_eq!(info.max_output_tokens, Some(128_000), "{model_id}");
        assert_eq!(info.supports_image, Some(true), "{model_id}");
        assert_eq!(info.supports_video, Some(false), "{model_id}");
        assert_eq!(info.supports_multimodal, Some(true), "{model_id}");
    }
}

#[test]
fn claude_5_family_has_documented_capabilities() {
    for model_id in ["claude-sonnet-5", "claude-fable-5", "claude-mythos-5"] {
        let info = CapabilityBaseline::lookup_for_provider("anthropic", model_id).unwrap();
        assert_eq!(info.context_window, Some(1_000_000), "{model_id}");
        assert_eq!(info.max_output_tokens, Some(128_000), "{model_id}");
        assert_eq!(info.supports_image, Some(true), "{model_id}");
        assert_eq!(info.supports_video, Some(false), "{model_id}");
        assert_eq!(info.supports_multimodal, Some(true), "{model_id}");
    }
}

#[test]
fn latest_gemini_flash_models_have_documented_capabilities() {
    for model_id in ["gemini-3.1-flash-lite", "gemini-3.5-flash"] {
        let info = CapabilityBaseline::lookup_for_provider("google", model_id).unwrap();
        assert_eq!(info.context_window, Some(1_048_576), "{model_id}");
        assert_eq!(info.max_output_tokens, Some(65_536), "{model_id}");
        assert_eq!(info.supports_image, Some(true), "{model_id}");
        assert_eq!(info.supports_video, Some(true), "{model_id}");
        assert_eq!(info.supports_multimodal, Some(true), "{model_id}");
    }
}

#[test]
fn glm_5_2_has_documented_capabilities() {
    let info = CapabilityBaseline::lookup_for_provider("zhipu-cn", "glm-5.2").unwrap();

    assert_eq!(info.name, "GLM-5.2");
    assert_eq!(info.context_window, Some(1_048_576));
    assert_eq!(info.max_output_tokens, Some(131_072));
    assert_eq!(info.supports_image, Some(false));
    assert_eq!(info.supports_video, Some(false));
    assert_eq!(info.supports_multimodal, Some(false));
}

#[test]
fn kimi_k3_uses_128k_estimated_output_limit() {
    let info = CapabilityBaseline::lookup_for_provider("kimi-intl", "kimi-k3").unwrap();

    assert_eq!(info.name, "Kimi K3");
    assert_eq!(info.context_window, Some(1_048_576));
    assert_eq!(info.max_output_tokens, Some(131_072));
    assert_eq!(info.supports_image, Some(true));
    assert_eq!(info.supports_video, Some(true));
    assert_eq!(info.supports_multimodal, Some(true));
}
