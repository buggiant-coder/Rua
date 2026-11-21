use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_types::ToolCall;
use async_trait::async_trait;
use rua_agent::runtime::agents_instructions::TurnAgentsInstructions;
use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscovery, AgentsInstructionDiscoveryRequest, AgentsInstructionLoader,
    AgentsInstructionSource, AgentsInstructionTargetKind, AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_core::agents_instructions::{AgentsInstructionDecision, AgentsInstructions};
use rua_core::tool::ToolContext;
use rua_core::{Result, RuaError};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn,
    RuntimeNotePayload, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Loader {
    files: Mutex<BTreeMap<(String, String), String>>,
    failed: Mutex<HashSet<String>>,
    requests: Mutex<Vec<AgentsInstructionDiscoveryRequest>>,
}
impl Loader {
    fn set(&self, device: &str, directory: &Path, text: &str) {
        self.files.lock().unwrap().insert(
            (device.into(), directory.display().to_string()),
            text.into(),
        );
    }
}
#[async_trait]
impl AgentsInstructionLoader for Loader {
    async fn discover(
        &self,
        request: &AgentsInstructionDiscoveryRequest,
        _: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<AgentsInstructionDiscovery> {
        if cancel.is_cancelled() {
            return Err(RuaError::Cancelled);
        }
        self.requests.lock().unwrap().push(request.clone());
        if self.failed.lock().unwrap().contains(&request.device_id) {
            return Err(RuaError::Io {
                message: "device temporarily unavailable".into(),
            });
        }
        let mut directory = PathBuf::new();
        for component in Path::new(&request.target_path).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    directory.pop();
                }
                part => directory.push(part.as_os_str()),
            }
        }
        if request.target_kind == AgentsInstructionTargetKind::File
            || (request.target_kind == AgentsInstructionTargetKind::Auto
                && directory.extension().is_some())
        {
            directory.pop();
        }
        let mut checked = directory
            .ancestors()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        checked.reverse();
        checked.retain(|directory| !request.checked_directories.contains(directory));
        let files = self.files.lock().unwrap();
        let sources = checked
            .iter()
            .filter_map(|scope| {
                let content = files.get(&(request.device_id.clone(), scope.clone()))?;
                Some(AgentsInstructionSource {
                    device_id: request.device_id.clone(),
                    path: Path::new(scope).join("AGENTS.md").display().to_string(),
                    scope_directory: scope.clone(),
                    content: content.clone(),
                    source_bytes: content.len() as u64,
                    ..Default::default()
                })
            })
            .collect();
        Ok(AgentsInstructionDiscovery {
            target_directory: Some(directory.display().to_string()),
            checked_directories: checked,
            sources,
            ..Default::default()
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    ctx: ToolContext,
    loader: Arc<Loader>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            local_device_id: Some("desktop".into()),
            working_dir: dir.path().canonicalize().unwrap(),
            session_id: "chat".into(),
            ..Default::default()
        };
        let loader = Arc::new(Loader::default());
        Self {
            _dir: dir,
            ctx,
            loader,
        }
    }
    async fn prepare(&self) -> TurnAgentsInstructions {
        TurnAgentsInstructions::prepare(
            self.loader.clone(),
            Some("Project resource list".into()),
            &self.ctx,
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }
    fn child(&self, name: &str) -> PathBuf {
        self.ctx.working_dir.join(name)
    }
    fn edit(&self, path: &Path) -> ToolCall {
        ToolCall::new(
            "edit-call",
            "edit",
            json!({"patch":format!("*** Begin Patch\n*** Add File: {}\n+hello\n*** End Patch",path.display())}),
        )
    }
    async fn before(
        &self,
        coordinator: &TurnAgentsInstructions,
        call: &ToolCall,
    ) -> AgentsInstructionDecision {
        coordinator
            .before_tool(call, &self.ctx, CancellationToken::new())
            .await
            .unwrap()
    }
}
fn reconsider(decision: AgentsInstructionDecision) {
    assert!(matches!(
        decision,
        AgentsInstructionDecision::Reconsider { .. }
    ));
}

