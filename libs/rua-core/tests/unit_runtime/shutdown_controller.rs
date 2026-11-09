use std::time::Duration;

use rua_core::{ShutdownConfig, ShutdownController, ShutdownPhase};

#[test]
fn shutdown_controller_advances_through_the_seven_phase_contract() {
    let mut controller = ShutdownController::new(ShutdownConfig::default());

    assert_eq!(controller.current_phase(), None);
    assert!(!controller.is_shutting_down());

    let phases: Vec<_> = std::iter::from_fn(|| controller.advance()).collect();

    assert_eq!(
        phases,
        vec![
            ShutdownPhase::StopAccepting,
            ShutdownPhase::NotifyStreams,
            ShutdownPhase::CancelNonCritical,
            ShutdownPhase::WaitPersistence,
            ShutdownPhase::StopSubsystems,
            ShutdownPhase::FlushObservability,
            ShutdownPhase::ReleasePowerResources,
        ]
    );
    assert_eq!(controller.current_phase(), None);
    assert!(!controller.is_shutting_down());
    assert_eq!(controller.advance(), Some(ShutdownPhase::StopAccepting));
}

#[test]
fn shutdown_phase_wire_names_are_stable_snake_case() {
    assert_eq!(ShutdownPhase::StopAccepting.to_string(), "stop_accepting");
    assert_eq!(ShutdownPhase::NotifyStreams.to_string(), "notify_streams");
    assert_eq!(
        ShutdownPhase::CancelNonCritical.to_string(),
        "cancel_non_critical"
    );
    assert_eq!(
        ShutdownPhase::WaitPersistence.to_string(),
        "wait_persistence"
    );
    assert_eq!(ShutdownPhase::StopSubsystems.to_string(), "stop_subsystems");
    assert_eq!(
        ShutdownPhase::FlushObservability.to_string(),
        "flush_observability"
    );
    assert_eq!(
        ShutdownPhase::ReleasePowerResources.to_string(),
        "release_power_resources"
    );

    let serialized = serde_json::to_value(ShutdownPhase::StopSubsystems).unwrap();
    assert_eq!(serialized, serde_json::json!("stop_subsystems"));
    let roundtripped: ShutdownPhase = serde_json::from_value(serialized).unwrap();
    assert_eq!(roundtripped, ShutdownPhase::StopSubsystems);
}

#[test]
fn shutdown_phase_timeouts_use_configured_persistence_subsystem_and_flush_values() {
    let config = ShutdownConfig {
        total_timeout: Duration::from_secs(99),
        persistence_grace: Duration::from_millis(123),
        subsystem_timeout: Duration::from_millis(456),
        flush_timeout: Duration::from_millis(789),
    };
    let mut controller = ShutdownController::new(config.clone());

    assert_eq!(controller.config().total_timeout, Duration::from_secs(99));

    assert_eq!(controller.advance(), Some(ShutdownPhase::StopAccepting));
    assert_eq!(controller.phase_timeout(), Duration::from_secs(5));

    assert_eq!(controller.advance(), Some(ShutdownPhase::NotifyStreams));
    assert_eq!(controller.phase_timeout(), Duration::from_secs(5));

    assert_eq!(controller.advance(), Some(ShutdownPhase::CancelNonCritical));
    assert_eq!(controller.phase_timeout(), Duration::from_secs(5));

    assert_eq!(controller.advance(), Some(ShutdownPhase::WaitPersistence));
    assert_eq!(controller.phase_timeout(), config.persistence_grace);

    assert_eq!(controller.advance(), Some(ShutdownPhase::StopSubsystems));
    assert_eq!(controller.phase_timeout(), config.subsystem_timeout);

    assert_eq!(
        controller.advance(),
        Some(ShutdownPhase::FlushObservability)
    );
    assert_eq!(controller.phase_timeout(), config.flush_timeout);

    assert_eq!(
        controller.advance(),
        Some(ShutdownPhase::ReleasePowerResources)
    );
    assert_eq!(controller.phase_timeout(), Duration::from_secs(5));
}

#[test]
fn shutdown_config_default_matches_runtime_shutdown_budget_contract() {
    let config = ShutdownConfig::default();

    assert_eq!(config.total_timeout, Duration::from_secs(30));
    assert_eq!(config.persistence_grace, Duration::from_secs(5));
    assert_eq!(config.subsystem_timeout, Duration::from_secs(10));
    assert_eq!(config.flush_timeout, Duration::from_secs(5));
}
