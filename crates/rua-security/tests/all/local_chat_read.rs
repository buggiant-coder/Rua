use std::path::Path;
use std::sync::Arc;

use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalDecision, ApprovalOwner, ApprovalRequestInput, ApprovalReuseMatcher,
    ApprovalReuseScope, ApprovalTrigger, PermissionRuleEffect, PermissionRuleInput,
    ToolCallSnapshot,
};
use rua_security::security_module::{FileReadAdmission, FileReadPolicyError};
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

fn open_security(root: &Path) -> SecurityModule {
    SecurityModule::new(Arc::new(SecurityConfigStore::new(root.to_owned())), None)
}

fn ask_first(module: &SecurityModule) -> rua_config::types::security::SecurityConfig {
    let mut config = module.config_store().load(AGENT).unwrap();
    config.tool_guard.approval_mode = "ask_first".into();
    module.save_and_invalidate(AGENT, &config).unwrap();
    config
}

async fn allow(authority: &ApprovalAuthority, args: Value, scope: ApprovalReuseScope) {
    let request = authority
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
            Some("desktop"),
        )
        .await;
    let matcher = request
        .reuse_options
        .iter()
        .find(|o| o.scope == scope)
        .unwrap()
        .matcher
        .clone();
    let decision = match scope {
        ApprovalReuseScope::Chat => ApprovalDecision::AllowChat {
            matcher,
            decided_by: "console".into(),
        },
        ApprovalReuseScope::Agent => ApprovalDecision::AllowAgent {
            matcher,
            decided_by: "console".into(),
        },
        _ => unreachable!(),
    };
    authority
        .decide_for_authenticated_user(AGENT, &request.id, decision, "alice")
        .await
        .unwrap();
}

async fn approved(authority: &ApprovalAuthority, args: Value) {
    allow(authority, args, ApprovalReuseScope::Chat).await;
}

