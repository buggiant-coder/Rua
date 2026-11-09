use rua_agent::memory_signal::{
    detect_memory_signal, MemorySignalConfidence, MemorySignalKind, MemorySignalState,
};
use rua_agent::memory_signal_note::render_memory_signal_for_model;

#[test]
fn explicit_memory_signal_is_detected_when_memory_write_available() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal("记住一下：以后回答直接给工程结论。", true, &mut state)
        .expect("explicit memory signal");

    assert_eq!(signal.kind, MemorySignalKind::ExplicitMemory);
    assert_eq!(signal.confidence, MemorySignalConfidence::High);
    assert_eq!(signal.suggested_memory_kind, "preference");
    assert_eq!(signal.suggested_scope, "user");
}

#[test]
fn exclude_phrase_suppresses_memory_signal() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal("不要记这个：以后回答直接给工程结论。", true, &mut state);

    assert!(signal.is_none());
}

#[test]
fn missed_recall_signal_is_high_confidence() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal("我之前说过我喜欢简洁回答。", true, &mut state)
        .expect("missed recall signal");

    assert_eq!(signal.kind, MemorySignalKind::MissedRecall);
    assert_eq!(signal.confidence, MemorySignalConfidence::High);
    assert_eq!(signal.suggested_memory_kind, "preference");
    assert_eq!(signal.suggested_scope, "user");
}

#[test]
fn english_missed_recall_signal_is_detected() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal(
        "I already told you I prefer short answers.",
        true,
        &mut state,
    )
    .expect("missed recall signal");

    assert_eq!(signal.kind, MemorySignalKind::MissedRecall);
    assert_eq!(signal.confidence, MemorySignalConfidence::High);
}

#[test]
fn missed_recall_marker_recommends_update_first_workflow() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal(
        "I already told you I prefer short answers.",
        true,
        &mut state,
    )
    .expect("missed recall signal");
    let rendered = render_memory_signal_for_model(&signal);

    assert!(rendered.contains("<memory_signal>"), "got: {rendered}");
    assert!(
        rendered.contains("signal_kind: missed_recall"),
        "got: {rendered}"
    );
    assert!(rendered.contains("memory_search"), "got: {rendered}");
    assert!(rendered.contains("memory_get"), "got: {rendered}");
    assert!(rendered.contains("memory_update"), "got: {rendered}");
    assert!(
        rendered.contains("memory_write only if no related record exists"),
        "got: {rendered}"
    );
}

#[test]
fn detector_ignores_code_blocks_and_quoted_text() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal(
        r#"> 记住一下：这是引用里的内容

```text
remember this: code sample
```
"#,
        true,
        &mut state,
    );

    assert!(signal.is_none());
}

#[test]
fn low_confidence_frustration_requires_repeated_hits() {
    let mut state = MemorySignalState::default();

    assert!(detect_memory_signal("你总是解释太多。", true, &mut state).is_none());
    let signal = detect_memory_signal("你总是解释太多。", true, &mut state)
        .expect("second low-confidence hit should trigger");

    assert_eq!(signal.kind, MemorySignalKind::Frustration);
    assert_eq!(signal.confidence, MemorySignalConfidence::Low);
}

#[test]
fn detector_does_not_emit_when_memory_write_is_unavailable() {
    let mut state = MemorySignalState::default();
    let signal = detect_memory_signal("记住一下：以后回答直接给工程结论。", false, &mut state);

    assert!(signal.is_none());
}
