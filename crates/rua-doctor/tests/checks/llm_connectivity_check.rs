use rua_config::dirs::Paths;
use rua_doctor::connectivity::SERVICES;
use rua_doctor::registry::{run_all_checks, CheckContext};

#[test]
fn services_contains_major_llm_providers() {
    let names: Vec<&str> = SERVICES.iter().map(|(name, _)| *name).collect();

    assert!(names.contains(&"openai"), "SERVICES must include openai");
    assert!(
        names.contains(&"anthropic"),
        "SERVICES must include anthropic"
    );
    assert!(names.contains(&"gemini"), "SERVICES must include gemini");
    assert!(names.contains(&"ollama"), "SERVICES must include ollama");
}

#[test]
fn connectivity_check_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-llm-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    assert!(
        results.iter().any(|r| r.name == "connectivity"),
        "connectivity check must be registered"
    );
}
