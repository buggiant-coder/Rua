use std::collections::HashSet;

use rua_server::route_compat::{CompatRouteStatus, COMPAT_ROUTE_LEDGER};

#[test]
fn compat_route_ledger_records_reason_and_exit_condition_for_each_surface() {
    assert!(
        !COMPAT_ROUTE_LEDGER.is_empty(),
        "compat route ledger must not be empty"
    );

    let mut surfaces = HashSet::new();
    for entry in COMPAT_ROUTE_LEDGER {
        assert!(
            surfaces.insert(entry.surface),
            "duplicate compat route surface: {}",
            entry.surface
        );
        assert!(
            !entry.compat_routes.is_empty(),
            "{} must list at least one compat route",
            entry.surface
        );
        assert!(
            !entry.canonical_routes.is_empty(),
            "{} must list at least one canonical route",
            entry.surface
        );
        assert!(
            !entry.retention_reason.trim().is_empty(),
            "{} must record why the compat route remains",
            entry.surface
        );
        assert!(
            !entry.exit_condition.trim().is_empty(),
            "{} must record when the compat route can be removed",
            entry.surface
        );
        for route in entry.compat_routes {
            assert!(
                route.starts_with("/api/") || route.starts_with("/voice/"),
                "{} compat route must be an absolute server route: {}",
                entry.surface,
                route
            );
        }
    }
}

#[test]
fn active_root_agent_mirrors_point_at_agent_scoped_canonical_routes() {
    for entry in COMPAT_ROUTE_LEDGER
        .iter()
        .filter(|entry| entry.status == CompatRouteStatus::Active)
    {
        let has_agent_scoped_canonical = entry
            .canonical_routes
            .iter()
            .any(|route| route.contains("/agents/{agentId}/"));
        let has_non_agent_canonical = matches!(
            entry.surface,
            "security_approvals" | "dingtalk_webhook" | "voice_asr"
        );
        assert!(
            has_agent_scoped_canonical || has_non_agent_canonical,
            "{} active compat entry must point at an agent-scoped canonical route or document a non-agent canonical route",
            entry.surface
        );
    }
}
