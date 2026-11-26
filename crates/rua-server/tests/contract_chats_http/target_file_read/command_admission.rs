use super::*;

fn request<'a>(f: &'a Fixture, args: &'a Value) -> TargetChatRead<'a> {
    TargetChatRead {
        tool_name: "bash",
        ..f.request(args)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_preparation_checks_actual_target_directory_without_running_it() {
    let f = Fixture::materialize(build_agent().await).await;
    let cwd = f.app.state.rua_root.join("command-target");
    std::fs::create_dir(&cwd).unwrap();
    let actual = cwd.canonicalize().unwrap();
    let args = json!({"device_id":f.device,"cwd":cwd,"command":"printf side-effect > created"});
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), actual.to_str().unwrap(), false)
            .await,
        Ok(FileReadAdmission::Allowed)
    );
    assert!(!cwd.join("created").exists());
    let approvals = f
        .app
        .state
        .security
        .approval_authority()
        .snapshot_at(&f.address)
        .await;
    assert!(approvals.pending.is_empty() && approvals.resolved.is_empty());

    let other = f.app.state.rua_root.join("different-target");
    std::fs::create_dir(&other).unwrap();
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), other.to_str().unwrap(), true)
            .await,
        Err(TargetReadRejection::Denied)
    );
    let mut wrong_device = args.clone();
    wrong_device["device_id"] = "another-device".into();
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &wrong_device), actual.to_str().unwrap(), true)
            .await,
        Err(TargetReadRejection::Denied)
    );
    let mut missing = args;
    missing["cwd"] = json!(cwd.join("missing"));
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &missing), actual.to_str().unwrap(), true)
            .await,
        Err(TargetReadRejection::FileUnavailable)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_start_rechecks_live_selection_and_captured_authority() {
    let f = Fixture::materialize(build_agent().await).await;
    let cwd = f.app.state.rua_root.canonicalize().unwrap();
    let args = json!({"device_id":f.device,"command":"printf checked"});
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), cwd.to_str().unwrap(), false)
            .await,
        Ok(FileReadAdmission::Allowed)
    );
    let store = &f.app.state.tool_config_store;
    let mut selection = ToolSelectionPolicy::default_enabled();
    selection.set_tool_enabled(ToolIdentity::builtin("bash"), false);
    let preview = store
        .preview_selection_policy(Some(AGENT), selection.clone())
        .await
        .unwrap();
    store
        .set_selection_policy(Some(AGENT), preview.expected, selection)
        .await
        .unwrap();
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), cwd.to_str().unwrap(), true)
            .await,
        Err(TargetReadRejection::Denied)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_approval_never_overrides_an_explicit_deny_or_opens_a_target_card() {
    let f = Fixture::materialize(build_agent().await).await;
    let cwd = f.app.state.rua_root.canonicalize().unwrap();
    let args = json!({"device_id":f.device,"command":"printf protected"});
    let security = &f.app.state.security;
    let mut policy = security.config_store().load(AGENT).unwrap();
    policy.tool_guard.approval_mode = "ask_first".into();
    security.save_and_invalidate(AGENT, &policy).unwrap();
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), cwd.to_str().unwrap(), false)
            .await,
        Ok(FileReadAdmission::RequiresUserConfirmation)
    );
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), cwd.to_str().unwrap(), true)
            .await,
        Ok(FileReadAdmission::Allowed)
    );
    security
        .approval_authority()
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Do not run this command".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("printf protected".into()),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.context
            .inspect_command(&request(&f, &args), cwd.to_str().unwrap(), true)
            .await,
        Err(TargetReadRejection::Denied)
    );
    let approvals = security.approval_authority().snapshot_at(&f.address).await;
    assert!(approvals.pending.is_empty() && approvals.resolved.is_empty());
}
