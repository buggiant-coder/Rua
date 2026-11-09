use rua_skills_hub::bundle::normalize_bundle;
use serde_json::json;

#[test]
fn normalize_bundle_adds_required_frontmatter_when_market_content_lacks_it() {
    let raw = json!({
        "name": "debugger",
        "description": "Diagnose failures from logs and stack traces.",
        "files": {
            "SKILL.md": "# Debugger\n\nUse when debugging errors."
        }
    });

    let normalized = normalize_bundle(&raw).expect("normalized");

    assert!(normalized.content.starts_with("---\n"));
    assert!(normalized.content.contains("name: debugger"));
    assert!(normalized
        .content
        .contains("description: Diagnose failures from logs and stack traces."));
    assert!(normalized.content.contains("# Debugger"));
}

#[test]
fn normalize_bundle_preserves_complete_frontmatter() {
    let skill_md = "---\nname: demo\ndescription: demo desc\n---\nbody";
    let raw = json!({
        "name": "demo",
        "files": {
            "SKILL.md": skill_md
        }
    });

    let normalized = normalize_bundle(&raw).expect("normalized");

    assert_eq!(normalized.content, skill_md);
}

#[test]
fn normalize_bundle_merges_missing_name_into_existing_frontmatter() {
    let raw = json!({
        "name": "debugger",
        "files": {
            "SKILL.md": "---\ndescription: debug errors\nauthor: BytesAgain\n---\n# debugger"
        }
    });

    let normalized = normalize_bundle(&raw).expect("normalized");

    assert!(normalized.content.contains("description: debug errors"));
    assert!(normalized.content.contains("author: BytesAgain"));
    assert!(normalized.content.contains("name: debugger\n---\n"));
    assert!(normalized.content.contains("# debugger"));
}
