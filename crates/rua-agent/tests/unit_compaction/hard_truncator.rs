use agent_types::{ModelDialogueItem, Role};
use rua_agent::{BoundaryCalculator, BoundaryPlan, HardTruncator};

fn sys(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::System, text)
}

fn user(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::User, text)
}

fn asst(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::Assistant, text)
}

#[test]
fn hard_truncator_inserts_notice_and_preserves_recent_messages() {
    let messages = vec![
        sys("system"),
        user("old turn"),
        asst("old response"),
        user("recent turn"),
        asst("recent response"),
    ];
    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    let (truncated, removed) = HardTruncator::truncate(&messages, &plan).unwrap();

    assert!(removed > 0);
    assert!(
        truncated
            .iter()
            .any(|message| message.text_content().contains("Context truncated")),
        "truncated output must contain a truncation notice"
    );
    assert_eq!(truncated[0].text_content(), "system");
    assert!(
        truncated
            .iter()
            .any(|message| message.text_content() == "recent turn"),
        "recent user turn must be preserved"
    );
}

#[test]
fn hard_truncator_noop_on_empty_compact_range() {
    let messages = vec![sys("system"), user("only turn")];
    let plan = BoundaryPlan {
        system_range: 0..1,
        compact_range: 1..1,
        keep_range: 1..2,
        protected_ranges: vec![],
        reason: "nothing to compact".into(),
    };

    let (truncated, removed) = HardTruncator::truncate(&messages, &plan).unwrap();

    assert_eq!(removed, 0);
    assert_eq!(truncated.len(), messages.len());
}
