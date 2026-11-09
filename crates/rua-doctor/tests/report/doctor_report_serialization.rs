use rua_doctor::registry::{CheckResult, CheckStatus};
use rua_doctor::{
    AgentWorkspaceDiag, ConfigDiagnostic, DefaultModelDiag, DoctorReport, ProviderDiagEntry,
    ProvidersDiagnostic, RuntimeInfo, VersionInfo, WorkspaceDiagnostic,
};

fn make_realistic_checks() -> Vec<CheckResult> {
    let version = VersionInfo {
        rua_version: "0.9.3".into(),
        rust_version: "1.82.0".into(),
    };
    let runtime = RuntimeInfo {
        platform: "macos".into(),
        arch: "aarch64".into(),
        working_dir: "/Users/testuser/.rua-rust".into(),
        working_dir_exists: true,
        working_dir_writable: true,
        runtime_authority: None,
    };
    let config = ConfigDiagnostic {
        config_path: "/Users/testuser/.rua-rust/config.json".into(),
        config_exists: true,
        config_parseable: true,
        parse_error: None,
        agent_count: 4,
        active_agent: "default-agent".into(),
    };
    let providers = ProvidersDiagnostic {
        secret_dir: "/Users/testuser/.rua-rust.secret".into(),
        secret_dir_exists: true,
        providers_dir_exists: true,
        providers: vec![
            ProviderDiagEntry {
                id: "openai-prod".into(),
                name: "OpenAI Production".into(),
                has_api_key: true,
                has_base_url: true,
                model_count: 5,
            },
            ProviderDiagEntry {
                id: "azure-eastus".into(),
                name: "Azure East US".into(),
                has_api_key: true,
                has_base_url: true,
                model_count: 3,
            },
            ProviderDiagEntry {
                id: "anthropic-main".into(),
                name: "Anthropic Main".into(),
                has_api_key: true,
                has_base_url: true,
                model_count: 4,
            },
            ProviderDiagEntry {
                id: "ollama-local".into(),
                name: "Ollama Local".into(),
                has_api_key: false,
                has_base_url: true,
                model_count: 8,
            },
            ProviderDiagEntry {
                id: "idealab-internal".into(),
                name: "Idealab Internal Gateway".into(),
                has_api_key: true,
                has_base_url: true,
                model_count: 6,
            },
        ],
        default_model: Some(DefaultModelDiag {
            provider_id: "openai-prod".into(),
            model: "gpt-4o".into(),
        }),
    };
    let workspace = WorkspaceDiagnostic {
        agents: vec![
            AgentWorkspaceDiag {
                agent_id: "default-agent".into(),
                workspace_dir: "~/.rua-rust/workspaces/default-agent".into(),
                enabled: true,
                workspace_exists: true,
                agent_json_exists: true,
            },
            AgentWorkspaceDiag {
                agent_id: "code-review".into(),
                workspace_dir: "~/.rua-rust/workspaces/code-review".into(),
                enabled: true,
                workspace_exists: true,
                agent_json_exists: true,
            },
            AgentWorkspaceDiag {
                agent_id: "ops-bot".into(),
                workspace_dir: "~/.rua-rust/workspaces/ops-bot".into(),
                enabled: false,
                workspace_exists: true,
                agent_json_exists: false,
            },
        ],
    };

    vec![
        CheckResult {
            name: "version".into(),
            status: CheckStatus::Pass,
            message: "rua v0.9.3".into(),
            details: Some(serde_json::to_value(&version).unwrap()),
        },
        CheckResult {
            name: "runtime".into(),
            status: CheckStatus::Pass,
            message: "macos/aarch64".into(),
            details: Some(serde_json::to_value(&runtime).unwrap()),
        },
        CheckResult {
            name: "config".into(),
            status: CheckStatus::Pass,
            message: "4 agents configured".into(),
            details: Some(serde_json::to_value(&config).unwrap()),
        },
        CheckResult {
            name: "providers".into(),
            status: CheckStatus::Pass,
            message: "5 providers found".into(),
            details: Some(serde_json::to_value(&providers).unwrap()),
        },
        CheckResult {
            name: "workspace".into(),
            status: CheckStatus::Pass,
            message: "3 agent workspaces".into(),
            details: Some(serde_json::to_value(&workspace).unwrap()),
        },
        CheckResult {
            name: "disk".into(),
            status: CheckStatus::Pass,
            message: "disk ok".into(),
            details: Some(serde_json::json!({"status": "ok"})),
        },
        CheckResult {
            name: "security_baseline".into(),
            status: CheckStatus::Pass,
            message: "security ok".into(),
            details: Some(serde_json::json!({"status": "ok"})),
        },
        CheckResult {
            name: "mcp_client".into(),
            status: CheckStatus::Warn,
            message: "no mcp servers configured".into(),
            details: Some(serde_json::json!({"status": "warn", "servers": []})),
        },
    ]
}

