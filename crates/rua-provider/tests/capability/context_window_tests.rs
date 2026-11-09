use llm_multiplex::CapabilityBaseline;
use rua_provider::manager::infer_context_window;

#[test]
fn infer_deepseek_v4_pro() {
    assert_eq!(infer_context_window("deepseek-v4-pro"), Some(1_000_000));
}

#[test]
fn infer_claude_opus_4_8() {
    assert_eq!(infer_context_window("claude-opus-4-8"), Some(1_000_000));
}

#[test]
fn infer_minimax_m2() {
    assert_eq!(infer_context_window("minimax-m2"), Some(196_608));
}

#[test]
fn infer_qwen3_7_max() {
    assert_eq!(infer_context_window("qwen3.7-max"), Some(983_040));
}

#[test]
fn qwen3_7_max_not_matched_by_qwen3() {
    let val = infer_context_window("qwen3.7-max").unwrap();
    assert_eq!(
        val, 983_040,
        "qwen3.7-max should match its own prefix, not qwen3 (32768)"
    );
}

#[test]
fn baseline_claude_opus_4_returns_1m() {
    let info = CapabilityBaseline::lookup_fuzzy("claude-opus-4-20250514").unwrap();
    assert_eq!(info.context_window, Some(1_000_000));
}

#[test]
fn baseline_claude_sonnet_4_returns_1m() {
    let info = CapabilityBaseline::lookup_fuzzy("claude-sonnet-4-20250514").unwrap();
    assert_eq!(info.context_window, Some(1_000_000));
}

#[test]
fn baseline_claude_opus_4_7_returns_1m() {
    let info = CapabilityBaseline::lookup_fuzzy("claude-opus-4-7").unwrap();
    assert_eq!(info.context_window, Some(1_000_000));
}
