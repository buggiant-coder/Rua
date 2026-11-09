use rua_config::dirs::Paths;
use rua_doctor::{registry::CheckStatus, run_diagnostics_filtered};
use serde_json::Value;

fn setup_paths_with_secret_provider() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let working_dir = tmp.path().join("working");
    let secret_dir = tmp.path().join("secret");
    let providers_dir = secret_dir.join("providers");
    std::fs::create_dir_all(&providers_dir).expect("providers dir");
    std::fs::create_dir_all(&working_dir).expect("working dir");

    let provider = serde_json::json!({
        "id": "secret-provider",
        "name": "Secret Provider",
        "chat_model": "OpenAIChatModel",
        "base_url": "https://provider.example.test/v1",
        "api_key": "sk-doctor-contract-secret",
        "models": [
            {"id": "model-a", "name": "Model A"}
        ]
    });
    std::fs::write(
        providers_dir.join("secret-provider.json"),
        serde_json::to_vec_pretty(&provider).expect("provider json"),
    )
    .expect("write provider");

    (
        tmp,
        Paths {
            cache_dir: working_dir.with_extension("cache"),
            rua_root: working_dir,
            secret_dir,
        },
    )
}

#[test]
fn doctor_report_json_keeps_single_checks_surface_with_stable_result_shape() {
    let (_tmp, paths) = setup_paths_with_secret_provider();
    let report = run_diagnostics_filtered(&paths, false);
    let value = serde_json::to_value(&report).expect("report json");
    let root = value.as_object().expect("report object");
    assert_eq!(
        root.keys().collect::<Vec<_>>(),
        vec!["checks"],
        "doctor report should expose one public checks array"
    );

    let checks = root["checks"].as_array().expect("checks array");
    assert!(!checks.is_empty(), "doctor report must include checks");

    for check in checks {
        let obj = check.as_object().expect("check object");
        assert!(
            obj.contains_key("name"),
            "check must include name: {check:?}"
        );
        assert!(
            obj.contains_key("status"),
            "check must include status: {check:?}"
        );
        assert!(
            obj.contains_key("message"),
            "check must include message: {check:?}"
        );
        assert!(
            check["name"].as_str().is_some_and(|name| !name.is_empty()),
            "check name must be non-empty: {check:?}"
        );
        assert!(
            matches!(check["status"].as_str(), Some("pass" | "warn" | "fail")),
            "check status must be lowercase pass/warn/fail: {check:?}"
        );
        assert!(
            check["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "check message must be non-empty: {check:?}"
        );
        assert!(
            !obj.contains_key("severity") && !obj.contains_key("fixable"),
            "Rust doctor CheckResult shape is name/status/message/details: {check:?}"
        );
    }
}

#[test]
fn provider_diagnostics_expose_secret_presence_without_leaking_secret_values() {
    let (_tmp, paths) = setup_paths_with_secret_provider();
    let report = run_diagnostics_filtered(&paths, false);
    let providers = report.providers();
    assert_eq!(providers.providers.len(), 1);
    assert_eq!(providers.providers[0].id, "secret-provider");
    assert_eq!(providers.providers[0].name, "Secret Provider");
    assert!(providers.providers[0].has_api_key);
    assert!(providers.providers[0].has_base_url);
    assert_eq!(providers.providers[0].model_count, 1);

    let json = serde_json::to_string(&report).expect("report json");
    assert!(
        !json.contains("sk-doctor-contract-secret"),
        "doctor output must not leak provider API keys"
    );
    assert!(
        !json.contains("\"api_key\""),
        "doctor output must expose has_api_key only, not raw api_key fields"
    );
    assert!(
        json.contains("\"has_api_key\":true"),
        "doctor output should preserve secret presence as a boolean"
    );
}

#[test]
fn filtered_diagnostics_skip_network_checks_but_keep_local_contract_checks() {
    let (_tmp, paths) = setup_paths_with_secret_provider();
    let report = run_diagnostics_filtered(&paths, false);
    let names: std::collections::BTreeSet<&str> = report
        .checks
        .iter()
        .map(|check| check.name.as_str())
        .collect();

    assert!(names.contains("version"));
    assert!(names.contains("runtime"));
    assert!(names.contains("providers"));
    assert!(names.contains("workspace"));
    assert!(
        !names.contains("connectivity"),
        "network connectivity must be skipped when deep=false"
    );
    assert!(
        !names.contains("provider_http"),
        "provider HTTP probe must be skipped when deep=false"
    );

    assert!(
        report
            .checks
            .iter()
            .any(|check| matches!(check.status, CheckStatus::Pass | CheckStatus::Warn)),
        "local diagnostics should produce at least one non-fail status"
    );
}

#[test]
fn typed_extractors_return_defaults_when_details_are_absent_or_malformed() {
    let report = rua_doctor::DoctorReport {
        checks: vec![
            rua_doctor::registry::CheckResult {
                name: "version".into(),
                status: CheckStatus::Pass,
                message: "missing details".into(),
                details: None,
            },
            rua_doctor::registry::CheckResult {
                name: "providers".into(),
                status: CheckStatus::Warn,
                message: "malformed details".into(),
                details: Some(Value::String("not a provider diagnostic".into())),
            },
        ],
    };

    assert_eq!(report.version().rua_version, "");
    assert_eq!(report.version().rust_version, "");
    assert_eq!(report.providers().providers.len(), 0);
    assert!(!report.providers().secret_dir_exists);
}
