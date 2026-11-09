use std::collections::BTreeSet;

use rua_config::dirs::Paths;
use rua_doctor::registry::{run_all_checks_filtered, CheckContext, CheckStatus};

fn context_paths() -> Paths {
    let path = format!("/tmp/rua-doctor-registry-contract-{}", std::process::id());
    Paths::new(Some(&path))
}

fn check_names(results: &[rua_doctor::registry::CheckResult]) -> BTreeSet<&str> {
    results.iter().map(|check| check.name.as_str()).collect()
}

#[test]
fn deep_false_includes_core_local_checks_and_skips_network_checks() {
    let paths = context_paths();
    let ctx = CheckContext { paths: &paths };
    let (results, skipped) = run_all_checks_filtered(&ctx, false);
    let names = check_names(&results);

    for expected in [
        "version",
        "runtime",
        "config",
        "providers",
        "workspace",
        "disk",
        "app_log_writable",
        "agent_workspace_writable",
        "llamacpp_installation",
        "llamacpp_server_status",
        "security_baseline",
        "mcp_client",
        "workspace_hygiene",
        "agent_json_schema",
        "cron_jobs",
        "browser",
    ] {
        assert!(
            names.contains(expected),
            "local doctor check {expected} must run when deep=false; got {names:?}"
        );
    }

    for network in ["connectivity", "provider_http"] {
        assert!(
            !names.contains(network),
            "network doctor check {network} must be skipped when deep=false"
        );
    }
    assert!(
        skipped >= 2,
        "deep=false should skip registered network checks, got {skipped}"
    );
}

#[test]
fn deep_true_includes_network_checks_without_requiring_success() {
    let paths = context_paths();
    let ctx = CheckContext { paths: &paths };
    let (results, skipped) = run_all_checks_filtered(&ctx, true);
    let names = check_names(&results);

    assert_eq!(skipped, 0);
    for network in ["connectivity", "provider_http"] {
        assert!(
            names.contains(network),
            "network doctor check {network} must run when deep=true"
        );
    }
}

#[test]
fn registered_check_names_are_unique() {
    let paths = context_paths();
    let ctx = CheckContext { paths: &paths };
    let (results, _skipped) = run_all_checks_filtered(&ctx, true);
    let mut seen = BTreeSet::new();

    for check in &results {
        assert!(
            seen.insert(check.name.as_str()),
            "duplicate doctor check name registered: {}",
            check.name
        );
    }
}

#[test]
fn every_registered_check_returns_public_status_and_message_shape() {
    let paths = context_paths();
    let ctx = CheckContext { paths: &paths };
    let (results, _skipped) = run_all_checks_filtered(&ctx, true);

    assert!(!results.is_empty(), "doctor registry must not be empty");
    for check in results {
        assert!(!check.name.is_empty(), "check name must be non-empty");
        assert!(
            matches!(
                check.status,
                CheckStatus::Pass | CheckStatus::Warn | CheckStatus::Fail
            ),
            "check status must stay within the public pass/warn/fail enum"
        );
        assert!(
            !check.message.is_empty(),
            "check message must be non-empty for {}",
            check.name
        );
    }
}
