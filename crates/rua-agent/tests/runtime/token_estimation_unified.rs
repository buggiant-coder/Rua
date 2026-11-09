use agent_types::{ContentPart, ModelDialogueItem, Role};
use rua_agent::context_checker::HeuristicEstimator;

fn estimate_messages(messages: &[ModelDialogueItem]) -> usize {
    let estimator = HeuristicEstimator::default();
    messages
        .iter()
        .map(|message| {
            let text_tokens = estimator.estimate_string(&message.text_content());
            let tool_tokens: usize = message
                .tool_results()
                .iter()
                .map(|result| estimator.estimate_string(&result.text_content()))
                .sum();
            text_tokens + tool_tokens
        })
        .sum()
}

fn make_text_msg(role: Role, text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role,
        content: vec![ContentPart::Text {
            text: text.to_string(),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn make_tool_result_msg(call_id: &str, name: &str, content: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            result: agent_types::tool::ToolResult {
                call_id: call_id.into(),
                name: Some(name.into()),
                content: vec![ContentPart::Text {
                    text: content.into(),
                }],
                is_error: false,
                duration_ms: None,
                attachments: None,
                metadata: serde_json::Map::new(),
                truncation: None,
            },
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn context_manager_estimate_matches_heuristic_estimator_ascii() {
    let estimator = HeuristicEstimator::default();
    let text = "Hello world, this is a test of token estimation.";
    let msgs = vec![make_text_msg(Role::User, text)];

    let context_result = estimate_messages(&msgs);
    let direct_result = estimator.estimate_string(text);

    assert_eq!(context_result, direct_result);
}

#[test]
fn context_manager_estimate_matches_heuristic_estimator_cjk() {
    let estimator = HeuristicEstimator::default();
    let text = "这是一个中文测试字符串用于验证token估算";
    let msgs = vec![make_text_msg(Role::User, text)];

    let context_result = estimate_messages(&msgs);
    let direct_result = estimator.estimate_string(text);

    assert_eq!(context_result, direct_result);
    let naive_byte_based = text.len() / 4;
    assert_ne!(
        direct_result, naive_byte_based,
        "CJK-aware estimate should differ from naive byte-len/4"
    );
}

#[test]
fn context_manager_estimate_matches_heuristic_estimator_with_tool_results() {
    let estimator = HeuristicEstimator::default();
    let user_text = "Search for files";
    let tool_content = "Found 3 files: main.rs, lib.rs, mod.rs";
    let msgs = vec![
        make_text_msg(Role::User, user_text),
        make_tool_result_msg("call_1", "search", tool_content),
    ];

    let context_result = estimate_messages(&msgs);
    let direct_result =
        estimator.estimate_string(user_text) + estimator.estimate_string(tool_content);

    assert_eq!(context_result, direct_result);
}

#[test]
fn context_manager_estimate_matches_heuristic_estimator_mixed_text() {
    let estimator = HeuristicEstimator::default();
    let text = "Hello 你好 world 世界";
    let msgs = vec![make_text_msg(Role::Assistant, text)];

    let context_result = estimate_messages(&msgs);
    let direct_result = estimator.estimate_string(text);

    assert_eq!(context_result, direct_result);
}

#[test]
fn heuristic_estimator_custom_divisor_changes_ascii_estimate() {
    let text = "test text here";
    let default_estimator = HeuristicEstimator::with_divisor(4.0);
    let tighter_estimator = HeuristicEstimator::with_divisor(2.0);

    assert!(
        tighter_estimator.estimate_string(text) > default_estimator.estimate_string(text),
        "smaller divisor should produce a larger token estimate"
    );
}

#[test]
fn heuristic_estimator_empty_string_returns_zero() {
    let estimator = HeuristicEstimator::default();

    assert_eq!(estimator.estimate_string(""), 0);
}

#[test]
fn heuristic_estimator_clamps_zero_and_negative_divisors_to_safe_minimum() {
    let text = "hello";
    let zero = HeuristicEstimator::with_divisor(0.0);
    let negative = HeuristicEstimator::with_divisor(-1.0);
    let minimum = HeuristicEstimator::with_divisor(1.0);

    assert_eq!(zero.estimate_string(text), minimum.estimate_string(text));
    assert_eq!(
        negative.estimate_string(text),
        minimum.estimate_string(text)
    );
    assert_eq!(minimum.estimate_string(text), 5);
}

#[test]
fn context_manager_estimate_empty_messages() {
    let msgs: Vec<ModelDialogueItem> = vec![];
    assert_eq!(estimate_messages(&msgs), 0);
}
