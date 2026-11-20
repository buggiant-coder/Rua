use std::path::Path;

use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
use rua_core::ChatAddress;
use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
use rua_server::services::{
    chat_file_read_admission::{
        FileReadAdmission, TargetChatRead, TargetFileReadContext, TargetReadRejection,
    },
    conversation_authority_placement::{
        parse_conversation_authority_placement, ConversationAuthorityEpoch,
    },
    device_context::current_sync_device_id,
};
use rua_tool::{core::read_file::OpenedRegularFile, ReadFileTool};
use serde_json::{json, Value};

use super::support::{build_app, create_agent, seed_materialized_chat, TestApp};

const AGENT: &str = "target-reader";
const AUTHORITY: &str = "0123456789abcdef";

struct Fixture {
    context: TargetFileReadContext,
    app: TestApp,
    address: ChatAddress,
    device: String,
    epoch: ConversationAuthorityEpoch,
    config_epoch: String,
}

impl Fixture {
    async fn materialize(app: TestApp) -> Self {
        let chat = seed_materialized_chat(&app, AGENT, "File read").await;
        let address = ChatAddress::new(chat["id"].as_str().unwrap());
        let device = current_sync_device_id(&app.state.sync_device).unwrap();
        let mut placement = parse_conversation_authority_placement(&chat["meta"]).unwrap();
        placement.device_id = AUTHORITY.into();
        let epoch =
            ConversationAuthorityEpoch::next_for_device(AUTHORITY, Some(placement.authority_epoch));
        placement.authority_epoch = epoch;
        let store = rua_session::WorkspaceSessionStore::new(app.state.chat_workspace().as_path());
        let mut header = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap();
        header
            .extra
            .insert("authority_placement".into(), json!(placement));
        store.save_chat_header(&header).await.unwrap();
        let config_epoch = chat["meta"]["hook_configuration"]["epoch"]
            .as_str()
            .unwrap()
            .to_owned();
        let state = &app.state;
        let context = TargetFileReadContext::new(
            state.rua_root.clone(),
            device.clone(),
            state.project_catalog.clone(),
            state.agent_service.clone(),
            state.tool_catalog.clone(),
            state.tool_config_store.clone(),
            state.security.clone(),
        );
        Self {
            context,
            app,
            address,
            device,
            epoch,
            config_epoch,
        }
    }

    fn request<'a>(&'a self, args: &'a Value) -> TargetChatRead<'a> {
        TargetChatRead {
            address: &self.address,
            agent_id: AGENT,
            authority_device_id: AUTHORITY,
            authority_epoch: self.epoch,
            configuration_epoch: &self.config_epoch,
            turn_id: "turn-1",
            tool_call_id: "tool-1",
            tool_name: "read",
            args,
        }
    }

    async fn open(&self, path: &Path) -> (OpenedRegularFile, Value) {
        let file = ReadFileTool
            .open_regular_file(path, path.parent().unwrap())
            .await
            .unwrap();
        let args = json!({"device_id":self.device,"file_path":path});
        (file, args)
    }
}

async fn build_agent() -> TestApp {
    let app = build_app();
    create_agent(&app.app, AGENT, "Target reader").await;
    app
}

