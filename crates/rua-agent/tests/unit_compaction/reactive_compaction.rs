use rua_agent::ContextOverflowDetector;

#[test]
fn overflow_detection_covers_provider_errors_without_hiding_unrelated_failures() {
    for message in [
        "context_length_exceeded",
        "Your input exceeds the context window of this model. Please adjust your input and try again.",
        "maximum context length exceeded",
        "prompt is too long",
        "input_tokens_overflow",
    ] {
        assert!(ContextOverflowDetector::is_context_overflow_message(message), "{message}");
    }
    for message in [
        "invalid API token",
        "context summary timed out",
        "connection reset",
    ] {
        assert!(
            !ContextOverflowDetector::is_context_overflow_message(message),
            "{message}"
        );
    }
}
