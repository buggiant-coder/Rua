use rua_command::{agent_input_indices, input_window_start};

#[test]
fn no_commands_returns_zero() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi there"),
        ("user", "how are you?"),
        ("assistant", "I'm fine"),
    ];
    assert_eq!(input_window_start(&msgs), 0);
}

#[test]
fn empty_messages_returns_zero() {
    let msgs: Vec<(&str, &str)> = vec![];
    assert_eq!(input_window_start(&msgs), 0);
}

#[test]
fn clear_excludes_clear_round() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/clear"),
        ("assistant", "History Cleared!"),
        ("user", "new question"),
        ("assistant", "new answer"),
    ];

    assert_eq!(input_window_start(&msgs), 4);
}

#[test]
fn clear_without_reply() {
    let msgs: Vec<(&str, &str)> = vec![("user", "hello"), ("assistant", "hi"), ("user", "/clear")];

    assert_eq!(input_window_start(&msgs), 3);
}

#[test]
fn compact_includes_compact_round() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/compact"),
        ("assistant", "**Compact Complete!** summary here"),
        ("user", "follow up"),
        ("assistant", "response"),
    ];

    assert_eq!(input_window_start(&msgs), 2);
}

#[test]
fn multiple_commands_takes_most_recent() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "old stuff"),
        ("assistant", "old reply"),
        ("user", "/clear"),
        ("assistant", "History Cleared!"),
        ("user", "middle"),
        ("assistant", "middle reply"),
        ("user", "/compact"),
        ("assistant", "**Compact Complete!** summary"),
        ("user", "latest"),
        ("assistant", "latest reply"),
    ];

    assert_eq!(input_window_start(&msgs), 6);
}

#[test]
fn clear_after_compact_takes_clear() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "/compact"),
        ("assistant", "summary"),
        ("user", "question"),
        ("assistant", "answer"),
        ("user", "/clear"),
        ("assistant", "History Cleared!"),
        ("user", "fresh start"),
        ("assistant", "fresh reply"),
    ];

    assert_eq!(input_window_start(&msgs), 6);
}

#[test]
fn non_user_role_with_command_text_ignored() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "/clear"),
        ("user", "still here"),
        ("assistant", "yes"),
    ];

    assert_eq!(input_window_start(&msgs), 0);
}

#[test]
fn command_with_args() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/compact extra instructions here"),
        ("assistant", "**Compact Complete!**"),
    ];

    assert_eq!(input_window_start(&msgs), 2);
}

#[test]
fn agent_indices_skips_command_rounds() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/skills"),
        ("assistant", "<skills list>"),
        ("user", "/status"),
        ("assistant", "<status>"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1]);
}

#[test]
fn agent_indices_skips_command_rounds_with_trailing_args() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/stop session=s123"),
        ("assistant", "stopped"),
        ("user", "next question"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1, 4]);
}

#[test]
fn agent_indices_keeps_normal_messages() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "how are you?"),
        ("assistant", "fine"),
    ];
    assert_eq!(agent_input_indices(&msgs), vec![0, 1, 2, 3]);
}

#[test]
fn agent_indices_mixed_commands_and_conversation() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/skills"),
        ("assistant", "<list>"),
        ("user", "what is rust?"),
        ("assistant", "a language"),
        ("user", "/status"),
        ("assistant", "<status>"),
        ("user", "thanks"),
        ("assistant", "welcome"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1, 4, 5, 8, 9]);
}

#[test]
fn agent_indices_preserves_clear_compact() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "old"),
        ("assistant", "old reply"),
        ("user", "/clear"),
        ("assistant", "cleared"),
        ("user", "new"),
        ("assistant", "new reply"),
        ("user", "/skills"),
        ("assistant", "<skills>"),
        ("user", "question"),
        ("assistant", "answer"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![4, 5, 8, 9]);
}

#[test]
fn agent_indices_compact_kept_in_window() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "old"),
        ("assistant", "old reply"),
        ("user", "/compact"),
        ("assistant", "summary"),
        ("user", "/skills"),
        ("assistant", "<skills>"),
        ("user", "question"),
        ("assistant", "answer"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![2, 3, 6, 7]);
}

#[test]
fn agent_indices_command_without_reply() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/skills"),
        ("user", "next question"),
        ("assistant", "answer"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1, 3, 4]);
}

#[test]
fn agent_indices_command_at_end() {
    let msgs: Vec<(&str, &str)> = vec![("user", "hello"), ("assistant", "hi"), ("user", "/status")];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1]);
}

#[test]
fn agent_indices_unknown_command_also_skipped() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "hello"),
        ("assistant", "hi"),
        ("user", "/foo"),
        ("assistant", "unknown command"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1]);
}

#[test]
fn agent_indices_keep_absolute_path_user_input() {
    let msgs: Vec<(&str, &str)> = vec![
        ("user", "/usr/local/bin"),
        ("assistant", "path noted"),
        ("user", "/tmp/project with args"),
    ];

    assert_eq!(agent_input_indices(&msgs), vec![0, 1, 2]);
}

#[test]
fn agent_indices_empty() {
    let msgs: Vec<(&str, &str)> = vec![];
    assert_eq!(agent_input_indices(&msgs), Vec::<usize>::new());
}