#[tokio::test]
async fn new_scope_requires_model_visibility_for_every_parallel_call_but_not_other_scopes() {
    let f = Fixture::new();
    f.loader.set("desktop", &f.ctx.working_dir, "ROOT_RULE");
    f.loader.set("desktop", &f.child("photos"), "PHOTO_RULE");
    let c = f.prepare().await;
    let first = c.snapshot_for_model().unwrap();
    assert!(first.contains("ROOT_RULE") && first.contains("Project resource list"));
    let edit = f.edit(&f.child("photos/a.txt"));
    let (a, b) = tokio::join!(f.before(&c, &edit), f.before(&c, &edit));
    reconsider(a);
    reconsider(b);
    let unrelated = f.edit(&f.child("notes/b.txt"));
    assert_eq!(
        f.before(&c, &unrelated).await,
        AgentsInstructionDecision::Continue
    );
    let snapshot = c.snapshot_for_model().unwrap();
    assert!(
        snapshot.contains("PHOTO_RULE")
            && snapshot.contains(&f.child("photos").display().to_string())
    );
    assert!(snapshot.find("ROOT_RULE").unwrap() < snapshot.find("PHOTO_RULE").unwrap());
    assert_eq!(
        f.before(&c, &edit).await,
        AgentsInstructionDecision::Continue
    );
    assert_eq!(
        f.loader
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.target_path == f.child("photos/a.txt").display().to_string())
            .count(),
        1
    );
}

#[tokio::test]
async fn captured_content_is_fixed_within_turn_and_refreshed_next_turn_without_failure_backfill() {
    let f = Fixture::new();
    f.loader.set("desktop", &f.ctx.working_dir, "ORIGINAL_ROOT");
    let c = f.prepare().await;
    assert!(c.snapshot_for_model().unwrap().contains("ORIGINAL_ROOT"));
    f.loader.set("desktop", &f.ctx.working_dir, "CHANGED_ROOT");
    let edit = f.edit(&f.child("new-scope/a.txt"));
    assert_eq!(
        f.before(&c, &edit).await,
        AgentsInstructionDecision::Continue
    );
    assert!(c.snapshot_for_model().unwrap().contains("ORIGINAL_ROOT"));
    assert!(!c.snapshot_for_model().unwrap().contains("CHANGED_ROOT"));
    let refreshed = f.prepare().await;
    assert!(refreshed
        .snapshot_for_model()
        .unwrap()
        .contains("CHANGED_ROOT"));
    f.loader.failed.lock().unwrap().insert("desktop".into());
    let unavailable = f.prepare().await;
    let snapshot = unavailable.snapshot_for_model().unwrap();
    assert!(!snapshot.contains("CHANGED_ROOT") && !snapshot.contains("ORIGINAL_ROOT"));
    assert!(snapshot.contains("unavailable"));
    assert_eq!(
        f.before(&unavailable, &edit).await,
        AgentsInstructionDecision::Continue
    );
    let requests = f.loader.requests.lock().unwrap().len();
    assert_eq!(
        f.before(&unavailable, &edit).await,
        AgentsInstructionDecision::Continue
    );
    assert_eq!(f.loader.requests.lock().unwrap().len(), requests);
}

#[tokio::test]
async fn identical_paths_on_different_devices_have_independent_scopes() {
    let f = Fixture::new();
    f.loader.set("desktop", &f.ctx.working_dir, "DESKTOP_RULE");
    f.loader.set("phone", &f.ctx.working_dir, "PHONE_RULE");
    let c = f.prepare().await;
    c.snapshot_for_model();
    let phone = ToolCall::new(
        "phone",
        "read",
        json!({"device_id":"phone","file_path":f.child("a.txt")}),
    );
    reconsider(f.before(&c, &phone).await);
    let desktop = f.edit(&f.child("b.txt"));
    assert_eq!(
        f.before(&c, &desktop).await,
        AgentsInstructionDecision::Continue
    );
    let snapshot = c.snapshot_for_model().unwrap();
    assert!(
        snapshot.contains("DESKTOP_RULE")
            && snapshot.contains("PHONE_RULE")
            && snapshot.contains("\"device_id\":\"phone\"")
    );
    assert_eq!(
        f.before(&c, &phone).await,
        AgentsInstructionDecision::Continue
    );
    assert!(f
        .loader
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.device_id == "phone")
        .unwrap()
        .checked_directories
        .is_empty());
}

