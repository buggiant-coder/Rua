//! Per-Turn directory instruction capture and the model-before-action boundary.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_types::ToolCall;
use async_trait::async_trait;
use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscovery, AgentsInstructionDiscoveryRequest, AgentsInstructionLoader,
    AgentsInstructionSource, AgentsInstructionTargetKind, AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_core::agents_instructions::{
    AgentsInstructionDecision, AgentsInstructionTarget, AgentsInstructions,
};
use rua_core::tool::ToolContext;
use rua_core::{Result, RuaError};
use rua_session::{
    AgentsInstructionsRuntimeNote, RuntimeNote, RuntimeNotePayload, WorkspaceSessionStore,
};
use tokio_util::sync::CancellationToken;

const SCOPE_GUIDANCE: &str = "Directory AGENTS.md instructions captured for this Turn. Each source applies only to its stated device and directory subtree. More specific directory instructions take precedence within that subtree; do not apply them to unrelated locations.\n";
type TargetKey = (String, String, u8);

#[derive(Default)]
struct DiscoveryState {
    checked: HashMap<String, HashSet<String>>,
    targets: HashMap<TargetKey, Option<String>>,
}

struct CapturedSource {
    source: AgentsInstructionSource,
    revision: u64,
}

#[derive(Default)]
struct ModelState {
    sources: Vec<CapturedSource>,
    diagnostics: Vec<String>,
    revision: u64,
    visible_revision: u64,
    source_bytes: usize,
}

struct Persistence {
    store: WorkspaceSessionStore,
    chat_id: String,
    turn_id: String,
}

pub struct TurnAgentsInstructions {
    loader: Arc<dyn AgentsInstructionLoader>,
    base_context: Option<String>,
    discovery: tokio::sync::Mutex<DiscoveryState>,
    model: Mutex<ModelState>,
    persistence: Option<Persistence>,
}

