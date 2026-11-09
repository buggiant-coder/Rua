pub struct ContextOverflowDetector;

impl ContextOverflowDetector {
    pub fn is_context_overflow_message(message: &str) -> bool {
        rua_provider::error::is_context_overflow(message, None)
    }
}
