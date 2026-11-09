use rua_session::{
    ChatModelSwitchState, CheckpointModelIdentity, ContextTargetBand, ModelSwitchApplyError,
    ModelSwitchMaintenanceProof, ModelSwitchPreflight, ModelSwitchPreflightDecision,
    ModelSwitchPreflightInput, ProjectionEntryMode, ProjectionFitStatus,
    ProjectionMessageGroupEstimate, ProjectionPlanner, ProjectionPlanningInput,
};

fn band() -> ContextTargetBand {
    ContextTargetBand {
        upper_target_tokens: 700,
        emergency_limit_tokens: 1_000,
        tail_reserve_tokens: 100,
    }
}

fn model(provider: &str, model: &str) -> CheckpointModelIdentity {
    CheckpointModelIdentity {
        provider: provider.to_string(),
        model: model.to_string(),
    }
}

fn turn(ordinal: u64, tokens: u64) -> ProjectionMessageGroupEstimate {
    ProjectionMessageGroupEstimate {
        turn_id: format!("turn-{ordinal}"),
        turn_ordinal: ordinal,
        first_sequence: ordinal * 2 - 1,
        last_sequence: ordinal * 2,
        token_estimate: tokens,
        foldable: true,
        retained_sequence_ranges: Vec::new(),
    }
}

fn preflight_input(current_projection_tokens: u64) -> ModelSwitchPreflightInput {
    ModelSwitchPreflightInput {
        current_model: model("openai", "gpt-5-large"),
        target_model: model("openai", "gpt-5-medium"),
        target_band: band(),
        current_projection_tokens,
        checkpoint_tokens: 120,
        system_context_tokens: 80,
        pending_user_input_tokens: 0,
        message_groups: vec![turn(1, 160), turn(2, 120), turn(3, 100)],
    }
}

fn proof_for(current_projection_tokens: u64) -> ModelSwitchMaintenanceProof {
    ModelSwitchMaintenanceProof {
        plan: ProjectionPlanner::new(band()).plan(ProjectionPlanningInput {
            entry_mode: ProjectionEntryMode::ForegroundContextMaintenance,
            current_projection_tokens,
            checkpoint_tokens: 120,
            system_context_tokens: 80,
            pending_user_input_tokens: 0,
            message_groups: vec![turn(1, 300), turn(2, 280), turn(3, 260)],
        }),
    }
}

#[test]
fn model_switch_preflight_allows_safe_target_without_checkpoint() {
    let decision = ModelSwitchPreflight::evaluate(preflight_input(650));

    assert_eq!(
        decision,
        ModelSwitchPreflightDecision::Allow {
            target_model: model("openai", "gpt-5-medium"),
            schedule_maintenance: false,
            estimated_projection_tokens: 650,
            target_upper_tokens: 700,
            emergency_limit_tokens: 1_000,
        }
    );
}

#[test]
fn model_switch_preflight_allows_emergency_fit_but_schedules_maintenance() {
    let decision = ModelSwitchPreflight::evaluate(preflight_input(800));

    assert_eq!(
        decision,
        ModelSwitchPreflightDecision::Allow {
            target_model: model("openai", "gpt-5-medium"),
            schedule_maintenance: true,
            estimated_projection_tokens: 800,
            target_upper_tokens: 700,
            emergency_limit_tokens: 1_000,
        }
    );
}

#[test]
fn model_switch_preflight_requires_confirmation_with_fold_frontier_when_checkpoint_needed() {
    let mut input = preflight_input(1_100);
    input.message_groups = vec![turn(1, 300), turn(2, 280), turn(3, 260)];

    let decision = ModelSwitchPreflight::evaluate(input);

    let ModelSwitchPreflightDecision::ConfirmationRequired {
        target_model,
        estimated_projection_tokens,
        target_upper_tokens,
        emergency_limit_tokens,
        recommended_fold_frontier,
    } = decision
    else {
        panic!("dangerous switch should require confirmation");
    };
    assert_eq!(target_model, model("openai", "gpt-5-medium"));
    assert_eq!(estimated_projection_tokens, 1_100);
    assert_eq!(target_upper_tokens, 700);
    assert_eq!(emergency_limit_tokens, 1_000);

    let frontier = recommended_fold_frontier.expect("fold frontier");
    assert_eq!(frontier.start_turn_ordinal, 1);

    assert_eq!(frontier.end_turn_ordinal, 3);
    assert_eq!(frontier.start_sequence, 1);
    assert_eq!(frontier.end_sequence, 6);
    assert_eq!(frontier.estimated_tokens_to_fold, 840);
}

#[test]
fn confirmed_model_switch_preserves_old_model_when_maintenance_fails() {
    let old_model = model("openai", "gpt-5-large");
    let target_model = model("openai", "gpt-5-mini");
    let mut state = ChatModelSwitchState::new(old_model.clone());

    let result = state.apply_confirmed_switch(
        target_model,
        Err(ModelSwitchApplyError::MaintenanceFailed(
            "summary model timed out".to_string(),
        )),
    );

    assert_eq!(
        result,
        Err(ModelSwitchApplyError::MaintenanceFailed(
            "summary model timed out".to_string(),
        ))
    );
    assert_eq!(
        state.default_model(),
        &old_model,
        "failed foreground maintenance must not apply the target model"
    );
}

#[test]
fn confirmed_model_switch_applies_target_only_after_safe_projection_proof() {
    let old_model = model("openai", "gpt-5-large");
    let target_model = model("openai", "gpt-5-mini");
    let mut state = ChatModelSwitchState::new(old_model);

    let applied = state
        .apply_confirmed_switch(target_model.clone(), Ok(proof_for(650)))
        .unwrap();

    assert_eq!(applied, target_model);
    assert_eq!(state.default_model(), &target_model);
}

#[test]
fn safe_model_switch_decision_updates_default_model_without_maintenance() {
    let old_model = model("openai", "gpt-5-large");
    let target_model = model("openai", "gpt-5-medium");
    let mut state = ChatModelSwitchState::new(old_model);
    let decision = ModelSwitchPreflight::evaluate(preflight_input(650));

    let applied = state.apply_preflight_decision(decision).unwrap();

    assert_eq!(applied, target_model);
    assert_eq!(state.default_model(), &target_model);
}

#[test]
fn dangerous_model_switch_decision_does_not_update_default_model_without_confirmation() {
    let old_model = model("openai", "gpt-5-large");
    let mut input = preflight_input(1_100);
    input.message_groups = vec![turn(1, 300), turn(2, 280), turn(3, 260)];
    let decision = ModelSwitchPreflight::evaluate(input);
    let mut state = ChatModelSwitchState::new(old_model.clone());

    let result = state.apply_preflight_decision(decision);

    assert_eq!(result, Err(ModelSwitchApplyError::ConfirmationRequired));
    assert_eq!(state.default_model(), &old_model);
}

#[test]
fn confirmed_model_switch_rejects_unsafe_projection_proof_and_keeps_old_model() {
    let old_model = model("openai", "gpt-5-large");
    let target_model = model("openai", "gpt-5-mini");
    let mut state = ChatModelSwitchState::new(old_model.clone());

    let result = state.apply_confirmed_switch(target_model, Ok(proof_for(800)));

    assert_eq!(
        result,
        Err(ModelSwitchApplyError::ProjectionStillUnsafe {
            status: ProjectionFitStatus::FitsEmergencyButNeedsMaintenance,
        })
    );
    assert_eq!(state.default_model(), &old_model);
}
