use rua_config::{AgentsRunningConfig, ContextCompactConfig};
use rua_server::services::context_projection::{
    blocking_context_target_band_from_running, context_target_band_for_model,
    context_target_band_from_running, target_band_version,
};

#[test]
fn model_context_window_overrides_running_max_input_for_target_band() {
    let running = AgentsRunningConfig {
        max_input_length: 1_000,
        context_compact: ContextCompactConfig {
            blocking_compact_ratio: 0.8,
            context_reserve_ratio: 0.2,
            ..Default::default()
        },
        ..Default::default()
    };

    let from_running = context_target_band_from_running(&running);
    assert_eq!(from_running.upper_target_tokens, 800);
    assert_eq!(from_running.emergency_limit_tokens, 1_000);
    assert_eq!(from_running.tail_reserve_tokens, 200);

    let from_default_model = context_target_band_for_model(&running, 2_000);
    assert_eq!(from_default_model.upper_target_tokens, 1_600);
    assert_eq!(from_default_model.emergency_limit_tokens, 2_000);
    assert_eq!(from_default_model.tail_reserve_tokens, 400);
    assert_ne!(
        target_band_version(&from_default_model),
        target_band_version(&from_running),
        "context projection should change when the active model context window changes"
    );
}

#[test]
fn early_background_compaction_uses_configured_target_band() {
    let running = AgentsRunningConfig {
        max_input_length: 1_000,
        context_compact: ContextCompactConfig {
            blocking_compact_ratio: 0.95,
            early_background_compaction_enabled: true,
            early_compaction_target_ratio: 0.5,
            context_reserve_ratio: 0.1,
            ..Default::default()
        },
        ..Default::default()
    };

    let band = context_target_band_from_running(&running);
    assert_eq!(band.upper_target_tokens, 500);
    assert_eq!(band.emergency_limit_tokens, 1_000);
    assert_eq!(band.tail_reserve_tokens, 100);
}

#[test]
fn blocking_target_band_ignores_early_compaction_target() {
    let running = AgentsRunningConfig {
        max_input_length: 1_000,
        context_compact: ContextCompactConfig {
            blocking_compact_ratio: 0.95,
            early_background_compaction_enabled: true,
            early_compaction_target_ratio: 0.5,
            context_reserve_ratio: 0.25,
            ..Default::default()
        },
        ..Default::default()
    };

    let band = blocking_context_target_band_from_running(&running);
    assert_eq!(band.upper_target_tokens, 950);
    assert_eq!(band.tail_reserve_tokens, 250);
}

#[test]
fn target_band_clamps_ratio_inputs_and_tiny_model_windows() {
    let running = AgentsRunningConfig {
        max_input_length: 4_000,
        context_compact: ContextCompactConfig {
            blocking_compact_ratio: 2.0,
            context_reserve_ratio: -1.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let band = context_target_band_for_model(&running, 0);
    assert_eq!(band.upper_target_tokens, 1);
    assert_eq!(band.emergency_limit_tokens, 1);
    assert_eq!(band.tail_reserve_tokens, 1);
}
