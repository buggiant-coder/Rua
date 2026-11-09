use rua_session::{
    evaluate_history_replacement_candidate, HistoryReplacementCandidateDecision,
    HistoryReplacementRejectionReason,
};

#[test]
fn candidate_quality_gates_cover_every_terminal_rejection() {
    assert_eq!(
        evaluate_history_replacement_candidate(20_000, "   ", "[locator]", "stop", 4.0,),
        HistoryReplacementCandidateDecision::Rejected {
            reason: HistoryReplacementRejectionReason::EmptyOutput,
            replacement_token_estimate: 3,
            saved_token_estimate: 19_997,
        }
    );
    assert!(matches!(
        evaluate_history_replacement_candidate(
            20_000,
            "partial",
            &"x".repeat(20_000),
            "length",
            4.0,
        ),
        HistoryReplacementCandidateDecision::Rejected {
            reason: HistoryReplacementRejectionReason::OutputLimitReached,
            ..
        }
    ));
    assert!(matches!(
        evaluate_history_replacement_candidate(
            16_000,
            "large digest",
            &"x".repeat(36_000),
            "stop",
            4.0,
        ),
        HistoryReplacementCandidateDecision::Rejected {
            reason: HistoryReplacementRejectionReason::InsufficientSavings,
            replacement_token_estimate: 9_000,
            saved_token_estimate: 7_000,
        }
    ));
    assert!(matches!(
        evaluate_history_replacement_candidate(
            30_000,
            "large digest",
            &"x".repeat(60_000),
            "stop",
            4.0,
        ),
        HistoryReplacementCandidateDecision::Rejected {
            reason: HistoryReplacementRejectionReason::InsufficientReduction,
            replacement_token_estimate: 15_000,
            saved_token_estimate: 15_000,
        }
    ));
    assert!(matches!(
        evaluate_history_replacement_candidate(
            20_000,
            "useful digest",
            &"x".repeat(20_000),
            "content_filter",
            4.0,
        ),
        HistoryReplacementCandidateDecision::Rejected {
            reason: HistoryReplacementRejectionReason::UnsupportedFinishReason,
            ..
        }
    ));
}

#[test]
fn candidate_quality_gate_accepts_once_both_savings_requirements_hold() {
    assert_eq!(
        evaluate_history_replacement_candidate(
            20_000,
            "useful digest",
            &"x".repeat(20_000),
            "stop",
            4.0,
        ),
        HistoryReplacementCandidateDecision::Ready {
            replacement_token_estimate: 5_000,
            saved_token_estimate: 15_000,
        }
    );
}

#[test]
fn cancellation_and_transport_style_terminal_reasons_remain_retryable() {
    for reason in ["cancelled", "canceled", "error"] {
        assert_eq!(
            evaluate_history_replacement_candidate(
                20_000,
                "partial",
                &"x".repeat(20_000),
                reason,
                4.0,
            ),
            HistoryReplacementCandidateDecision::Retryable {
                reason: reason.to_string(),
            }
        );
    }
}
