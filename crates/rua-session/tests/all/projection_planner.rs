use rua_session::{
    CheckpointRetainedSequenceRange, ContextTargetBand, ContextTargetBandOverride,
    ProjectionEntryAction, ProjectionEntryMode, ProjectionFitStatus,
    ProjectionMessageGroupEstimate, ProjectionPlanner, ProjectionPlanningInput,
};

fn band() -> ContextTargetBand {
    ContextTargetBand {
        upper_target_tokens: 700,
        emergency_limit_tokens: 1_000,
        tail_reserve_tokens: 100,
    }
}

fn turn(
    ordinal: u64,
    first_sequence: u64,
    last_sequence: u64,
    tokens: u64,
) -> ProjectionMessageGroupEstimate {
    ProjectionMessageGroupEstimate {
        turn_id: format!("turn-{ordinal}"),
        turn_ordinal: ordinal,
        first_sequence,
        last_sequence,
        token_estimate: tokens,
        foldable: true,
        retained_sequence_ranges: Vec::new(),
    }
}

fn nonfoldable_group(
    ordinal: u64,
    first_sequence: u64,
    last_sequence: u64,
    tokens: u64,
) -> ProjectionMessageGroupEstimate {
    ProjectionMessageGroupEstimate {
        foldable: false,
        ..turn(ordinal, first_sequence, last_sequence, tokens)
    }
}

fn input(
    current_projection_tokens: u64,
    message_groups: Vec<ProjectionMessageGroupEstimate>,
) -> ProjectionPlanningInput {
    input_for(
        ProjectionEntryMode::ForegroundTurn,
        current_projection_tokens,
        message_groups,
    )
}

fn input_for(
    entry_mode: ProjectionEntryMode,
    current_projection_tokens: u64,
    message_groups: Vec<ProjectionMessageGroupEstimate>,
) -> ProjectionPlanningInput {
    ProjectionPlanningInput {
        entry_mode,
        current_projection_tokens,
        checkpoint_tokens: 120,
        system_context_tokens: 80,
        pending_user_input_tokens: 0,
        message_groups,
    }
}

#[test]
fn projection_planner_classifies_fit_statuses_against_target_band() {
    let planner = ProjectionPlanner::new(band());

    let fits = planner.plan(input(650, vec![turn(1, 1, 2, 120)]));
    assert_eq!(fits.status, ProjectionFitStatus::FitsTarget);
    assert!(!fits.maintenance_needed);
    assert!(fits.fold_frontier.is_none());

    let needs_maintenance = planner.plan(input(
        800,
        vec![turn(1, 1, 2, 120), turn(2, 3, 4, 100), turn(3, 5, 6, 80)],
    ));
    assert_eq!(
        needs_maintenance.status,
        ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
    );
    assert!(needs_maintenance.maintenance_needed);
    assert_eq!(
        needs_maintenance
            .fold_frontier
            .as_ref()
            .map(|frontier| frontier.end_turn_ordinal),
        Some(2),
        "raw history after the frontier should fit the 100-token tail allowance"
    );

    let requires_checkpoint = planner.plan(input(
        1_100,
        vec![turn(1, 1, 2, 300), turn(2, 3, 4, 280), turn(3, 5, 6, 260)],
    ));
    assert_eq!(
        requires_checkpoint.status,
        ProjectionFitStatus::RequiresCheckpoint
    );
    assert!(requires_checkpoint.maintenance_needed);

    assert_eq!(
        requires_checkpoint
            .fold_frontier
            .as_ref()
            .map(|frontier| frontier.end_turn_ordinal),
        Some(3)
    );

    let emergency = planner.plan(input(1_600, vec![nonfoldable_group(1, 1, 2, 100)]));
    assert_eq!(
        emergency.status,
        ProjectionFitStatus::RequiresEmergencyFallback
    );
    assert!(emergency.emergency_tail.is_some());

    let retained_tail = planner.plan(input(1_600, vec![turn(1, 1, 2, 100)]));
    assert_eq!(
        retained_tail.status,
        ProjectionFitStatus::RequiresEmergencyFallback
    );
    assert!(
        retained_tail.fold_frontier.is_none(),
        "the configured 100-token recent tail must remain raw"
    );
}

#[test]
fn projection_planner_skips_checkpoint_when_unfoldable_context_already_exceeds_emergency_limit() {
    let planner = ProjectionPlanner::new(band());
    let mut oversized_static_context = input(1_500, vec![turn(1, 1, 2, 400)]);
    oversized_static_context.system_context_tokens = 1_050;
    oversized_static_context.pending_user_input_tokens = 25;

    let plan = planner.plan(oversized_static_context);

    assert_eq!(plan.status, ProjectionFitStatus::RequiresEmergencyFallback);
    assert_eq!(
        plan.action,
        ProjectionEntryAction::UseEmergencyBoundedProjection
    );
    assert!(
        plan.fold_frontier.is_none(),
        "folding durable Message groups cannot make an oversized static envelope fit"
    );
}

