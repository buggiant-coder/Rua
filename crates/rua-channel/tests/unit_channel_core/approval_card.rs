use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rua_channel::core::approval_card::{
    build_callback_data, parse_callback_data, ApprovalCardAction, ApprovalCardMetadata,
    ApprovalCardParseError, ApprovalCardResolveError, ApprovalCardResolver,
};

#[test]
fn metadata_requires_request_id_and_builds_fallback_text() {
    assert_eq!(
        ApprovalCardMetadata::new("", "bash", "Approve?", "agent", "chat").unwrap_err(),
        ApprovalCardParseError::MissingRequestId
    );

    let meta = ApprovalCardMetadata::new("req-1", "bash", "Approve Bash?", "agent", "chat")
        .expect("metadata");
    let fallback = meta.fallback_text();
    assert!(fallback.contains("Approve Bash?"));
    assert!(fallback.contains("Tool: bash"));
    assert!(fallback.contains("Request: req-1"));
    assert!(fallback.contains("/approve"));
    assert!(fallback.contains("/deny"));
}

#[test]
fn callback_data_roundtrips_platform_actions() {
    assert_eq!(
        build_callback_data(ApprovalCardAction::Approve, "req-1"),
        "tga:req-1"
    );
    assert_eq!(
        build_callback_data(ApprovalCardAction::Deny, "req-1"),
        "tgd:req-1"
    );

    let approve = parse_callback_data("telegram", "tga:req-1", "user-1").unwrap();
    assert_eq!(approve.request_id, "req-1");
    assert_eq!(approve.action, ApprovalCardAction::Approve);
    assert_eq!(approve.decided_by, "user-1");
    assert_eq!(approve.platform, "telegram");

    let deny = parse_callback_data("feishu", "approval:deny:req-2", "user-2").unwrap();
    assert_eq!(deny.request_id, "req-2");
    assert_eq!(deny.action, ApprovalCardAction::Deny);
}

#[test]
fn callback_parse_rejects_missing_and_unknown_actions() {
    assert_eq!(
        parse_callback_data("telegram", "tga:", "user").unwrap_err(),
        ApprovalCardParseError::MissingRequestId
    );
    assert_eq!(
        parse_callback_data("telegram", "approval:maybe:req", "user").unwrap_err(),
        ApprovalCardParseError::UnknownAction
    );
    assert_eq!(
        parse_callback_data("telegram", "approval:approve", "user").unwrap_err(),
        ApprovalCardParseError::Malformed
    );
}

#[test]
fn resolver_allows_first_action_and_rejects_duplicate() {
    let mut resolver = ApprovalCardResolver::new(Duration::from_secs(300));
    let callback = parse_callback_data("telegram", "tga:req-1", "user").unwrap();
    let resolved = resolver.resolve(callback.clone()).unwrap();
    assert_eq!(resolved.request_id, "req-1");
    assert_eq!(resolved.action, ApprovalCardAction::Approve);
    assert_eq!(resolved.decided_by, "user");
    assert_eq!(resolved.command_text(), "/approval approve req-1");
    assert_eq!(resolved.authority_decision_kind(), "run_once");

    assert_eq!(
        resolver.resolve(callback).unwrap_err(),
        ApprovalCardResolveError::Duplicate
    );
}

#[test]
fn resolver_rejects_stale_callbacks_by_created_at() {
    let now = Arc::new(Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
    ));
    let now_for_resolver = now.clone();
    let resolver = ApprovalCardResolver::with_now(
        Duration::from_secs(60),
        Box::new(move || *now_for_resolver.lock().unwrap()),
    );

    assert_eq!(
        resolver
            .reject_stale(SystemTime::UNIX_EPOCH + Duration::from_secs(900))
            .unwrap_err(),
        ApprovalCardResolveError::Stale
    );
    resolver
        .reject_stale(SystemTime::UNIX_EPOCH + Duration::from_secs(950))
        .expect("fresh callback");
}

#[test]
fn resolver_maps_deny_to_resolution_for_authority_adapter() {
    let mut resolver = ApprovalCardResolver::new(Duration::from_secs(300));
    let callback = parse_callback_data("wecom", "tgd:req-deny", "manager").unwrap();
    let resolved = resolver.resolve(callback).unwrap();
    assert_eq!(resolved.request_id, "req-deny");
    assert_eq!(resolved.action, ApprovalCardAction::Deny);
    assert_eq!(resolved.decided_by, "manager");
    assert_eq!(resolved.command_text(), "/approval deny req-deny");
    assert_eq!(resolved.authority_decision_kind(), "reject");
}
