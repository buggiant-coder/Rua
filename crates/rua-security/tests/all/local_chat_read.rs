use std::sync::Arc;

use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalDecision, ApprovalOwner, ApprovalRequest, ApprovalRequestInput,
    ApprovalReuseMatcher, ApprovalReuseScope, ApprovalTrigger, PermissionRuleEffect,
    PermissionRuleInput, ToolCallSnapshot,
};
use rua_security::{SecurityConfigStore, SecurityModule, ToolCallContext};
use serde_json::{json, Value};

const PHONE: &str = "phone";
const FILE: &str = "/fixture/report-*.txt";
const AGENT: &str = "agent";

fn context(args: Value) -> ToolCallContext {
    let mut ctx = ToolCallContext::new(AGENT, "chat", "read", args)
        .with_chat_address(ChatAddress::new("chat"));
    ctx.local_device_id = Some(PHONE.into());
    ctx
}

async fn register(
    authority: &ApprovalAuthority,
    args: Value,
    device: Option<&str>,
) -> ApprovalRequest {
    authority
        .create_request_for_device(
            ApprovalRequestInput {
                agent_id: AGENT.into(),
                owner: ApprovalOwner::chat(ChatAddress::new("chat")),
                turn_id: "turn".into(),
                tool_call_id: "call".into(),
                tool_name: "read".into(),
                trigger: ApprovalTrigger::approval_mode("Ask first"),
                tool_call: ToolCallSnapshot {
                    display: "read".into(),
                    args,
                },
            },
            device,
        )
        .await
}

fn allow_chat(request: &ApprovalRequest) -> ApprovalDecision {
    ApprovalDecision::AllowChat {
        matcher: request
            .reuse_options
            .iter()
            .find(|o| o.scope == ApprovalReuseScope::Chat)
            .unwrap()
            .matcher
            .clone(),
        decided_by: "console".into(),
    }
}

fn allow_agent(request: &ApprovalRequest) -> ApprovalDecision {
    let selected = request
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::Agent)
        .unwrap();
    ApprovalDecision::AllowAgent {
        matcher: selected.matcher.clone(),
        decided_by: "console".into(),
    }
}

async fn approved(authority: &ApprovalAuthority, args: Value) {
    let request = register(authority, args, Some(PHONE)).await;
    authority
        .decide_for_authenticated_user(AGENT, &request.id, allow_chat(&request), "alice")
        .await
        .unwrap();
}

async fn lookup(authority: &ApprovalAuthority, ctx: &ToolCallContext) -> Option<String> {
    authority
        .locally_approved_chat_read(ctx, PHONE)
        .await
        .unwrap()
}

fn open_security(root: &std::path::Path) -> SecurityModule {
    SecurityModule::new(Arc::new(SecurityConfigStore::new(root.to_owned())), None)
}

#[tokio::test]
async fn authenticated_allow_chat_preserves_agent_device_file_and_full_window() {
    let authority = ApprovalAuthority::new();
    let args = json!({"file_path":FILE,"start_line":2,"limit":1});
    approved(&authority, args.clone()).await;
    let mut ctx = context(args);
    assert_eq!(lookup(&authority, &ctx).await.as_deref(), Some(FILE));
    ctx.tool_input["device_id"] = PHONE.into();
    assert_eq!(lookup(&authority, &ctx).await.as_deref(), Some(FILE));
    let mut changed = ctx
        .clone()
        .with_chat_address(ChatAddress::new("other-chat"));
    assert_eq!(lookup(&authority, &changed).await, None);
    changed = ctx.clone();
    changed.agent_id = "other-agent".into();
    assert_eq!(lookup(&authority, &changed).await, None);
    assert_eq!(
        authority
            .locally_approved_chat_read(&ctx, "other-device")
            .await
            .unwrap(),
        None
    );
    for (key, value) in [
        ("device_id", json!("other")),
        ("file_path", json!("/fixture/report-1.txt")),
        ("limit", json!(5)),
        ("byte_offset", json!(0)),
        ("offset", json!(0)),
    ] {
        changed = ctx.clone();
        changed.tool_input[key] = value;
        assert_eq!(lookup(&authority, &changed).await, None, "{key}");
    }

    authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Deny local read".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(lookup(&authority, &ctx).await, None);
}

