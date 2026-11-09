use agent_types::{ContentPart, ModelDialogueItem, Role};
use rua_agent::EstimationMethod;
use rua_agent::{BpeEstimator, HeuristicEstimator, TokenEstimator};

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

#[test]
fn bpe_encode_len_matches_python() {
    let bpe = BpeEstimator::new();
    assert_eq!(
        bpe.encode_len("Hello world, this is a test of token estimation."),
        11
    );
    assert_eq!(bpe.encode_len("Hello world"), 2);
    assert_eq!(
        bpe.encode_len(&"The quick brown fox jumps over the lazy dog. ".repeat(10)),
        101,
    );
    assert_eq!(
        bpe.encode_len("fn main() { println!(\"Hello, world!\"); let x = 42; }"),
        18,
    );
}

#[test]
fn bpe_empty_string_returns_zero() {
    let bpe = BpeEstimator::new();
    assert_eq!(bpe.encode_len(""), 0);
}

#[tokio::test]
async fn bpe_estimator_returns_tiktoken_method() {
    let bpe = BpeEstimator::new();
    let msgs = vec![make_text_msg(Role::User, "Hello world")];
    let result = bpe
        .estimate_messages("test", "test", &msgs, &[])
        .await
        .unwrap();
    assert_eq!(result.method, EstimationMethod::Tiktoken);
    assert!(!result.estimated);
}

#[tokio::test]
async fn bpe_vs_python_deviation_under_5_percent() {
    let bpe = BpeEstimator::new();
    let long_text = "The quick brown fox jumps over the lazy dog. ".repeat(100);
    let msgs = vec![make_text_msg(Role::User, &long_text)];

    let result = bpe
        .estimate_messages("test", "test", &msgs, &[])
        .await
        .unwrap();
    let bpe_total = result.per_message[0];
    let overhead = 4;
    let bpe_text_tokens = bpe_total - overhead;

    let python_tokens = 1001usize;
    let deviation = (bpe_text_tokens as f64 - python_tokens as f64).abs() / python_tokens as f64;
    assert!(
        deviation < 0.05,
        "BPE deviation {:.2}% >= 5% (rust={bpe_text_tokens}, python={python_tokens})",
        deviation * 100.0,
    );
}

#[test]
fn heuristic_path_still_works() {
    let heuristic = HeuristicEstimator::default();
    let tokens = heuristic.estimate_string("Hello world");
    assert!(tokens > 0);
}