#[tokio::test]
async fn only_explicit_paths_are_discovered_and_edit_uses_the_real_patch_parser() {
    let f = Fixture::new();
    f.loader.set("desktop", &f.child("photos"), "PHOTO_RULE");
    let c = f.prepare().await;
    c.snapshot_for_model();
    let opaque = ToolCall::new(
        "shell",
        "bash",
        json!({"command":format!("cd {} && touch result",f.child("photos").display())}),
    );
    assert_eq!(
        f.before(&c, &opaque).await,
        AgentsInstructionDecision::Continue
    );
    assert!(!c.snapshot_for_model().unwrap().contains("PHOTO_RULE"));
    let shell = ToolCall::new("shell-cwd", "bash", json!({"command":"pwd","cwd":"photos"}));
    reconsider(f.before(&c, &shell).await);
    c.snapshot_for_model();
    let relative_remote = ToolCall::new(
        "invalid",
        "read",
        json!({"device_id":"phone","file_path":"photos/a.txt"}),
    );
    let before = f.loader.requests.lock().unwrap().len();
    assert_eq!(
        f.before(&c, &relative_remote).await,
        AgentsInstructionDecision::Continue
    );
    assert_eq!(f.loader.requests.lock().unwrap().len(), before);
    f.loader.set("desktop", &f.child("other"), "OTHER_RULE");
    let patch = ToolCall::new(
        "multi",
        "edit",
        json!({"patch":format!("*** Begin Patch\n*** Add File: {}\n+one\n*** Add File: {}\n+two\n*** End Patch",f.child("photos/a.txt").display(),f.child("other/b.txt").display())}),
    );
    reconsider(f.before(&c, &patch).await);
    let snapshot = c.snapshot_for_model().unwrap();
    assert!(snapshot.contains("OTHER_RULE"));
    assert!(f
        .loader
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(
            |r| r.target_path == f.child("other/b.txt").display().to_string()
                && r.target_kind == AgentsInstructionTargetKind::File
        ));
}

#[tokio::test]
async fn total_instruction_budget_includes_scope_headers_and_stays_utf8() {
    let f = Fixture::new();
    f.loader
        .set("desktop", &f.ctx.working_dir, &"界".repeat(20000));
    let c =
        TurnAgentsInstructions::prepare(f.loader.clone(), None, &f.ctx, CancellationToken::new())
            .await
            .unwrap();
    let text = c.snapshot_for_model().unwrap();
    assert!(text.len() <= AGENTS_INSTRUCTIONS_MAX_BYTES);
    assert!(text.contains("\"truncated\":true"));
    f.loader.set("desktop", &f.child("child"), "LATE_RULE");
    assert_eq!(
        f.before(&c, &f.edit(&f.child("child/a.txt"))).await,
        AgentsInstructionDecision::Continue
    );
    assert!(!c.snapshot_for_model().unwrap().contains("LATE_RULE"));
    let requests = f.loader.requests.lock().unwrap();
    assert!(requests.last().unwrap().max_bytes < 256);
}

