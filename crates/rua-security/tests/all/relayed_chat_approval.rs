use std::sync::Arc;

use rua_core::{ChatAddress, ChatStore};
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalAuthorityError, ApprovalDecision, ApprovalOwner, ApprovalRequest,
    ApprovalRequestInput, ApprovalReuseScope, ApprovalStatus, ApprovalTrigger,
    AuthenticatedApprovalDecision, ToolCallSnapshot,
};
use rua_security::{SecurityConfigStore, SecurityModule};
use serde_json::json;

fn open_authority(root: &std::path::Path) -> Arc<ApprovalAuthority> {
    SecurityModule::new(Arc::new(SecurityConfigStore::new(root.to_owned())), None)
        .approval_authority()
        .clone()
}

async fn original_card(execution: &ApprovalAuthority) -> ApprovalRequest {
    execution
        .create_request_for_device(
            ApprovalRequestInput {
                agent_id: "agent".into(),
                owner: ApprovalOwner::chat(ChatAddress::new("chat")),
                turn_id: "turn".into(),
                tool_call_id: "task:photo:attempt:1:tool".into(),
                tool_name: "bash".into(),
                trigger: ApprovalTrigger::approval_mode("Ask before running"),
                tool_call: ToolCallSnapshot {
                    display: "Read photos on phone".into(),
                    args: json!({"command":"ls /photos", "cwd":"/photos", "device_id":"phone"}),
                },
            },
            Some("phone"),
        )
        .await
}

fn decision(card: &ApprovalRequest) -> ApprovalDecision {
    ApprovalDecision::AllowChat {
        decided_by: "webui".into(),
        matcher: card
            .reuse_options
            .iter()
            .find(|option| option.scope == ApprovalReuseScope::Chat)
            .expect("Chat reuse option")
            .matcher
            .clone(),
    }
}

#[tokio::test]
async fn canonical_authority_persists_before_execution_applies_the_same_allow_chat_card() {
    let a_root = tempfile::tempdir().unwrap();
    let b_root = tempfile::tempdir().unwrap();
    let authority = open_authority(a_root.path());
    let execution = open_authority(b_root.path());
    let original = original_card(&execution).await;
    let address = original.owner.as_chat_address().unwrap();
    let mut events = authority.subscribe();

    let approved = authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .unwrap();
    assert_eq!(approved.id, original.id);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(
        approved
            .authenticated_user_decision
            .as_ref()
            .unwrap()
            .decision,
        AuthenticatedApprovalDecision::AllowChat
    );
    assert_eq!(events.try_recv().unwrap().event, "approval.updated");
    assert!(
        events.try_recv().is_err(),
        "no second pending card is published"
    );
    assert!(authority.snapshot_at(address).await.pending.is_empty());
    let persisted = open_authority(a_root.path()).list_chat_rules(address).await;
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].source_approval_id, original.id);
    assert_eq!(execution.request(&original.id).await.unwrap(), original);
    assert!(execution.list_chat_rules(address).await.is_empty());

    assert_eq!(
        execution
            .apply_chat_authority_decision(&original, &approved)
            .await
            .unwrap(),
        approved
    );
    assert!(
        execution
            .invocation_is_approved(
                &original.agent_id,
                &original.owner,
                &original.turn_id,
                &original.tool_call_id,
                &original.tool_name,
                &original.tool_call.args,
            )
            .await
    );
    assert!(
        !execution
            .invocation_is_approved(
                &original.agent_id,
                &original.owner,
                &original.turn_id,
                "task:photo:attempt:2:tool",
                &original.tool_name,
                &original.tool_call.args,
            )
            .await
    );
    assert!(execution.list_chat_rules(address).await.is_empty());
    assert!(open_authority(b_root.path())
        .list_chat_rules(address)
        .await
        .is_empty());
    assert!(!ChatStore::resolve(b_root.path(), address.clone())
        .evergreen_chat_root()
        .join("permission_rules.json")
        .exists());
}

#[tokio::test]
async fn repeated_allow_chat_keeps_one_rule_and_does_not_recreate_a_revoked_rule() {
    let root = tempfile::tempdir().unwrap();
    let authority = open_authority(root.path());
    let execution = ApprovalAuthority::new();
    let original = original_card(&execution).await;
    let address = original.owner.as_chat_address().unwrap();
    let approved = authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .unwrap();
    let rules = authority.list_chat_rules(address).await;
    for card in [&original, &approved] {
        assert_eq!(
            authority
                .decide_relayed_for_authenticated_user(card, decision(card), "user")
                .await
                .unwrap(),
            approved
        );
        assert_eq!(
            execution
                .apply_chat_authority_decision(card, &approved)
                .await
                .unwrap(),
            approved
        );
    }
    assert_eq!(authority.list_chat_rules(address).await, rules);
    assert!(authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "another-user")
        .await
        .is_err());
    authority.delete_chat_rules(address).await.unwrap();
    assert!(authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .is_err());
    assert!(authority.list_chat_rules(address).await.is_empty());
    assert!(open_authority(root.path())
        .list_chat_rules(address)
        .await
        .is_empty());
}