#[test]
fn single_serialization_no_typed_fields_in_json() {
    let report = DoctorReport {
        checks: make_realistic_checks(),
    };
    let json_val: serde_json::Value = serde_json::to_value(&report).unwrap();
    let obj = json_val.as_object().unwrap();

    assert!(obj.contains_key("checks"), "must have checks field");
    assert!(!obj.contains_key("version"), "typed field must not appear");
    assert!(!obj.contains_key("runtime"), "typed field must not appear");
    assert!(!obj.contains_key("config"), "typed field must not appear");
    assert!(
        !obj.contains_key("providers"),
        "typed field must not appear"
    );
    assert!(
        !obj.contains_key("workspace"),
        "typed field must not appear"
    );
    assert!(
        !obj.contains_key("cron_jobs"),
        "typed field must not appear"
    );
}

#[test]
fn round_trip_preserves_typed_fields() {
    let report = DoctorReport {
        checks: make_realistic_checks(),
    };

    let json = serde_json::to_string(&report).unwrap();
    let deserialized: DoctorReport = serde_json::from_str(&json).unwrap();

    assert_eq!(
        report.version().rua_version,
        deserialized.version().rua_version
    );
    assert_eq!(report.runtime().platform, deserialized.runtime().platform);
    assert_eq!(report.runtime().arch, deserialized.runtime().arch);
    assert_eq!(
        report.config().config_exists,
        deserialized.config().config_exists
    );
    assert_eq!(
        report.providers().providers.len(),
        deserialized.providers().providers.len()
    );
    assert_eq!(
        report.workspace().agents.len(),
        deserialized.workspace().agents.len()
    );
    assert_eq!(report.checks.len(), deserialized.checks.len());

    for (orig, deser) in report.checks.iter().zip(deserialized.checks.iter()) {
        assert_eq!(orig.name, deser.name);
        assert_eq!(orig.message, deser.message);
        assert_eq!(orig.details, deser.details);
    }
}

#[test]
fn json_size_reduced_at_least_30_percent() {
    let checks = make_realistic_checks();
    let report = DoctorReport {
        checks: checks.clone(),
    };

    let new_json = serde_json::to_string(&report).unwrap();

    let old_json = serde_json::to_string(&OldDoctorReport::from_checks(&checks)).unwrap();

    let reduction = 1.0 - (new_json.len() as f64 / old_json.len() as f64);
    assert!(
        reduction >= 0.30,
        "JSON size reduction {:.1}% must be >= 30%. old={}, new={}",
        reduction * 100.0,
        old_json.len(),
        new_json.len(),
    );
}

#[derive(serde::Serialize)]
struct OldDoctorReport {
    version: VersionInfo,
    runtime: RuntimeInfo,
    config: ConfigDiagnostic,
    providers: ProvidersDiagnostic,
    workspace: WorkspaceDiagnostic,
    checks: Vec<CheckResult>,
}

impl OldDoctorReport {
    fn from_checks(checks: &[CheckResult]) -> Self {
        OldDoctorReport {
            version: rua_doctor::extract_typed(checks, "version").unwrap_or_default(),
            runtime: rua_doctor::extract_typed(checks, "runtime").unwrap_or_default(),
            config: rua_doctor::extract_typed(checks, "config").unwrap_or_default(),
            providers: rua_doctor::extract_typed(checks, "providers").unwrap_or_default(),
            workspace: rua_doctor::extract_typed(checks, "workspace").unwrap_or_default(),
            checks: checks.to_vec(),
        }
    }
}
