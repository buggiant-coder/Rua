use super::*;

fn paths(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf, Value) {
    let source = f.app.state.rua_root.join("selected");
    let destination = f.app.state.rua_root.join("destination");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    let file = source.join("photo.jpg");
    std::fs::write(&file, "selected photo").unwrap();
    let target = destination.join("photo.jpg");
    let args = json!({"source_device_id":f.device,"source_paths":[source],
        "target_device_id":f.device,"target_dir":destination,"conflict":"fail"});
    (file, target, args)
}

fn request<'a>(f: &'a Fixture, args: &'a Value) -> TargetChatRead<'a> {
    TargetChatRead {
        tool_name: "sync_send",
        ..f.request(args)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_checks_each_selected_side_without_reading_or_writing_files() {
    let f = Fixture::materialize(build_agent().await).await;
    let (source, target, args) = paths(&f);
    let actual_source = source.canonicalize().unwrap();
    let actual_target = target
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join("photo.jpg");
    assert_eq!(
        f.context
            .inspect_transfer_source(
                &request(&f, &args),
                source.to_str().unwrap(),
                actual_source.to_str().unwrap(),
                false
            )
            .await,
        Ok(FileReadAdmission::Allowed)
    );
    assert_eq!(
        f.context
            .inspect_transfer_target(
                &request(&f, &args),
                target.to_str().unwrap(),
                actual_target.to_str().unwrap(),
                false
            )
            .await,
        Ok(FileReadAdmission::Allowed)
    );
    assert!(!target.exists());
    assert_eq!(std::fs::read_to_string(source).unwrap(), "selected photo");
    let snapshot = f
        .app
        .state
        .security
        .approval_authority()
        .snapshot_at(&f.address)
        .await;
    assert!(snapshot.pending.is_empty() && snapshot.resolved.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_cannot_switch_owning_device_or_escape_the_explicit_collection() {
    let f = Fixture::materialize(build_agent().await).await;
    let (source, target, args) = paths(&f);
    let outside = f.app.state.rua_root.join("not-selected.jpg");
    std::fs::write(&outside, "not selected").unwrap();
    for (requested, resolved) in [(&outside, &outside), (&source, &outside)] {
        assert_eq!(
            f.context
                .inspect_transfer_source(
                    &request(&f, &args),
                    requested.to_str().unwrap(),
                    resolved.to_str().unwrap(),
                    true
                )
                .await,
            Err(TargetReadRejection::Denied)
        );
    }
    let mut wrong = args.clone();
    wrong["target_device_id"] = "another-device".into();
    assert_eq!(
        f.context
            .inspect_transfer_target(
                &request(&f, &wrong),
                target.to_str().unwrap(),
                target.to_str().unwrap(),
                true
            )
            .await,
        Err(TargetReadRejection::Denied)
    );
    #[cfg(unix)]
    {
        let escape_dir = f.app.state.rua_root.join("outside");
        std::fs::create_dir(&escape_dir).unwrap();
        let link = target.parent().unwrap().join("link");
        std::os::unix::fs::symlink(&escape_dir, &link).unwrap();
        assert_eq!(
            f.context
                .inspect_transfer_target(
                    &request(&f, &args),
                    link.join("photo.jpg").to_str().unwrap(),
                    escape_dir.join("photo.jpg").to_str().unwrap(),
                    true
                )
                .await,
            Err(TargetReadRejection::Denied)
        );
        assert!(!escape_dir.join("photo.jpg").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_read_and_write_denials_are_checked_even_after_confirmation() {
    let f = Fixture::materialize(build_agent().await).await;
    let (source, target, args) = paths(&f);
    for (tool, input) in [
        ("read", json!({"device_id":f.device,"file_path":source})),
        ("edit", json!({"device_id":f.device,"path":target})),
    ] {
        f.app
            .state
            .security
            .approval_authority()
            .create_agent_rule(
                AGENT,
                PermissionRuleInput {
                    label: format!("Deny {tool} on the selected path"),
                    enabled: true,
                    effect: PermissionRuleEffect::Deny,
                    matcher: ApprovalReuseMatcher::ToolCall {
                        tool_name: tool.into(),
                        input_params: Some(input.to_string()),
                    },
                },
            )
            .await
            .unwrap();
        let result = if tool == "read" {
            f.context
                .inspect_transfer_source(
                    &request(&f, &args),
                    source.to_str().unwrap(),
                    source.to_str().unwrap(),
                    true,
                )
                .await
        } else {
            f.context
                .inspect_transfer_target(
                    &request(&f, &args),
                    target.to_str().unwrap(),
                    target.to_str().unwrap(),
                    true,
                )
                .await
        };
        assert_eq!(result, Err(TargetReadRejection::Denied));
    }
    assert!(!target.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_requires_current_chat_authority_configuration_and_selected_tool() {
    let f = Fixture::materialize(build_agent().await).await;
    let (source, _, args) = paths(&f);
    for invalid in [
        TargetChatRead {
            authority_device_id: "obsolete-device",
            ..request(&f, &args)
        },
        TargetChatRead {
            configuration_epoch: "obsolete-config",
            ..request(&f, &args)
        },
    ] {
        assert_eq!(
            f.context
                .inspect_transfer_source(
                    &invalid,
                    source.to_str().unwrap(),
                    source.to_str().unwrap(),
                    true
                )
                .await,
            Err(TargetReadRejection::Unavailable)
        );
    }
    let mut candidate = ToolSelectionPolicy::default_enabled();
    candidate.set_tool_enabled(ToolIdentity::builtin("sync_send"), false);
    let store = &f.app.state.tool_config_store;
    let preview = store
        .preview_selection_policy(Some(AGENT), candidate.clone())
        .await
        .unwrap();
    store
        .set_selection_policy(Some(AGENT), preview.expected, candidate)
        .await
        .unwrap();
    assert_eq!(
        f.context
            .inspect_transfer_source(
                &request(&f, &args),
                source.to_str().unwrap(),
                source.to_str().unwrap(),
                true
            )
            .await,
        Err(TargetReadRejection::Denied)
    );
}