#[tokio::test]
async fn failed_authority_store_leaves_no_mirror_pending_or_successful_decision() {
    let root = tempfile::tempdir().unwrap();
    let authority = open_authority(root.path());
    let execution = ApprovalAuthority::new();
    let original = original_card(&execution).await;
    let address = original.owner.as_chat_address().unwrap();
    let chat = ChatStore::resolve(root.path(), address.clone());
    std::fs::create_dir_all(chat.root()).unwrap();
    let blocker = chat.root().join("sessions");
    std::fs::write(&blocker, "prevent writing the canonical Chat rule").unwrap();
    let mut events = authority.subscribe();
    assert!(matches!(
        authority
            .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
            .await,
        Err(ApprovalAuthorityError::Store(_))
    ));
    assert!(authority.request(&original.id).await.is_none());
    let snapshot = authority.snapshot_at(address).await;
    assert!(snapshot.pending.is_empty());
    assert!(snapshot.resolved.is_empty());
    assert!(events.try_recv().is_err());
    assert_eq!(execution.request(&original.id).await.unwrap(), original);

    std::fs::remove_file(blocker).unwrap();
    authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .unwrap();
    assert_eq!(authority.list_chat_rules(address).await.len(), 1);
}

#[tokio::test]
async fn relayed_recovery_cannot_invent_local_evidence_or_change_original_identity() {
    let authority = ApprovalAuthority::new();
    let execution = ApprovalAuthority::new();
    let original = original_card(&execution).await;
    let approved = authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .unwrap();
    let fresh = ApprovalAuthority::new();
    assert!(matches!(
        fresh
            .decide_relayed_for_authenticated_user(&approved, decision(&approved), "user")
            .await,
        Err(ApprovalAuthorityError::NotFound(_))
    ));
    assert!(fresh.request(&original.id).await.is_none());
    let mut changed = original.clone();
    changed.tool_call.args["cwd"] = json!("/other");
    assert!(authority
        .decide_relayed_for_authenticated_user(&changed, decision(&changed), "user")
        .await
        .is_err());
    let mut changed = original.clone();
    changed.reuse_options.clear();
    assert!(authority
        .decide_relayed_for_authenticated_user(&changed, decision(&original), "user")
        .await
        .is_err());
    assert!(fresh
        .decide_relayed_for_authenticated_user(&original, decision(&original), " ")
        .await
        .is_err());
    assert!(fresh.request(&original.id).await.is_none());
    assert_eq!(authority.request(&original.id).await.unwrap(), approved);
}

#[tokio::test]
async fn execution_rejects_different_decisions_and_never_revives_a_cancelled_card() {
    let authority = ApprovalAuthority::new();
    let execution = ApprovalAuthority::new();
    let original = original_card(&execution).await;
    let approved = authority
        .decide_relayed_for_authenticated_user(&original, decision(&original), "user")
        .await
        .unwrap();
    let mut changed = approved.clone();
    changed.tool_call.args["device_id"] = json!("desktop");
    assert!(execution
        .apply_chat_authority_decision(&original, &changed)
        .await
        .is_err());
    let mut changed = approved.clone();
    changed
        .authenticated_user_decision
        .as_mut()
        .unwrap()
        .decision = AuthenticatedApprovalDecision::RunOnce;
    assert!(execution
        .apply_chat_authority_decision(&original, &changed)
        .await
        .is_err());
    let mut changed = approved.clone();
    changed.authenticated_user_decision = None;
    assert!(execution
        .apply_chat_authority_decision(&original, &changed)
        .await
        .is_err());
    assert_eq!(execution.request(&original.id).await.unwrap(), original);
    execution.supersede_request(&original.id).await;
    assert!(execution
        .apply_chat_authority_decision(&original, &approved)
        .await
        .is_err());
    assert_eq!(
        execution.request(&original.id).await.unwrap().status,
        ApprovalStatus::Superseded
    );
    assert!(execution
        .list_chat_rules(original.owner.as_chat_address().unwrap())
        .await
        .is_empty());
}