async fn persisted_turn(store: &WorkspaceSessionStore, turn: &str) {
    store
        .append_conversation_turn(
            "chat",
            NewConversationTurn {
                turn_id: turn.into(),
                lifecycle: TurnLifecycle {
                    status: "in_progress".into(),
                    user_submitted_at: "2026-11-21T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: format!("user-{turn}"),
                    role: "user".into(),
                    created_at: "2026-11-21T00:00:00Z".into(),
                    status: "completed".into(),
                    content: json!([{"type":"text","text":"work"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn restarting_same_turn_restores_pending_and_persisted_sources_without_leaking_to_new_turn() {
    let mut f = Fixture::new();
    let store = WorkspaceSessionStore::new(f._dir.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "chat".into(),
            agent_id: "agent".into(),
            title: "Instructions".into(),
            created_at: "2026-11-21T00:00:00Z".into(),
            last_activity_at: "2026-11-21T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    persisted_turn(&store, "one").await;
    f.ctx.conversation_store_dir = Some(f._dir.path().to_owned());
    f.ctx.metadata.insert("turn_id".into(), json!("one"));
    f.loader
        .set("desktop", &f.ctx.working_dir, "FROZEN_CONTENT");
    let first = f.prepare().await;
    first.snapshot_for_model();
    drop(first);
    let captures = store
        .read_runtime_notes_for_turn("chat", "one")
        .await
        .unwrap();
    assert_eq!(captures.len(), 1);
    assert!(!captures[0].is_model_visible());
    assert!(matches!(
        captures[0].payload,
        RuntimeNotePayload::AgentsInstructions(_)
    ));
    assert_eq!(
        store.read_message_range("chat", 1, 20).await.unwrap().len(),
        1,
        "reading pending notes must not flush them"
    );
    let requests = f.loader.requests.lock().unwrap().len();
    f.loader
        .set("desktop", &f.ctx.working_dir, "UPDATED_CONTENT");
    let restored = f.prepare().await;
    assert!(restored
        .snapshot_for_model()
        .unwrap()
        .contains("FROZEN_CONTENT"));
    assert_eq!(f.loader.requests.lock().unwrap().len(), requests);
    store
        .flush_pending_runtime_notes("chat", "one")
        .await
        .unwrap();
    assert_eq!(
        store
            .read_runtime_notes_for_turn("chat", "one")
            .await
            .unwrap()
            .len(),
        1
    );
    let restored = f.prepare().await;
    assert!(restored
        .snapshot_for_model()
        .unwrap()
        .contains("FROZEN_CONTENT"));
    assert_eq!(f.loader.requests.lock().unwrap().len(), requests);
    persisted_turn(&store, "two").await;
    f.ctx.metadata.insert("turn_id".into(), json!("two"));
    let next = f.prepare().await;
    let snapshot = next.snapshot_for_model().unwrap();
    assert!(snapshot.contains("UPDATED_CONTENT") && !snapshot.contains("FROZEN_CONTENT"));
    assert_eq!(
        store
            .read_runtime_notes_for_turn("chat", "two")
            .await
            .unwrap()
            .len(),
        1
    );
    let messages = store.read_message_range("chat", 1, 30).await.unwrap();
    let note = messages
        .iter()
        .find(|message| message.metadata.get("model_visible") == Some(&json!(false)))
        .unwrap();
    assert_eq!(note.content, json!([{"type":"text","text":""}]));
    let projection = store
        .build_context_projection(
            "chat",
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .all(|message| message.message_id != note.message_id));
    assert!(!serde_json::to_string(&projection.tail_messages)
        .unwrap()
        .contains("FROZEN_CONTENT"));

    persisted_turn(&store, "three").await;
    f.ctx.metadata.insert("turn_id".into(), json!("three"));
    f.loader.failed.lock().unwrap().insert("desktop".into());
    let unavailable = f.prepare().await.snapshot_for_model().unwrap();
    assert!(!unavailable.contains("UPDATED_CONTENT") && !unavailable.contains("FROZEN_CONTENT"));
    assert!(unavailable.contains("unavailable"));
}

#[tokio::test]
async fn cancellation_does_not_start_discovery() {
    let f = Fixture::new();
    let c = f.prepare().await;
    let before = f.loader.requests.lock().unwrap().len();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        c.before_tool(&f.edit(&f.child("child/a.txt")), &f.ctx, cancel)
            .await,
        Err(RuaError::Cancelled)
    ));
    assert_eq!(f.loader.requests.lock().unwrap().len(), before);
}
struct DeviceSpellingLoader {
    scope: String,
    requests: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentsInstructionLoader for DeviceSpellingLoader {
    async fn discover(
        &self,
        request: &AgentsInstructionDiscoveryRequest,
        _: &ToolContext,
        _: CancellationToken,
    ) -> Result<AgentsInstructionDiscovery> {
        if request.device_id != "windows" {
            return Ok(AgentsInstructionDiscovery::default());
        }
        self.requests
            .lock()
            .unwrap()
            .push(request.target_path.clone());
        let unrelated = request.target_path.contains("-other");
        Ok(AgentsInstructionDiscovery {
            target_directory: Some(format!(
                "{}{}\\2026",
                self.scope,
                if unrelated { "-other" } else { "" }
            )),
            checked_directories: vec![self.scope.clone()],
            sources: if unrelated {
                Vec::new()
            } else {
                vec![AgentsInstructionSource {
                    device_id: "windows".into(),
                    path: format!("{}\\AGENTS.md", self.scope),
                    scope_directory: self.scope.clone(),
                    content: "WINDOWS_PHOTO_RULE".into(),
                    source_bytes: 18,
                    ..Default::default()
                }]
            },
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn target_device_drive_and_unc_paths_do_not_use_source_os_path_rules() {
    for scope in [r"C:\photos", r"\\machine\photos", "C:/photos"] {
        let f = Fixture::new();
        let loader = Arc::new(DeviceSpellingLoader {
            scope: scope.into(),
            requests: Mutex::new(Vec::new()),
        });
        let c =
            TurnAgentsInstructions::prepare(loader.clone(), None, &f.ctx, CancellationToken::new())
                .await
                .unwrap();
        c.snapshot_for_model();
        let target = format!(r"{scope}\2026\image.jpg");
        let call = ToolCall::new(
            "windows-read",
            "read",
            json!({"device_id":"windows","file_path":target}),
        );
        reconsider(f.before(&c, &call).await);
        let unrelated = ToolCall::new(
            "windows-other",
            "read",
            json!({"device_id":"windows","file_path":format!(r"{scope}-other\2026\image.jpg")}),
        );
        assert_eq!(
            f.before(&c, &unrelated).await,
            AgentsInstructionDecision::Continue
        );
        assert!(c
            .snapshot_for_model()
            .unwrap()
            .contains("WINDOWS_PHOTO_RULE"));
        assert_eq!(
            f.before(&c, &call).await,
            AgentsInstructionDecision::Continue
        );
        assert_eq!(loader.requests.lock().unwrap()[0], target);
    }
}
