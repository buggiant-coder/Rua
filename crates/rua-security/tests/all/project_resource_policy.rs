use rua_config::types::security::SecurityConfig;
use rua_security::{
    EffectiveProjectResourcePolicy, ProjectResourcePolicySource, ProjectResourceProfile,
    PROJECT_RESOURCE_POLICY_KEY,
};
use serde_json::json;

#[test]
fn project_policy_defaults_to_resource_write_at_resource_root() {
    let root = tempfile::tempdir().unwrap();
    let policy = EffectiveProjectResourcePolicy::resolve(&SecurityConfig::default(), root.path());

    assert_eq!(policy.profile, ProjectResourceProfile::ResourceWrite);
    assert_eq!(policy.source, ProjectResourcePolicySource::ProjectDefault);
    assert_eq!(policy.cwd, root.path().to_string_lossy());
    assert_eq!(policy.writable_roots, vec![policy.cwd.clone()]);
    assert!(policy.dedicated_operations_root_confined);
}

#[test]
fn agent_security_can_be_stricter_or_explicitly_full_access() {
    let root = tempfile::tempdir().unwrap();
    for (configured, expected) in [
        ("read-only", ProjectResourceProfile::ReadOnly),
        (
            "danger-full-access",
            ProjectResourceProfile::DangerFullAccess,
        ),
    ] {
        let mut config = SecurityConfig::default();
        config.extra.insert(
            PROJECT_RESOURCE_POLICY_KEY.to_string(),
            json!({"profile": configured}),
        );

        let policy = EffectiveProjectResourcePolicy::resolve(&config, root.path());

        assert_eq!(policy.profile, expected);
        assert_eq!(policy.source, ProjectResourcePolicySource::AgentSecurity);
        assert!(policy.writable_roots.is_empty());
    }
}

#[test]
fn dedicated_project_operations_stay_root_confined_under_full_access() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut config = SecurityConfig::default();
    config.extra.insert(
        PROJECT_RESOURCE_POLICY_KEY.to_string(),
        json!({"profile": "danger-full-access"}),
    );
    let policy = EffectiveProjectResourcePolicy::resolve(&config, root.path());

    assert!(policy
        .resolve_dedicated_path(std::path::Path::new("src/new.rs"))
        .is_err());
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    assert_eq!(
        policy
            .resolve_dedicated_path(std::path::Path::new("src/new.rs"))
            .unwrap(),
        root.path().canonicalize().unwrap().join("src/new.rs")
    );
    assert!(policy
        .resolve_dedicated_path(&outside.path().join("escape.txt"))
        .is_err());
}

#[test]
fn multiple_roots_do_not_grant_their_common_parent() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let docs = temp.path().join("docs");
    let policy = EffectiveProjectResourcePolicy::resolve_with_roots(
        &SecurityConfig::default(),
        &app,
        &[app.clone(), docs.clone()],
    );
    assert_eq!(policy.cwd, app.to_string_lossy());
    assert_eq!(
        policy.writable_roots,
        vec![app.to_string_lossy(), docs.to_string_lossy()]
    );
    assert!(!policy
        .writable_roots
        .contains(&temp.path().to_string_lossy().to_string()));
}

#[test]
fn actual_tool_guard_uses_each_root_and_preserves_external_read_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let docs = temp.path().join("docs");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&docs).unwrap();
    let engine = rua_security::ToolGuardEngine::new(&SecurityConfig::default(), None);
    let mut ctx = rua_security::ToolCallContext::new("agent", "chat", "edit", json!({}));
    ctx.working_dir = app.to_string_lossy().into_owned();
    ctx.metadata
        .insert("project_resource_roots".into(), json!([app, docs]));
    for (path, allowed) in [
        (app.join("new.rs"), true),
        (docs.join("new.md"), true),
        (temp.path().join("outside.txt"), false),
    ] {
        ctx.tool_input = json!({"patch": format!("*** Begin Patch\n*** Add File: {}\n+hello\n*** End Patch", path.display())});
        let result = engine.evaluate(&ctx);
        assert_eq!(
            !result
                .findings
                .iter()
                .any(|finding| finding.rule_id == "PROJECT_RESOURCE_WRITE"),
            allowed
        );
    }
    for patch in [
        format!(
            "*** Update File: {}\n*** Move to: {}\n@@\n-old\n+new",
            app.join("source").display(),
            temp.path().join("escaped").display()
        ),
        format!(
            "*** Add File: {}\n+new\n*** Delete File: {}",
            docs.join("new").display(),
            temp.path().join("outside").display()
        ),
    ] {
        ctx.tool_input = json!({"patch":patch});
        assert!(engine
            .evaluate(&ctx)
            .findings
            .iter()
            .any(|finding| finding.rule_id == "PROJECT_RESOURCE_WRITE"));
    }
    ctx.tool_name = "bash".into();
    ctx.tool_input = json!({"command": "ls /tmp"});
    assert_eq!(
        engine.evaluate(&ctx).decision,
        rua_security::GuardDecision::Allow
    );
    ctx.tool_input = json!({"command": "python script.py"});
    assert_eq!(
        engine.evaluate(&ctx).decision,
        rua_security::GuardDecision::RequiresApproval
    );
}

#[cfg(unix)]
#[test]
fn resource_write_resolves_symlink_destinations_and_missing_suffixes() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("file"), "secret").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    symlink(outside.path().join("file"), root.path().join("file")).unwrap();
    let policy = EffectiveProjectResourcePolicy::resolve(&SecurityConfig::default(), root.path());
    assert!(!policy.permits_write(&root.path().join("escape/new/sub/file")));
    assert!(!policy.permits_write(&root.path().join("file")));
    assert!(!policy.permits_write(&root.path().join("missing/../../outside")));
    assert!(policy.permits_write(&root.path().join("new/sub/file")));
}
