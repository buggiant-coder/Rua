pub fn redact_for_log(input: &str, max_len: usize) -> String {
    let truncated = if input.len() > max_len {
        let mut end = max_len;
        while end > 0 && !input.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}...[truncated, {} bytes total]",
            &input[..end],
            input.len()
        )
    } else {
        input.to_string()
    };

    truncated
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\r' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}