fn install_settings(
    source: &SecurityModule,
    target: &SecurityModule,
    source_root: &Path,
    target_root: &Path,
    scope: ApprovalReuseScope,
) {
    match scope {
        ApprovalReuseScope::Chat => {
            let relative = "chats/sessions/chat/permission_rules.json";
            let destination = target_root.join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            rua_config::atomic_write(
                &destination,
                &std::fs::read(source_root.join(relative)).unwrap(),
            )
            .unwrap();
        }
        ApprovalReuseScope::Agent => {
            target
                .save_and_invalidate(AGENT, &source.config_store().load(AGENT).unwrap())
                .unwrap();
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_desktop_permission_applies_after_target_restart_and_preserves_scope_device_path_and_window(
) {
    use FileReadAdmission::{Allowed, RequiresUserConfirmation};
    for scope in [ApprovalReuseScope::Chat, ApprovalReuseScope::Agent] {
        let desktop_dir = tempfile::tempdir().unwrap();
        let phone_dir = tempfile::tempdir().unwrap();
        let desktop = open_security(desktop_dir.path());
        let phone = open_security(phone_dir.path());
        ask_first(&desktop);
        let captured = ask_first(&phone);
        let args = json!({"device_id":PHONE,"file_path":FILE,"limit":3});
        allow(desktop.approval_authority(), args.clone(), scope).await;
        install_settings(
            &desktop,
            &phone,
            desktop_dir.path(),
            phone_dir.path(),
            scope,
        );
        for target in [&phone, &open_security(phone_dir.path())] {
            let ctx = context(args.clone());
            assert!(target
                .approval_authority()
                .snapshot_at(&ChatAddress::new("chat"))
                .await
                .resolved
                .is_empty());
            assert_eq!(
                target
                    .inspect_chat_file_read(&ctx, &captured, &[FILE])
                    .await
                    .unwrap(),
                Allowed
            );
            for (key, value) in [
                ("device_id", json!("desktop")),
                ("file_path", json!("/fixture/report-1.txt")),
                ("limit", json!(4)),
            ] {
                let mut changed = ctx.clone();
                changed.tool_input[key] = value;
                let path = changed.tool_input["file_path"].as_str().unwrap();
                assert_eq!(
                    target
                        .inspect_chat_file_read(&changed, &captured, &[path])
                        .await
                        .unwrap(),
                    RequiresUserConfirmation,
                    "{scope:?} {key}"
                );
            }
            let other_chat = ctx
                .clone()
                .with_chat_address(ChatAddress::new("other-chat"));
            assert_eq!(
                target
                    .inspect_chat_file_read(&other_chat, &captured, &[FILE])
                    .await
                    .unwrap(),
                if scope == ApprovalReuseScope::Agent {
                    Allowed
                } else {
                    RequiresUserConfirmation
                }
            );
            if scope == ApprovalReuseScope::Agent {
                let mut other_agent = ctx;
                other_agent.agent_id = "other-agent".into();
                assert_eq!(
                    target
                        .inspect_chat_file_read(&other_agent, &captured, &[FILE])
                        .await
                        .unwrap(),
                    RequiresUserConfirmation
                );
            }
        }
    }
}

#[tokio::test]
async fn synchronized_revocation_wins_over_warm_rules_and_a_captured_allow() {
    use FileReadAdmission::{Allowed, RequiresUserConfirmation};
    for scope in [ApprovalReuseScope::Chat, ApprovalReuseScope::Agent] {
        let desktop_dir = tempfile::tempdir().unwrap();
        let phone_dir = tempfile::tempdir().unwrap();
        let desktop = open_security(desktop_dir.path());
        let phone = open_security(phone_dir.path());
        ask_first(&desktop);
        ask_first(&phone);
        let args = json!({"device_id":PHONE,"file_path":FILE});
        let ctx = context(args.clone());
        allow(desktop.approval_authority(), args.clone(), scope).await;
        install_settings(
            &desktop,
            &phone,
            desktop_dir.path(),
            phone_dir.path(),
            scope,
        );
        let captured = phone.config_store().load(AGENT).unwrap();
        assert_eq!(
            phone
                .inspect_chat_file_read(&ctx, &captured, &[FILE])
                .await
                .unwrap(),
            Allowed
        );
        assert!(phone
            .approval_authority()
            .find_reuse_rule_at(AGENT, &ChatAddress::new("chat"), "read", &args)
            .await
            .is_some());
        match scope {
            ApprovalReuseScope::Chat => {
                rua_config::atomic_write(
                    &desktop_dir.path().join("chats/sessions/chat/session.json"),
                    br#"{"chat_id":"chat"}"#,
                )
                .unwrap();
                desktop
                    .approval_authority()
                    .delete_chat_rules(&ChatAddress::new("chat"))
                    .await
                    .unwrap();
            }
            ApprovalReuseScope::Agent => {
                let rule = desktop
                    .approval_authority()
                    .list_agent_rules(AGENT)
                    .await
                    .pop()
                    .unwrap();
                assert!(desktop
                    .approval_authority()
                    .delete_agent_rule(AGENT, &rule.id)
                    .await
                    .unwrap());
            }
            _ => unreachable!(),
        }
        install_settings(
            &desktop,
            &phone,
            desktop_dir.path(),
            phone_dir.path(),
            scope,
        );
        for target in [&phone, &open_security(phone_dir.path())] {
            assert_eq!(
                target
                    .inspect_chat_file_read(&ctx, &captured, &[FILE])
                    .await
                    .unwrap(),
                RequiresUserConfirmation,
                "{scope:?}"
            );
            assert!(target
                .approval_authority()
                .find_reuse_rule_at(AGENT, &ChatAddress::new("chat"), "read", &args)
                .await
                .is_none());
        }
    }
}

#[tokio::test]
async fn a_confirmed_invocation_reads_a_new_file_and_still_obeys_explicit_backing_path_denials() {
    use FileReadAdmission::{Allowed, RequiresUserConfirmation};
    let dir = tempfile::tempdir().unwrap();
    let module = open_security(dir.path());
    let captured = ask_first(&module);
    let ctx = context(json!({"device_id":PHONE,"file_path":FILE}));
    let names = [FILE, "/fixture/backing.txt"];
    assert_eq!(
        module
            .inspect_chat_file_read(&ctx, &captured, &names)
            .await
            .unwrap(),
        RequiresUserConfirmation
    );
    assert_eq!(
        module
            .inspect_confirmed_chat_file_read(&ctx, &captured, &names, true)
            .await
            .unwrap(),
        Allowed
    );
    module
        .approval_authority()
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Restricted backing file".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(json!({"device_id":PHONE,"file_path":names[1]}).to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module
            .inspect_confirmed_chat_file_read(&ctx, &captured, &names, true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_hit_and_revocation_cannot_interleave_with_saving_the_same_rule() {
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
    let saving = tokio::spawn(async move { approved(&writer, approval_args).await });
    written.await.unwrap();

    let owner = ChatAddress::new("chat");
    let mut hit = Box::pin(authority.find_reuse_rule_at(AGENT, &owner, "read", &args));
    let mut revoke = Box::pin(authority.delete_chat_rules(&owner));
    let hit_pending = poll_fn(|cx| Poll::Ready(hit.as_mut().poll(cx).is_pending())).await;
    let revoke_pending = poll_fn(|cx| Poll::Ready(revoke.as_mut().poll(cx).is_pending())).await;
    store.resume.wait();
    saving.await.unwrap();
    assert!(
        hit_pending && revoke_pending,
        "a partial commit became observable"
    );
    let (matched, deleted) = tokio::join!(hit, revoke);
    assert!(matched.is_some());
    deleted.unwrap();
    assert!(store.list_chat_rules(&owner).unwrap().is_empty());
    assert!(authority
        .find_reuse_rule_at(AGENT, &owner, "read", &args)
        .await
        .is_none());
}