#[test]
fn projection_planner_maps_same_status_to_entry_mode_actions() {
    let planner = ProjectionPlanner::new(band());
    let foldable_turns = vec![turn(1, 1, 2, 300), turn(2, 3, 4, 280), turn(3, 5, 6, 260)];

    let pre_model_call = planner.plan(input_for(
        ProjectionEntryMode::PreModelCall,
        1_100,
        foldable_turns.clone(),
    ));
    assert_eq!(
        pre_model_call.action,
        ProjectionEntryAction::CreateCheckpointSynchronously
    );

    let foreground = planner.plan(input_for(
        ProjectionEntryMode::ForegroundTurn,
        1_100,
        foldable_turns.clone(),
    ));
    assert_eq!(foreground.status, ProjectionFitStatus::RequiresCheckpoint);
    assert_eq!(
        foreground.action,
        ProjectionEntryAction::UseEmergencyBoundedProjection
    );

    let background = planner.plan(input_for(
        ProjectionEntryMode::EarlyBackgroundCompaction,
        800,
        foldable_turns.clone(),
    ));
    assert_eq!(
        background.status,
        ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
    );
    assert_eq!(
        background.action,
        ProjectionEntryAction::ScheduleLowPriorityCheckpoint
    );

    let manual_compact = planner.plan(input_for(
        ProjectionEntryMode::ForegroundContextMaintenance,
        650,
        foldable_turns.clone(),
    ));
    assert_eq!(manual_compact.status, ProjectionFitStatus::FitsTarget);
    assert_eq!(
        manual_compact.action,
        ProjectionEntryAction::CreateCheckpointSynchronously
    );

    let model_switch = planner.plan(input_for(
        ProjectionEntryMode::ModelSwitchPreflight,
        1_100,
        foldable_turns.clone(),
    ));
    assert_eq!(
        model_switch.action,
        ProjectionEntryAction::BlockAndAskToRunCheckpoint
    );

    let crash_recovery = planner.plan(input_for(
        ProjectionEntryMode::CrashRecovery,
        1_100,
        foldable_turns,
    ));
    assert_eq!(
        crash_recovery.action,
        ProjectionEntryAction::UseOldCheckpointPlusBoundedTail
    );
}

#[test]
fn context_target_band_resolves_agent_overrides_over_model_defaults() {
    let model_default = band();
    let resolved = ContextTargetBand::resolve(
        model_default,
        Some(ContextTargetBandOverride {
            upper_target_tokens: Some(800),
            emergency_limit_tokens: None,
            tail_reserve_tokens: Some(120),
        }),
    );

    assert_eq!(resolved.upper_target_tokens, 800);
    assert_eq!(
        resolved.emergency_limit_tokens,
        model_default.emergency_limit_tokens
    );
    assert_eq!(resolved.tail_reserve_tokens, 120);
}

#[test]
fn projection_planner_selects_only_complete_message_group_boundaries() {
    let planner = ProjectionPlanner::new(band());
    let plan = planner.plan(input(
        1_050,
        vec![
            turn(1, 1, 4, 360),
            nonfoldable_group(2, 5, 8, 360),
            turn(3, 9, 12, 260),
        ],
    ));

    let frontier = plan.fold_frontier.expect("fold frontier");
    assert_eq!(frontier.start_turn_ordinal, 1);
    assert_eq!(frontier.end_turn_ordinal, 1);
    assert_eq!(frontier.start_sequence, 1);
    assert_eq!(frontier.end_sequence, 4);

    let emergency = planner.plan(input(
        1_600,
        vec![turn(1, 1, 2, 400), turn(2, 3, 4, 360), turn(3, 5, 6, 260)],
    ));
    let tail = emergency.emergency_tail.expect("emergency tail");
    assert_eq!(tail.start_turn_ordinal, 2);
    assert_eq!(tail.start_sequence, 3);
    assert!(
        tail.estimated_tail_tokens <= band().emergency_limit_tokens,
        "reduced history must stay under max_input_length"
    );

    let oversized_latest_group = planner.plan(input(1_600, vec![turn(1, 1, 2, 900)]));
    assert!(oversized_latest_group.emergency_tail.is_some());
}

#[test]
fn blocking_compaction_has_a_complete_group_fallback_before_the_hard_limit() {
    let planner = ProjectionPlanner::new(band());
    let plan = planner.plan(input_for(
        ProjectionEntryMode::PreModelCall,
        950,
        vec![turn(1, 1, 4, 400), turn(2, 5, 8, 360), turn(3, 9, 12, 80)],
    ));
    assert_eq!(
        plan.action,
        ProjectionEntryAction::CreateCheckpointSynchronously
    );
    let fallback = plan
        .emergency_tail
        .expect("summary failure needs a planned fallback");
    assert_eq!(fallback.start_sequence, 9);
    assert!(fallback.estimated_tail_tokens <= band().emergency_limit_tokens);
}

#[test]
fn projection_planner_can_end_at_complete_active_message_group_and_retain_user() {
    let planner = ProjectionPlanner::new(band());
    let retained_user = CheckpointRetainedSequenceRange {
        start_sequence: 5,
        end_sequence: 5,
        reason: "active_user_request".to_string(),
    };
    let active_group = ProjectionMessageGroupEstimate {
        turn_id: "turn-active".to_string(),
        turn_ordinal: 2,
        first_sequence: 5,
        last_sequence: 8,
        token_estimate: 400,
        foldable: true,
        retained_sequence_ranges: vec![retained_user.clone()],
    };

    let plan = planner.plan(input(1_100, vec![turn(1, 1, 4, 400), active_group]));
    let frontier = plan.fold_frontier.expect("model call frontier");

    assert_eq!(frontier.turn_id, "turn-active");
    assert_eq!(frontier.end_turn_ordinal, 2);
    assert_eq!(frontier.end_sequence, 8);
    assert_eq!(frontier.retained_sequence_ranges, vec![retained_user]);
}