async fn select_read(app: &TestApp, enabled: bool) {
    let mut candidate = ToolSelectionPolicy::default_enabled();
    candidate.set_tool_enabled(ToolIdentity::builtin("read"), enabled);
    let store = &app.state.tool_config_store;
    let preview = store
        .preview_selection_policy(Some(AGENT), candidate.clone())
        .await
        .unwrap();
    store
        .set_selection_policy(Some(AGENT), preview.expected, candidate)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_denies_names_protected_by_the_file_tool_before_any_file_is_opened() {
    let f = Fixture::materialize(build_agent().await).await;
    let parent = f.app.state.rua_root.join("test-only");
    for (directory, expected) in [
        ("public", Ok(())),
        (".azure", Err(TargetReadRejection::Denied)),
    ] {
        let args =
            json!({"device_id":f.device, "file_path":parent.join(directory).join("sample.txt")});
        let request = f.request(&args);
        assert_eq!(f.context.check_authority(&request).await, Ok(()));
        assert_eq!(f.context.check_metadata(&request).await, expected);
    }
    assert!(!parent.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_rejects_only_after_checking_the_replica_without_probing_the_file() {
    let f = Fixture::materialize(build_agent().await).await;
    let path = f.app.state.rua_root.join("metadata-only.txt");
    let args = json!({"device_id": f.device, "file_path": path});
    let request = f.request(&args);
    assert_eq!(f.context.check_authority(&request).await, Ok(()));
    assert_eq!(f.context.check_metadata(&request).await, Ok(()));

    for stale in [
        TargetChatRead {
            authority_device_id: "other-device",
            ..f.request(&args)
        },
        TargetChatRead {
            authority_epoch: ConversationAuthorityEpoch::next_for_device(AUTHORITY, Some(f.epoch)),
            ..f.request(&args)
        },
        TargetChatRead {
            configuration_epoch: "obsolete",
            ..f.request(&args)
        },
        TargetChatRead {
            agent_id: "other",
            ..f.request(&args)
        },
    ] {
        assert_eq!(
            f.context.check_authority(&stale).await,
            Err(TargetReadRejection::Unavailable)
        );
        assert_eq!(
            f.context.check_metadata(&stale).await,
            Err(TargetReadRejection::Unavailable)
        );
    }
    for invalid in [
        json!({"byte_offset":0,"start_line":1}),
        json!({"byte_offset":1,"limit":1}),
        json!({"file_path":"/invalid\0path"}),
        json!({"device_id":AUTHORITY}),
    ] {
        let mut invalid_args = args.clone();
        invalid_args
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert_eq!(
            f.context.check_authority(&f.request(&invalid_args)).await,
            Err(TargetReadRejection::Denied)
        );
    }
    let authority = f.app.state.security.approval_authority();
    authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Private local label".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(json!({"file_path":path}).to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(f.context.check_authority(&request).await, Ok(()));
    assert_eq!(
        f.context.check_metadata(&request).await,
        Err(TargetReadRejection::Denied)
    );
    std::fs::write(&path, "exists only in the local test fixture").unwrap();
    assert_eq!(
        f.context.check_metadata(&request).await,
        Err(TargetReadRejection::Denied)
    );

    let security = &f.app.state.security;
    let mut unreadable_policy = security.config_store().load(AGENT).unwrap();
    unreadable_policy
        .extra
        .insert("permission_rules".into(), json!("invalid"));
    security
        .save_and_invalidate(AGENT, &unreadable_policy)
        .unwrap();
    assert_eq!(f.context.check_authority(&request).await, Ok(()));
    assert_eq!(
        f.context.check_metadata(&request).await,
        Err(TargetReadRejection::Unavailable)
    );

    rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(&f.app.state.rua_root)
        .persist(&f.address)
        .unwrap();
    assert_eq!(
        f.context.check_authority(&request).await,
        Err(TargetReadRejection::Unavailable)
    );
    assert_eq!(
        f.context.check_metadata(&request).await,
        Err(TargetReadRejection::Unavailable)
    );
    assert!(authority.snapshot_at(&f.address).await.pending.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_current_live_chat_and_matching_authority_can_use_the_target_policy() {
    let f = Fixture::materialize(build_agent().await).await;
    let path = f.app.state.rua_root.join("visible.txt");
    std::fs::write(&path, "fixture").unwrap();
    let (file, args) = f.open(&path).await;
    let state = &f.app.state;
    assert_eq!(
        f.context
            .inspect_opened(&f.request(&args), &file)
            .await
            .unwrap(),
        FileReadAdmission::Allowed
    );
    let rejects = [
        (
            TargetChatRead {
                authority_device_id: "other-device",
                ..f.request(&args)
            },
            "authority",
        ),
        (
            TargetChatRead {
                authority_epoch: ConversationAuthorityEpoch::next_for_device(
                    &f.device,
                    Some(f.epoch),
                ),
                ..f.request(&args)
            },
            "epoch",
        ),
        (
            TargetChatRead {
                configuration_epoch: "obsolete",
                ..f.request(&args)
            },
            "configuration epoch",
        ),
        (
            TargetChatRead {
                agent_id: "other",
                ..f.request(&args)
            },
            "Agent",
        ),
    ];
    for (request, reason) in rejects {
        assert!(f
            .context
            .inspect_opened(&request, &file)
            .await
            .unwrap_err()
            .contains(reason));
    }
    for fields in [
        json!({"device_id":AUTHORITY}),
        json!({"file_path":path.with_extension("other")}),
        json!({"start_line":0}),
        json!({"after":"a-directory-entry"}),
    ] {
        let mut other = args.clone();
        other
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(f
            .context
            .inspect_opened(&f.request(&other), &file)
            .await
            .is_err());
    }
    let store = rua_session::WorkspaceSessionStore::new(state.chat_workspace().as_path());
    let id = f.address.chat_id().as_str();
    let mut header = store.load_chat_header(id).await.unwrap().unwrap();
    header.metadata.archived = true;
    store.save_chat_header(&header).await.unwrap();
    assert!(f
        .context
        .inspect_opened(&f.request(&args), &file)
        .await
        .unwrap_err()
        .contains("archived"));
    rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(&state.rua_root)
        .persist(&f.address)
        .unwrap();
    assert!(f
        .context
        .inspect_opened(&f.request(&args), &file)
        .await
        .unwrap_err()
        .contains("unavailable"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_must_be_selected_in_both_the_saved_chat_and_target_agent() {
    for captured_disabled in [false, true] {
        let app = build_agent().await;
        if captured_disabled {
            select_read(&app, false).await;
        }
        let f = Fixture::materialize(app).await;
        select_read(&f.app, captured_disabled).await;
        let path = f.app.state.rua_root.join("selected.txt");
        std::fs::write(&path, "fixture").unwrap();
        let (file, args) = f.open(&path).await;
        assert!(f
            .context
            .inspect_opened(&f.request(&args), &file)
            .await
            .unwrap_err()
            .contains("disabled"));
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_ask_first_never_creates_a_second_approval_and_resolved_file_deny_wins() {
    let f = Fixture::materialize(build_agent().await).await;
    let security = &f.app.state.security;
    let marker = f.app.state.rua_root.join("restricted.txt");
    std::fs::write(&marker, "test marker").unwrap();
    let alias = f.app.state.rua_root.join("selected.txt");
    std::os::unix::fs::symlink(&marker, &alias).unwrap();
    let (file, args) = f.open(&alias).await;
    let mut policy = security.config_store().load(AGENT).unwrap();
    policy
        .file_guard
        .sensitive_files
        .push("**/restricted.txt".into());
    security.save_and_invalidate(AGENT, &policy).unwrap();
    assert_eq!(
        f.context
            .inspect_opened(&f.request(&args), &file)
            .await
            .unwrap(),
        FileReadAdmission::RequiresUserConfirmation,
        "the sensitive-file rule matches the resolved path behind the selected alias"
    );
    policy.file_guard.sensitive_files.clear();
    policy.tool_guard.approval_mode = "ask_first".into();
    security.save_and_invalidate(AGENT, &policy).unwrap();
    let authority = security.approval_authority();
    assert_eq!(
        f.context
            .inspect_opened(&f.request(&args), &file)
            .await
            .unwrap(),
        FileReadAdmission::RequiresUserConfirmation
    );
    assert!(authority.snapshot_at(&f.address).await.pending.is_empty());
    authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Previously allowed marker reads".into(),
                enabled: true,
                effect: PermissionRuleEffect::Allow,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(args.to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.context
            .inspect_opened(&f.request(&args), &file)
            .await
            .unwrap(),
        FileReadAdmission::Allowed
    );
    let mut other_window = args.clone();
    other_window["start_line"] = json!(2);
    assert_eq!(
        f.context
            .inspect_opened(&f.request(&other_window), &file)
            .await
            .unwrap(),
        FileReadAdmission::RequiresUserConfirmation
    );
    assert!(authority.snapshot_at(&f.address).await.pending.is_empty());
    authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Restricted local read".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(json!({"file_path": file.backing_path()}).to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert!(f
        .context
        .inspect_opened(&f.request(&args), &file)
        .await
        .unwrap_err()
        .contains("Restricted local read"));
    assert!(authority.snapshot_at(&f.address).await.pending.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_the_agent_or_a_captured_assignment_disables_its_target_read() {
    let f = Fixture::materialize(build_agent().await).await;
    let path = f.app.state.rua_root.join("assigned.txt");
    std::fs::write(&path, "fixture").unwrap();
    let (file, args) = f.open(&path).await;
    for enabled in [false, true] {
        let response = super::support::request(
            &f.app.app,
            axum::http::Method::PATCH,
            &format!("/api/agents/{AGENT}/toggle"),
            Some(json!({"enabled":enabled})),
            None,
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let admission = f.context.inspect_opened(&f.request(&args), &file).await;
        if enabled {
            assert_eq!(admission.unwrap(), FileReadAdmission::Allowed);
        } else {
            assert!(admission.unwrap_err().contains("Agent is disabled"));
        }
    }
    let store = rua_session::WorkspaceSessionStore::new(f.app.state.chat_workspace().as_path());
    let id = f.address.chat_id().as_str();
    let mut header = store.load_chat_header(id).await.unwrap().unwrap();
    let mut captured = header.extra.remove("hook_configuration").unwrap();
    let epoch = "disabled-assignment";
    captured["epoch"] = json!(epoch);
    captured["agent_assignment"]["disabled_tools"] = json!(["read"]);
    store
        .replace_chat_configuration(id, &f.config_epoch, captured)
        .await
        .unwrap();
    let updated = TargetChatRead {
        configuration_epoch: epoch,
        ..f.request(&args)
    };
    assert!(f
        .context
        .inspect_opened(&updated, &file)
        .await
        .unwrap_err()
        .contains("read tool is disabled"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_deny_in_the_captured_chat_survives_a_more_permissive_target_config() {
    let app = build_agent().await;
    let rule = app
        .state
        .security
        .approval_authority()
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Captured read restriction".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let f = Fixture::materialize(app).await;
    let authority = f.app.state.security.approval_authority();
    assert!(authority.delete_agent_rule(AGENT, &rule.id).await.unwrap());
    let path = f.app.state.rua_root.join("captured.txt");
    std::fs::write(&path, "fixture").unwrap();
    let (file, args) = f.open(&path).await;
    assert!(f
        .context
        .inspect_opened(&f.request(&args), &file)
        .await
        .unwrap_err()
        .contains("Captured read restriction"));
    assert!(authority.snapshot_at(&f.address).await.pending.is_empty());
}