impl TurnAgentsInstructions {
    /// A fresh coordinator is created for each run, restoring only this exact Turn's captures.
    pub async fn prepare(
        loader: Arc<dyn AgentsInstructionLoader>,
        base_context: Option<String>,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<Self> {
        if cancellation.is_cancelled() {
            return Err(RuaError::Cancelled);
        }
        let persistence = ctx.conversation_store_dir.as_ref().and_then(|root| {
            Some(Persistence {
                store: WorkspaceSessionStore::new(root),
                chat_id: ctx.session_id.clone(),
                turn_id: ctx.metadata.get("turn_id")?.as_str()?.to_owned(),
            })
        });
        let coordinator = Self {
            loader,
            base_context,
            discovery: tokio::sync::Mutex::new(DiscoveryState::default()),
            model: Mutex::new(ModelState::default()),
            persistence,
        };
        if let Some(persistence) = &coordinator.persistence {
            let notes = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(RuaError::Cancelled),
                notes = persistence.store.read_runtime_notes_for_turn(&persistence.chat_id, &persistence.turn_id) => notes,
            };
            match notes {
                Ok(notes) => {
                    let mut state = coordinator.discovery.lock().await;
                    for note in notes {
                        if let RuntimeNotePayload::AgentsInstructions(mut capture) = note.payload {
                            if capture.turn_id == persistence.turn_id {
                                coordinator.capture(
                                    &mut state,
                                    &capture.request,
                                    &mut capture.discovery,
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "could not restore this Turn's AGENTS.md captures")
                }
            }
        }
        if let Some(device) = &ctx.local_device_id {
            if let Some(path) = ctx.working_dir.to_str() {
                coordinator
                    .discover(
                        device,
                        path,
                        AgentsInstructionTargetKind::Directory,
                        ctx,
                        cancellation,
                    )
                    .await?;
            }
        }
        Ok(coordinator)
    }

    fn capture(
        &self,
        state: &mut DiscoveryState,
        request: &AgentsInstructionDiscoveryRequest,
        discovery: &mut AgentsInstructionDiscovery,
    ) {
        state
            .targets
            .insert(target_key(request), discovery.target_directory.clone());
        state
            .checked
            .entry(request.device_id.clone())
            .or_default()
            .extend(discovery.checked_directories.iter().cloned());
        let mut model = self.model.lock().expect("AGENTS snapshot lock poisoned");
        let mut adopted = Vec::new();
        for mut source in std::mem::take(&mut discovery.sources) {
            if source.device_id != request.device_id || source.content.trim().is_empty() {
                continue;
            }
            // A source's first captured content is fixed, including when reached through another scope.
            if let Some(prior) = model.sources.iter().find(|prior| {
                prior.source.device_id == source.device_id && prior.source.path == source.path
            }) {
                source.content.clone_from(&prior.source.content);
                source.source_bytes = prior.source.source_bytes;
                source.truncated = prior.source.truncated;
            }
            if model.sources.iter().any(|prior| {
                prior.source.device_id == source.device_id
                    && prior.source.scope_directory == source.scope_directory
            }) {
                continue;
            }
            let header = source_header(&source);
            let available = AGENTS_INSTRUCTIONS_MAX_BYTES
                .saturating_sub(SCOPE_GUIDANCE.len() + model.source_bytes + header.len() + 1);
            if source.content.len() > available {
                truncate_utf8(&mut source.content, available);
                source.truncated = true;
                discovery.truncated = true;
            }
            if source.content.trim().is_empty() {
                continue;
            }
            model.source_bytes += header.len() + source.content.len() + 1;
            model.revision += 1;
            let revision = model.revision;
            model.sources.push(CapturedSource {
                source: source.clone(),
                revision,
            });
            adopted.push(source);
        }
        discovery.sources = adopted;
        for diagnostic in &discovery.diagnostics {
            if model.diagnostics.len() >= 8 {
                break;
            }
            let mut diagnostic = diagnostic.clone();
            truncate_utf8(&mut diagnostic, 256);
            if !model.diagnostics.contains(&diagnostic) {
                model.diagnostics.push(diagnostic);
            }
        }
        if discovery.truncated
            && !model
                .diagnostics
                .iter()
                .any(|line| line == "AGENTS.md instruction capture was truncated.")
        {
            model
                .diagnostics
                .push("AGENTS.md instruction capture was truncated.".into());
        }
    }

    async fn discover(
        &self,
        device: &str,
        path: &str,
        kind: AgentsInstructionTargetKind,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<Option<String>> {
        let mut state = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(RuaError::Cancelled),
            state = self.discovery.lock() => state,
        };
        let mut request = AgentsInstructionDiscoveryRequest {
            device_id: device.into(),
            target_path: path.into(),
            target_kind: kind,
            ..Default::default()
        };
        if let Some(directory) = state.targets.get(&target_key(&request)) {
            return Ok(directory.clone());
        }
        request.checked_directories = state
            .checked
            .get(device)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        request.checked_directories.sort();
        request.max_bytes = {
            let model = self.model.lock().expect("AGENTS snapshot lock poisoned");
            AGENTS_INSTRUCTIONS_MAX_BYTES.saturating_sub(SCOPE_GUIDANCE.len() + model.source_bytes)
        };
        let mut discovery = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(RuaError::Cancelled),
            discovery = self.loader.discover(&request, ctx, cancellation.clone()) => match discovery {
                Ok(discovery) => discovery,
                Err(RuaError::Cancelled) => return Err(RuaError::Cancelled),
                Err(error) => AgentsInstructionDiscovery {
                    diagnostics: vec![format!("AGENTS.md discovery unavailable for {device}:{path}: {error}")],
                    ..Default::default()
                },
            },
        };
        self.capture(&mut state, &request, &mut discovery);
        if let Some(persistence) = &self.persistence {
            let note = RuntimeNote {
                runtime_note_id: format!(
                    "agents-instructions:{}:{}",
                    persistence.turn_id,
                    uuid::Uuid::new_v4()
                ),
                created_at: chrono::Utc::now().to_rfc3339(),
                payload: RuntimeNotePayload::AgentsInstructions(AgentsInstructionsRuntimeNote {
                    turn_id: persistence.turn_id.clone(),
                    request,
                    discovery: discovery.clone(),
                    extra: Default::default(),
                }),
            };
            if let Err(error) = persistence
                .store
                .append_runtime_note(&persistence.chat_id, &persistence.turn_id, note)
                .await
            {
                tracing::warn!(%error, "could not persist this Turn's AGENTS.md capture");
            }
        }
        Ok(discovery.target_directory)
    }
}

#[async_trait]
impl AgentsInstructions for TurnAgentsInstructions {
    fn snapshot_for_model(&self) -> Option<String> {
        let mut model = self.model.lock().expect("AGENTS snapshot lock poisoned");
        let mut instructions = String::new();
        if !model.sources.is_empty() {
            instructions.push_str(SCOPE_GUIDANCE);
            let mut sources = model.sources.iter().collect::<Vec<_>>();
            sources.sort_by_key(|captured| {
                (&captured.source.device_id, &captured.source.scope_directory)
            });
            for captured in sources {
                instructions.push_str(&source_header(&captured.source));
                instructions.push_str(&captured.source.content);
                instructions.push('\n');
            }
        }
        for diagnostic in &model.diagnostics {
            let line = format!("\nAGENTS.md diagnostic: {diagnostic}\n");
            if instructions.len() + line.len() <= AGENTS_INSTRUCTIONS_MAX_BYTES {
                instructions.push_str(&line);
            }
        }
        model.visible_revision = model.revision;
        match (&self.base_context, instructions.is_empty()) {
            (Some(base), true) => Some(base.clone()),
            (Some(base), false) => Some(format!("{base}\n\n{instructions}")),
            (None, false) => Some(instructions),
            (None, true) => None,
        }
    }

    async fn before_tool(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<AgentsInstructionDecision> {
        self.before_targets(&explicit_targets(call, ctx), ctx, cancellation)
            .await
    }

    async fn before_targets(
        &self,
        targets: &[AgentsInstructionTarget],
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<AgentsInstructionDecision> {
        if cancellation.is_cancelled() {
            return Err(RuaError::Cancelled);
        }
        let mut scopes = Vec::new();
        for target in targets {
            if target.device_id.trim().is_empty()
                || !rua_core::execution_directory::is_device_absolute_path(&target.path)
            {
                continue;
            }
            if let Some(directory) = self
                .discover(
                    &target.device_id,
                    &target.path,
                    target.kind,
                    ctx,
                    cancellation.clone(),
                )
                .await?
            {
                scopes.push((target.device_id.clone(), directory));
            }
        }
        if cancellation.is_cancelled() {
            return Err(RuaError::Cancelled);
        }
        let model = self.model.lock().expect("AGENTS snapshot lock poisoned");
        if model.sources.iter().any(|captured| {
            captured.revision > model.visible_revision
                && scopes.iter().any(|(device, directory)| {
                    device == &captured.source.device_id
                        && directory_in_scope(directory, &captured.source.scope_directory)
                })
        }) {
            Ok(AgentsInstructionDecision::Reconsider { reason:
                "New applicable AGENTS.md instructions were captured. Review them in the next model context, then modify, abandon, or reissue this tool call; it has not executed.".into() })
        } else {
            Ok(AgentsInstructionDecision::Continue)
        }
    }
}

fn source_header(source: &AgentsInstructionSource) -> String {
    format!(
        "\nAGENTS.md source {}\n",
        serde_json::json!({
            "device_id":source.device_id, "path":source.path, "scope_directory":source.scope_directory,
            "truncated":source.truncated,
        })
    )
}

fn target_key(request: &AgentsInstructionDiscoveryRequest) -> TargetKey {
    (
        request.device_id.clone(),
        request.target_path.clone(),
        request.target_kind as u8,
    )
}

/// Both paths were canonicalized on their owning Device; only compare directory boundaries here.
fn directory_in_scope(directory: &str, scope: &str) -> bool {
    let contains = |directory: &str, scope: &str| {
        directory
            .strip_prefix(scope.trim_end_matches('/'))
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
    };
    if scope.starts_with('/') {
        contains(directory, scope)
    } else if rua_core::execution_directory::is_device_absolute_path(scope) {
        contains(&directory.replace('\\', "/"), &scope.replace('\\', "/"))
    } else {
        false
    }
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn explicit_targets(call: &ToolCall, ctx: &ToolContext) -> Vec<AgentsInstructionTarget> {
    let args = &call.arguments;
    if call.name == "sync_send" {
        return transfer_targets(args);
    }
    let local_device = ctx.local_device_id.as_deref();
    let device = if call.name == "bash" {
        local_device
    } else {
        args.get("device_id")
            .and_then(serde_json::Value::as_str)
            .or(local_device)
    };
    let Some(device) = device.filter(|device| !device.is_empty()) else {
        return Vec::new();
    };
    let paths = match call.name.as_str() {
        "read" => args
            .get("file_path")
            .and_then(serde_json::Value::as_str)
            .map(|path| vec![(path.to_owned(), AgentsInstructionTargetKind::Auto)])
            .unwrap_or_default(),
        "edit" => args
            .get("patch")
            .and_then(serde_json::Value::as_str)
            .and_then(|patch| rua_tool::EditFileTool::affected_paths(patch).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|path| (path, AgentsInstructionTargetKind::File))
            .collect(),
        "grep" | "glob" => vec![(
            args.get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(".")
                .to_owned(),
            AgentsInstructionTargetKind::Auto,
        )],
        "bash" => vec![(
            args.get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(".")
                .to_owned(),
            AgentsInstructionTargetKind::Directory,
        )],
        _ => return Vec::new(),
    };
    let fs = if Some(device) == local_device {
        rua_tool::WorkspaceFs::for_execution(
            &ctx.working_dir,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
        .ok()
    } else {
        None
    };
    paths
        .into_iter()
        .filter_map(|(path, kind)| {
            if rua_core::chat_media::reference_id(&path).is_some() {
                return None;
            }
            let path = if Some(device) == local_device {
                fs.as_ref()?.public_path(Path::new(&path))
            } else if rua_core::execution_directory::is_device_absolute_path(&path) {
                PathBuf::from(path)
            } else {
                return None;
            };
            Some(AgentsInstructionTarget {
                device_id: device.to_owned(),
                path: path.to_str()?.to_owned(),
                kind,
            })
        })
        .collect()
}

fn transfer_targets(args: &serde_json::Value) -> Vec<AgentsInstructionTarget> {
    let Some(source_device) = args["source_device_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
    else {
        return Vec::new();
    };
    let Some(target_device) = args["target_device_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
    else {
        return Vec::new();
    };
    let Some(target_dir) = args["target_dir"]
        .as_str()
        .filter(|path| rua_core::execution_directory::is_device_absolute_path(path))
    else {
        return Vec::new();
    };
    let Some(paths) = args["source_paths"].as_array() else {
        return Vec::new();
    };
    let Some(paths) = paths
        .iter()
        .map(|path| {
            path.as_str()
                .filter(|path| rua_core::execution_directory::is_device_absolute_path(path))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };
    if paths.is_empty() {
        return Vec::new();
    }
    paths
        .into_iter()
        .map(|path| AgentsInstructionTarget {
            device_id: source_device.into(),
            path: path.into(),
            kind: AgentsInstructionTargetKind::Auto,
        })
        .chain(std::iter::once(AgentsInstructionTarget {
            device_id: target_device.into(),
            path: target_dir.into(),
            kind: AgentsInstructionTargetKind::Directory,
        }))
        .collect()
}