#[tokio::test]
async fn only_authenticated_device_bound_chat_approval_qualifies() {
    let args = json!({"file_path": FILE});
    for outcome in [
        "pending",
        "reject",
        "run_once",
        "agent_hook",
        "hook",
        "unbound",
    ] {
        let authority = ApprovalAuthority::new();
        let request = register(
            &authority,
            args.clone(),
            (outcome != "unbound").then_some(PHONE),
        )
        .await;
        match outcome {
            "pending" => {}
            "hook" | "agent_hook" => {
                let decision = if outcome == "agent_hook" {
                    allow_agent(&request)
                } else {
                    allow_chat(&request)
                };
                authority.decide(&request.id, decision).await.unwrap();
            }
            _ => {
                let decision = match outcome {
                    "reject" => ApprovalDecision::Reject {
                        decided_by: "console".into(),
                    },
                    "run_once" => ApprovalDecision::RunOnce {
                        decided_by: "console".into(),
                    },
                    "unbound" => allow_chat(&request),
                    _ => unreachable!(),
                };
                authority
                    .decide_for_authenticated_user(AGENT, &request.id, decision, "alice")
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            lookup(&authority, &context(args.clone())).await,
            None,
            "{outcome}"
        );
    }
}

#[tokio::test]
async fn directory_pagination_media_and_ambiguous_names_never_select_a_file() {
    for args in [
        json!({"file_path": "/folder/"}),
        json!({"file_path": "/folder/../elsewhere"}),
        json!({"file_path": FILE, "directory_offset": 1}),
        json!({"file_path": FILE, "start_line": 3, "byte_offset": 2}),
        json!({"file_path": "media/55fe8dd6-4dd7-4f7a-95d4-2c01aea1620e"}),
    ] {
        let authority = ApprovalAuthority::new();
        approved(&authority, args.clone()).await;
        assert_eq!(lookup(&authority, &context(args)).await, None);
    }
}

#[tokio::test]
async fn disabled_replaced_and_revoked_rules_cannot_supply_local_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let module = open_security(dir.path());
    let authority = module.approval_authority();
    let args = json!({"device_id":PHONE,"file_path":FILE});
    let ctx = context(args.clone());
    let chat = ChatAddress::new("chat");
    approved(authority, args.clone()).await;
    let captured = ctx
        .clone()
        .with_security_snapshot(Arc::new(module.config_store().load(AGENT).unwrap()));
    assert!(module.enforce(&captured, &[]).await.is_allow());
    assert!(authority.list_chat_rules(&chat).await[0].hit_count > 0);
    let store = dir.path().join("chats/sessions/chat/permission_rules.json");
    let original: Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    for (key, value) in [
        ("enabled", json!(false)),
        ("id", json!("different-rule")),
        ("source_approval_id", json!("another-decision")),
        ("exact_read_args", json!(false)),
        (
            "matcher",
            json!({"type":"tool_call","tool_name":"read","input_params":null}),
        ),
    ] {
        let mut changed = original.clone();
        changed[0][key] = value;
        rua_config::atomic_write(&store, &serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(lookup(authority, &ctx).await, None, "{key}");
    }
    let original = serde_json::to_vec(&original).unwrap();
    rua_config::atomic_write(&store, &original).unwrap();
    let reopened = open_security(dir.path());
    assert!(reopened
        .approval_authority()
        .find_reuse_rule_at(AGENT, &chat, "read", &args)
        .await
        .is_some());
    assert_eq!(
        reopened
            .approval_authority()
            .locally_approved_chat_read(&ctx, PHONE)
            .await
            .unwrap(),
        None
    );

    authority.delete_chat_rules(&chat).await.unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
    rua_config::atomic_write(&store, &original).unwrap();
    assert!(authority
        .find_reuse_rule_at(AGENT, &chat, "read", &args)
        .await
        .is_some());
    assert_eq!(lookup(authority, &ctx).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_hit_and_revocation_cannot_interleave_with_signing_the_same_rule() {
    use std::future::{poll_fn, Future};
    use std::sync::{Barrier, Mutex};
    use std::task::Poll;

    use rua_security::approval_authority::{
        AgentScopeRuleStore, ApprovalReuseRule, ChatScopeRuleStore,
    };

    // Pause a real AllowChat decision immediately after its store write, before the cache update.
    #[derive(Debug)]
    struct PausedStore {
        rule: Mutex<Option<ApprovalReuseRule>>,
        wrote: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        resume: Barrier,
    }
    impl ChatScopeRuleStore for PausedStore {
        fn list_chat_rules(&self, _: &ChatAddress) -> Result<Vec<ApprovalReuseRule>, String> {
            Ok(self.rule.lock().unwrap().iter().cloned().collect())
        }
        fn upsert_chat_rule(
            &self,
            _: &ChatAddress,
            rule: &ApprovalReuseRule,
        ) -> Result<(), String> {
            *self.rule.lock().unwrap() = Some(rule.clone());
            let notify = self.wrote.lock().unwrap().take();
            if let Some(wrote) = notify {
                wrote.send(()).unwrap();
                self.resume.wait();
            }
            Ok(())
        }
        fn delete_chat_rules(&self, _: &ChatAddress) -> Result<(), String> {
            self.rule.lock().unwrap().take();
            Ok(())
        }
    }
    impl AgentScopeRuleStore for PausedStore {
        fn list_agent_rules(&self, _: &str) -> Result<Vec<ApprovalReuseRule>, String> {
            Ok(Vec::new())
        }
        fn upsert_agent_rule(&self, _: &str, _: &ApprovalReuseRule) -> Result<(), String> {
            unreachable!()
        }
        fn delete_agent_rule(&self, _: &str, _: &str) -> Result<bool, String> {
            unreachable!()
        }
    }

    let (notify, written) = tokio::sync::oneshot::channel();
    let store = Arc::new(PausedStore {
        rule: Mutex::new(None),
        wrote: Mutex::new(Some(notify)),
        resume: Barrier::new(2),
    });
    let authority = Arc::new(ApprovalAuthority::with_rule_stores(
        store.clone(),
        store.clone(),
    ));
    let args = json!({"device_id": PHONE, "file_path":FILE});
    let writer = Arc::clone(&authority);
    let approval_args = args.clone();
    let signing = tokio::spawn(async move { approved(&writer, approval_args).await });
    written.await.unwrap();

    let owner = ChatAddress::new("chat");
    let mut hit = Box::pin(authority.find_reuse_rule_at(AGENT, &owner, "read", &args));
    let mut revoke = Box::pin(authority.delete_chat_rules(&owner));
    let hit_pending = poll_fn(|cx| Poll::Ready(hit.as_mut().poll(cx).is_pending())).await;
    let revoke_pending = poll_fn(|cx| Poll::Ready(revoke.as_mut().poll(cx).is_pending())).await;
    store.resume.wait();
    signing.await.unwrap();
    assert!(
        hit_pending && revoke_pending,
        "a partial commit became observable"
    );
    let (matched, deleted) = tokio::join!(hit, revoke);
    assert!(matched.is_some());
    deleted.unwrap();
    assert!(store.list_chat_rules(&owner).unwrap().is_empty());
    assert_eq!(lookup(&authority, &context(args)).await, None);
}

#[tokio::test]
async fn local_agent_approval_applies_only_to_its_file_and_window_in_another_chat() {
    let temp = tempfile::tempdir().unwrap();
    let security = open_security(temp.path());
    let authority = security.approval_authority();
    let args = json!({"file_path":FILE,"limit":3});
    let pending = register(authority, args.clone(), Some(PHONE)).await;
    authority
        .decide_for_authenticated_user(AGENT, &pending.id, allow_agent(&pending), "alice")
        .await
        .unwrap();
    let mut ctx = context(args).with_chat_address(ChatAddress::new("next-chat"));
    ctx.tool_input["device_id"] = PHONE.into();
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    for (key, value) in [
        ("device_id", json!("other")),
        ("file_path", json!("/fixture/report-1.txt")),
        ("limit", json!(4)),
    ] {
        let mut changed = ctx.clone();
        changed.tool_input[key] = value;
        assert_eq!(lookup(authority, &changed).await, None, "{key}");
    }
    let mut other_agent = ctx.clone();
    other_agent.agent_id = "other".into();
    assert_eq!(lookup(authority, &other_agent).await, None);
    let reader = open_security(temp.path());
    assert!(reader
        .approval_authority()
        .find_reuse_rule_at(
            AGENT,
            &ChatAddress::new("next-chat"),
            "read",
            &ctx.tool_input
        )
        .await
        .is_some());
    assert_eq!(lookup(reader.approval_authority(), &ctx).await, None);

    let rule = authority.list_agent_rules(AGENT).await.pop().unwrap();
    assert!(!authority
        .delete_agent_rule("other", &rule.id)
        .await
        .unwrap());
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    authority.delete_agent_rule(AGENT, &rule.id).await.unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
}

#[tokio::test]
async fn synced_agent_policy_can_revoke_but_cannot_mint_a_local_file_approval() {
    let temp = tempfile::tempdir().unwrap();
    let security = open_security(temp.path());
    let authority = security.approval_authority();
    let args = json!({"file_path":FILE,"device_id":PHONE});
    let pending = register(authority, args.clone(), Some(PHONE)).await;
    authority
        .decide_for_authenticated_user(AGENT, &pending.id, allow_agent(&pending), "alice")
        .await
        .unwrap();
    let ctx = context(args.clone()).with_chat_address(ChatAddress::new("next-chat"));
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    let mut config = security.config_store().load(AGENT).unwrap();
    let original = config.clone();
    config.extra.insert("permission_rules".into(), json!([]));
    security.save_and_invalidate(AGENT, &config).unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
    assert!(authority
        .find_reuse_rule_at(AGENT, &ChatAddress::new("next-chat"), "read", &args)
        .await
        .is_none());
    assert_eq!(lookup(authority, &ctx).await, None);
    assert_eq!(
        security.config_store().load(AGENT).unwrap().extra["permission_rules"],
        json!([])
    );
    security.save_and_invalidate(AGENT, &original).unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
}

#[tokio::test]
async fn agent_file_approval_obeys_deny_and_cannot_be_widened_in_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let security = open_security(tmp.path());
    let authority = security.approval_authority();
    let args = json!({"file_path":FILE,"device_id":PHONE});
    let pending = register(authority, args.clone(), Some(PHONE)).await;
    authority
        .decide_for_authenticated_user(AGENT, &pending.id, allow_agent(&pending), "alice")
        .await
        .unwrap();
    let ctx = context(args).with_chat_address(ChatAddress::new("next-chat"));
    let original = authority.list_agent_rules(AGENT).await.pop().unwrap();
    let mut edit = PermissionRuleInput {
        label: "Rename unchanged rule".into(),
        matcher: original.matcher.clone(),
        enabled: true,
        effect: PermissionRuleEffect::Allow,
    };
    authority
        .update_agent_rule(AGENT, &original.id, edit.clone())
        .await
        .unwrap();
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    let any_read = ApprovalReuseMatcher::ToolCall {
        tool_name: "read".into(),
        input_params: None,
    };
    let deny = authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                enabled: true,
                label: "Deny current read".into(),
                effect: PermissionRuleEffect::Deny,
                matcher: any_read.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
    authority.delete_agent_rule(AGENT, &deny.id).await.unwrap();
    assert_eq!(lookup(authority, &ctx).await.as_deref(), Some(FILE));
    edit.matcher = any_read;
    authority
        .update_agent_rule(AGENT, &original.id, edit)
        .await
        .unwrap();
    assert_eq!(lookup(authority, &ctx).await, None);
    let mut other = ctx;
    other.tool_input["file_path"] = "/fixture/report-1.txt".into();
    assert_eq!(lookup(authority, &other).await, None);
}

#[tokio::test]
async fn warming_agent_reads_preserves_a_new_deny_in_current_device_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let security = open_security(tmp.path());
    let authority = security.approval_authority();
    let args = json!({"file_path":FILE,"device_id":PHONE});
    let pending = register(authority, args.clone(), Some(PHONE)).await;
    authority
        .decide_for_authenticated_user(AGENT, &pending.id, allow_agent(&pending), "alice")
        .await
        .unwrap();
    let manual = authority
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                enabled: true,
                effect: PermissionRuleEffect::Allow,
                label: "Temporary allow".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let mut config = security.config_store().load(AGENT).unwrap();
    let rules = config
        .extra
        .get_mut("permission_rules")
        .unwrap()
        .as_array_mut()
        .unwrap();
    rules
        .iter_mut()
        .find(|rule| rule["id"] == manual.id)
        .unwrap()["effect"] = json!("deny");
    security.save_and_invalidate(AGENT, &config).unwrap();
    let chat = ChatAddress::new("next-chat");
    let ctx = context(args.clone()).with_chat_address(chat.clone());
    assert_eq!(lookup(authority, &ctx).await, None);
    // Looking up the surviving genuine allow must not restore the old manual allow by writing its hit count.
    assert!(authority
        .find_reuse_rule_at(AGENT, &chat, "read", &args)
        .await
        .is_some());
    assert_eq!(lookup(authority, &ctx).await, None);
}
