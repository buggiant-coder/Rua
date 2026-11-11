use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use iroh_docs::store::Query;
use serde::Serialize;
use serde_json::{json, Value};
use tempfile::TempDir;

use rua_server::services::chat_index::{self, ChatIndexEntry};
use rua_server::services::sync_ops;
use rua_server::sync_publication::{publish_sync_fact, PublicationStatus, SyncPublicationFact};
use rua_server::AppState;
use rua_sync::device_list::{DeviceEntry, DeviceList};

async fn build_sync_enabled_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    seed_sync_identity(&tmp);
    seed_portable_state(&tmp).await;

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for v4 full export regression test"
    );
    seed_encrypted_secret_state(&state).await;
    (state, tmp)
}

fn seed_sync_identity(tmp: &TempDir) {
    let network = rua_sync::identity::NetworkSecret::generate();
    seed_sync_identity_with_network(tmp, &network);
}

fn seed_sync_identity_with_network(tmp: &TempDir, network: &rua_sync::identity::NetworkSecret) {
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::from_bytes(&network.to_bytes())
        .save(&sync_dir)
        .expect("network secret");
}

async fn seed_portable_state(tmp: &TempDir) {
    let root = tmp.path();
    let ws = root.join("workspaces/default");
    std::fs::create_dir_all(ws.join("sessions/default-evergreen/messages"))
        .expect("seed evergreen session dir");
    std::fs::create_dir_all(ws.join("sessions")).expect("seed flat sessions dir");
    std::fs::create_dir_all(ws.join("media")).expect("seed media dir");
    std::fs::create_dir_all(ws.join("skills/full_export_skill")).expect("seed workspace skill dir");
    std::fs::create_dir_all(ws.join("prompt-state")).expect("seed prompt-state dir");
    std::fs::create_dir_all(root.join("prompt_library")).expect("seed prompt library");
    std::fs::create_dir_all(root.join("sync")).expect("seed sync dir");
    std::fs::create_dir_all(root.join("secret/providers/providers")).expect("seed providers dir");

    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "agents": {
                "profiles": {
                    "default": {
                        "id": "default",
                        "workspace_dir": ws.to_string_lossy(),
                        "enabled": true
                    }
                },
                "agent_order": ["default"]
            },
            "security": {
                "file_guard": {
                    "enabled": false,
                    "paths": ["**/.env.full-export"]
                },
                "skill_scanner": {
                    "mode": "warn"
                },
                "allow_no_auth_hosts": ["203.0.113.10"]
            },
            "user_timezone": "Asia/Shanghai"
        }))
        .expect("config json"),
    )
    .expect("write config");
    std::fs::write(
        root.join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "sync_full_export_marker": true
        }))
        .expect("settings json"),
    )
    .expect("write settings");
    std::fs::write(
        root.join("agent_templates.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "templates": {
                "full_export_template": {
                    "name": "Full Export Template",
                    "description": "agent template marker"
                }
            }
        }))
        .expect("agent templates json"),
    )
    .expect("write agent templates");
    std::fs::write(
        root.join("subagent_templates.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "templates": [{
                "id": "full-export-subagent",
                "name": "Full Export Subagent"
            }]
        }))
        .expect("subagent templates json"),
    )
    .expect("write subagent templates");
    std::fs::create_dir_all(root.join("secret/providers")).expect("seed provider root dir");
    std::fs::write(
        root.join("secret/providers/default_model.json"),
        serde_json::to_vec_pretty(&json!({
            "provider_id": "full-export-provider",
            "model": "full-export-model",
            "source": "full-export-test"
        }))
        .expect("active model json"),
    )
    .expect("write active model");
    std::fs::write(
        ws.join("agent.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "default",
            "name": "Full Export Agent",
            "description": "full export v4 marker",
            "workspace_dir": ws.to_string_lossy(),
            "language": "",
            "system_prompt_files": []
        }))
        .expect("agent json"),
    )
    .expect("write agent");
    std::fs::write(
        ws.join("security.json"),
        serde_json::to_vec_pretty(&json!({
            "tool_guard": {
                "enabled": true,
                "approval_mode": "balanced"
            },
            "permission_rules": [{
                "id": "deny-bash",
                "agent_id": "default",
                "scope": "Agent",
                "label": "Block Bash",
                "matcher": {"type": "tool_name", "pattern": "bash"},
                "source_approval_id": "",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "enabled": true,
                "effect": "deny",
                "source": "manual",
                "hit_count": 0
            }]
        }))
        .expect("security json"),
    )
    .expect("write agent security");
    let chat_updated_at = chrono::DateTime::parse_from_rfc3339("2026-06-22T00:00:00Z")
        .expect("chat updated timestamp")
        .with_timezone(&chrono::Utc);
    chat_index::upsert(
        &ws,
        ChatIndexEntry {
            id: "full-export-chat".to_string(),
            name: "Full Export Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: chat_updated_at,
            updated_at: chat_updated_at,
            meta: json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("write chat index");
    std::fs::write(
        ws.join("sessions/default-flat-session.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "flat-session",
            "messages": [{"id": "m1", "role": "user", "content": "flat session marker"}]
        }))
        .expect("flat session json"),
    )
    .expect("write flat session");
    std::fs::write(
        ws.join("sessions/default-evergreen/session.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "default-evergreen",
            "name": "Evergreen Full Export"
        }))
        .expect("evergreen session json"),
    )
    .expect("write evergreen session");
    std::fs::write(
        ws.join("sessions/default-evergreen/messages/000000000000.jsonl"),
        "{\"id\":\"em1\",\"content\":\"evergreen message marker\"}\n",
    )
    .expect("write evergreen message log");
    std::fs::write(
        ws.join("prompt-state/default-evergreen.json"),
        serde_json::to_vec_pretty(&json!({
            "text": "full export prompt-state marker",
            "files": []
        }))
        .expect("prompt-state json"),
    )
    .expect("write prompt-state");
    std::fs::create_dir_all(ws.join("prompts")).expect("prompt dir");
    std::fs::write(
        ws.join("prompts/AGENT.md"),
        "# Full export agent prompt marker\n",
    )
    .expect("write agent prompt file");
    std::fs::write(
        ws.join("media/00000000-0000-4000-8000-000000000501"),
        "full export media marker\n",
    )
    .expect("write media");
    std::fs::write(
        ws.join("skills/full_export_skill/SKILL.md"),
        "# Full Export Skill\n\nUse this skill as a v4 full export marker.\n",
    )
    .expect("write workspace skill");
    std::fs::write(
        root.join("prompt_library/full-export.md"),
        "# Full export prompt marker\n",
    )
    .expect("write prompt");
    std::fs::write(
        root.join("sync/device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: vec![DeviceEntry {
                device_id: [9; 8],
                public_key: [9; 32],
                label: "full-export-phone".to_string(),
                friendly_name: "full-export-phone".to_string(),
                added_at_ms: 1_000,
                last_seen_ms: 2_000,
                direct_addrs: Vec::new(),
            }],
            ..Default::default()
        })
        .expect("device list json"),
    )
    .expect("write device list");
}

async fn seed_encrypted_secret_state(state: &AppState) {
    let key = *state.device_key.as_bytes();
    let agent_path = state.rua_root.join("workspaces/default/agent.json");
    let mut agent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&agent_path).expect("read agent"))
            .expect("agent json");
    let agent_mcp = json!({
        "clients": {
            "full-export-agent-mcp": {
                "name": "Full Export Agent MCP",
                "enabled": true,
                "transport": "streamable_http",
                "url": "https://mcp.example.test/mcp",
                "headers": {
                    "Authorization": "Bearer agent-mcp-auth-marker"
                },
                "oauth": {
                    "client_id": "agent-mcp-client",
                    "scope": "tools:read",
                    "access_token": "agent-mcp-access-marker",
                    "refresh_token": "agent-mcp-refresh-marker",
                    "expires_at": 1782080000.0,
                    "token_endpoint": "https://auth.example.test/token",
                    "auth_endpoint": "https://auth.example.test/authorize"
                }
            }
        }
    });
    let encrypted_agent_mcp =
        rua_mcp::secret_field_io::encrypt_client_value(&agent_mcp, Some(&key), "agent:default:mcp")
            .expect("encrypt agent mcp");
    agent["mcp"] = encrypted_agent_mcp;
    std::fs::write(
        &agent_path,
        serde_json::to_vec_pretty(&agent).expect("agent json"),
    )
    .expect("write encrypted agent mcp");

    let provider: rua_provider::ProviderConfig = serde_json::from_value(json!({
        "id": "full-export-provider",
        "name": "Full Export Provider",
        "base_url": "https://provider.example.test/v1",
        "api_key": "sk-full-export-provider",
        "models": [{
            "id": "full-export-model",
            "name": "Full Export Model"
        }]
    }))
    .expect("provider config");
    rua_provider::ProviderConfigLoader::with_key(state.secret_dir.clone(), key)
        .save_provider(&provider)
        .await
        .expect("save encrypted provider");

    let shared_mcp = json!({
        "clients": {
            "full-export-shared-mcp": {
                "name": "Full Export Shared MCP",
                "enabled": true,
                "transport": "stdio",
                "command": "/bin/echo",
                "args": ["ok"],
                "env": {
                    "TOKEN": "shared-mcp-secret-marker"
                },
                "headers": {
                    "Authorization": "Bearer shared-mcp-auth-marker"
                }
            }
        }
    });
    let encrypted_shared_mcp =
        rua_mcp::secret_field_io::encrypt_client_value(&shared_mcp, Some(&key), "shared:mcp")
            .expect("encrypt shared mcp");
    std::fs::write(
        state.rua_root.join("shared_mcp.json"),
        serde_json::to_vec_pretty(&encrypted_shared_mcp).expect("shared mcp json"),
    )
    .expect("write encrypted shared mcp");

    let provider_raw = std::fs::read_to_string(
        state
            .secret_dir
            .join("providers/providers/full-export-provider.json"),
    )
    .expect("read encrypted provider");
    assert!(
        !provider_raw.contains("sk-full-export-provider"),
        "provider api_key must be encrypted at rest before full export"
    );
    let agent_raw = std::fs::read_to_string(&agent_path).expect("read encrypted agent");
    assert!(
        !agent_raw.contains("Bearer agent-mcp-auth-marker")
            && !agent_raw.contains("agent-mcp-access-marker")
            && !agent_raw.contains("agent-mcp-refresh-marker"),
        "agent MCP secrets must be encrypted at rest before full export"
    );
    let shared_raw =
        std::fs::read_to_string(state.rua_root.join("shared_mcp.json")).expect("read shared mcp");
    assert!(
        !shared_raw.contains("shared-mcp-secret-marker")
            && !shared_raw.contains("Bearer shared-mcp-auth-marker"),
        "shared MCP secrets must be encrypted at rest before full export"
    );
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let (sync_doc, blobs_store) = {
        let guard = state.sync_node.lock().await;
        let node = guard.as_ref().expect("sync node");
        (node.doc().clone(), node.blobs_store().clone())
    };
    let network_secret = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .clone();
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("device")
        .clone();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        sync_doc.clone(),
        device.to_iroh_author(),
        network_secret.derive_sync_key(),
        blobs_store,
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let frame_count = entries.len();
    let mut changes = Vec::new();
    for entry in entries {
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            let frame_changes = frame.into_changes();
            assert_eq!(
                frame_changes.len(),
                1,
                "each full export entry must contain exactly one logical record"
            );
            let change = &frame_changes[0];
            assert_eq!(
                entry.key(),
                log.record_entry_key(&change.domain, &change.record_id),
                "full export entry must use the logical record's stable key"
            );
            changes.extend(frame_changes);
        }
    }
    assert_eq!(
        frame_count,
        changes.len(),
        "full export should expose one current replica-log entry per logical record"
    );
    changes
}

async fn collect_all_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let bound = state
        .bound_sync_network()
        .await
        .expect("bound sync network");
    let log = bound.opaque_replica_log();
    let stream = bound
        .doc()
        .get_many(Query::all().build())
        .await
        .expect("query replica doc");
    tokio::pin!(stream);
    let mut changes = Vec::new();
    while let Some(entry) = stream.next().await {
        let entry = entry.expect("replica entry");
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read replica frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

fn find_change<'a>(
    changes: &'a [rua_sync::opaque_replica_log::SyncChange],
    domain: &str,
    record_id: &str,
) -> &'a rua_sync::opaque_replica_log::SyncChange {
    changes
        .iter()
        .find(|change| change.domain == domain && change.record_id == record_id)
        .unwrap_or_else(|| {
            let summary = changes
                .iter()
                .map(|change| format!("{}/{}/{}", change.domain, change.record_id, change.kind))
                .collect::<Vec<_>>();
            panic!("missing {domain}/{record_id}; available={summary:#?}")
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_export_v4_appends_opaque_changes_for_portable_domains() {
    let (state, _tmp) = build_sync_enabled_state().await;

    sync_ops::full_export_v4(&state).await;

    let changes = collect_v4_changes(&state).await;
    let domains = changes
        .iter()
        .map(|change| change.domain.as_str())
        .collect::<BTreeSet<_>>();
    for expected in [
        "agent_config",
        "agent_security",
        "agent_template",
        "chat",
        "device_list",
        "evergreen_chat",
        "media",
        "prompts",
        "provider_secret",
        "root_config",
        "shared_mcp",
        "skill_blob_manifest",
        "agent_prompt_file",
    ] {
        assert!(
            domains.contains(expected),
            "full_export_v4 must append {expected} changes; domains={domains:?}"
        );
    }

    let agent = find_change(&changes, "agent_config", "default");
    assert_eq!(agent.kind, "snapshot");
    assert_eq!(agent.payload["name"], "Full Export Agent");
    assert_eq!(
        agent.payload["mcp"]["clients"]["full-export-agent-mcp"]["headers"]["Authorization"],
        "Bearer agent-mcp-auth-marker"
    );
    assert_eq!(
        agent.payload["mcp"]["clients"]["full-export-agent-mcp"]["oauth"]["access_token"],
        "agent-mcp-access-marker"
    );
    assert_eq!(
        agent.payload["mcp"]["clients"]["full-export-agent-mcp"]["oauth"]["refresh_token"],
        "agent-mcp-refresh-marker"
    );

    let agent_security = find_change(&changes, "agent_security", "default");
    assert_eq!(agent_security.kind, "snapshot");
    assert_eq!(
        agent_security.payload["tool_guard"]["approval_mode"],
        "balanced"
    );
    assert_eq!(
        agent_security.payload["permission_rules"][0]["matcher"]["pattern"],
        "bash"
    );

    let agent_template = find_change(&changes, "agent_template", "full_export_template");
    assert_eq!(agent_template.kind, "snapshot");
    assert_eq!(agent_template.payload["name"], "Full Export Template");

    let chat_index = find_change(&changes, "chat", "agent/default/chats/full-export-chat");
    assert_eq!(chat_index.kind, "index_upsert");
    assert_eq!(chat_index.payload["name"], "Full Export Chat");

    assert!(
        !changes.iter().any(|change| {
            change.domain == "chat"
                && (change.kind == "session_upsert"
                    || change.record_id.contains("default-flat-session"))
        }),
        "legacy flat Session files must not re-enter Chat Sync after the chat_id-only cutover"
    );

    let evergreen_prompt_state = find_change(
        &changes,
        "evergreen_chat",
        "agent/default/default-evergreen/prompt_state",
    );
    assert_eq!(evergreen_prompt_state.kind, "file_snapshot");
    assert_eq!(
        evergreen_prompt_state.payload["text"],
        "full export prompt-state marker"
    );

    let prompt = find_change(&changes, "prompts", "full-export.md");
    assert_eq!(prompt.kind, "snapshot");
    assert_eq!(
        prompt.payload["content"],
        Value::String("# Full export prompt marker\n".to_string())
    );

    let provider = find_change(&changes, "provider_secret", "full-export-provider");
    assert_eq!(provider.kind, "snapshot");
    assert_eq!(provider.payload["api_key"], "sk-full-export-provider");
    assert_eq!(provider.payload["name"], "Full Export Provider");

    let shared_mcp = find_change(&changes, "shared_mcp", "catalog");
    assert_eq!(shared_mcp.kind, "snapshot");
    assert_eq!(
        shared_mcp.payload["clients"]["full-export-shared-mcp"]["env"]["TOKEN"],
        "shared-mcp-secret-marker"
    );
    assert_eq!(
        shared_mcp.payload["clients"]["full-export-shared-mcp"]["headers"]["Authorization"],
        "Bearer shared-mcp-auth-marker"
    );

    let media = find_change(
        &changes,
        "media",
        "agent/default/00000000-0000-4000-8000-000000000501",
    );
    assert_eq!(media.kind, "upsert");
    assert_eq!(
        media.payload["media_id"],
        "00000000-0000-4000-8000-000000000501"
    );
    assert_eq!(media.payload["size"], 25);
    assert!(
        media
            .payload
            .get("inline_ciphertext_b64")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty()),
        "small media full export must inline encrypted file bytes"
    );
    assert!(media.payload.get("hash_ref").is_none());

    let skill = find_change(
        &changes,
        "skill_blob_manifest",
        "workspace/default/full_export_skill",
    );
    assert_eq!(skill.kind, "upsert");
    assert_eq!(skill.payload["scope"], "workspace");
    assert_eq!(skill.payload["agent_id"], "default");
    assert_eq!(skill.payload["skill_name"], "full_export_skill");
    assert!(
        skill.payload["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| {
                file["rel_path"] == "SKILL.md"
                    && file["is_text"] == true
                    && file
                        .get("inline_ciphertext_b64")
                        .and_then(|value| value.as_str())
                        .is_some_and(|value| !value.is_empty())
                    && file.get("hash_ref").is_none()
            })),
        "workspace skill full export must include small SKILL.md as encrypted inline metadata"
    );

    let agent_prompt = find_change(&changes, "agent_prompt_file", "default/root/AGENT.md");
    assert_eq!(agent_prompt.kind, "snapshot");
    assert_eq!(
        agent_prompt.payload["content"],
        Value::String("# Full export agent prompt marker\n".to_string())
    );

    let device = find_change(&changes, "device_list", "0909090909090909");
    assert_eq!(device.kind, "snapshot");
    assert_eq!(device.payload["friendly_name"], "full-export-phone");

    let settings = find_change(&changes, "root_config", "settings");
    assert_eq!(settings.kind, "snapshot");
    assert_eq!(settings.payload["sync_full_export_marker"], true);

    let subagent_templates = find_change(&changes, "root_config", "subagent_templates");
    assert_eq!(subagent_templates.kind, "snapshot");
    assert_eq!(
        subagent_templates.payload["templates"][0]["id"],
        "full-export-subagent"
    );

    assert!(
        changes.iter().all(|change| {
            change.domain != "root_config" || change.record_id != "default_model"
        }),
        "full export must not include the device-local default model"
    );

    let agents_registry = find_change(&changes, "root_config", "agents");
    assert_eq!(agents_registry.kind, "snapshot");
    assert_eq!(
        agents_registry.payload["profiles"]["default"]["id"],
        "default"
    );

    let user_timezone = find_change(&changes, "root_config", "user_timezone");
    assert_eq!(user_timezone.kind, "snapshot");
    assert_eq!(user_timezone.payload, "Asia/Shanghai");

    let security = find_change(&changes, "root_config", "security");
    assert_eq!(security.kind, "snapshot");
    assert_eq!(security.payload["file_guard"]["enabled"], false);
    assert_eq!(
        security.payload["file_guard"]["paths"],
        json!(["**/.env.full-export"])
    );
    assert_eq!(security.payload["skill_scanner"]["mode"], "warn");
    assert!(
        security.payload.get("allow_no_auth_hosts").is_none(),
        "auth-bypass hosts are device-local and must not full-export"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frozen_full_export_does_not_reread_canonical_files_during_publication() {
    let (state, tmp) = build_sync_enabled_state().await;
    let prompt_path = tmp.path().join("prompt_library/full-export.md");
    let original = std::fs::read_to_string(&prompt_path).expect("read original prompt");

    let snapshot = sync_ops::capture_full_export_v4_bootstrap(&state)
        .await
        .expect("capture frozen Full Export");
    std::fs::write(&prompt_path, "# Changed after First Sync boundary\n")
        .expect("mutate canonical prompt after capture");

    assert!(sync_ops::publish_full_export_snapshot(&state, snapshot).await);
    let changes = collect_v4_changes(&state).await;
    let prompt = find_change(&changes, "prompts", "full-export.md");
    assert_eq!(prompt.payload["content"], original);
    assert_ne!(
        prompt.payload["content"], "# Changed after First Sync boundary\n",
        "publication must use the frozen pre-pair change rather than rereading canonical state"
    );
}

#[derive(Serialize)]
struct OrdinarySyncAcceptanceReport {
    iterations: usize,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
    route: rua_sync::transport::SyncRouteKind,
    direct_bytes: u64,
    relay_bytes: u64,
    selected_peer_node_id: String,
    repair_scanned: usize,
    repair_recorded: usize,
    failure_stage: Option<rua_sync::stage_diagnostics::SyncFailureStage>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_prompt_sync_acceptance_reports_canonical_visibility_latency() {
    const ITERATIONS: usize = 8;

    let network = rua_sync::identity::NetworkSecret::generate();
    let (state_a, tmp_a) = build_first_sync_state(&network, "acceptance-a").await;
    let (state_b, tmp_b) = build_first_sync_state(&network, "acceptance-b").await;
    state_a
        .ensure_device_network_runtime()
        .await
        .expect("start Device A network runtime");
    state_b
        .ensure_device_network_runtime()
        .await
        .expect("start Device B network runtime");
    let node_a = state_a
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device A SyncNode");
    let node_b = state_b
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device B SyncNode");
    let addr_a = local_endpoint_addr(&node_a);
    let addr_b = local_endpoint_addr(&node_b);
    node_a
        .register_peer(addr_b.clone())
        .await
        .expect("register Device B");
    node_b
        .register_peer(addr_a.clone())
        .await
        .expect("register Device A");
    write_two_device_list(&state_a, &state_b, &addr_b);
    write_two_device_list(&state_b, &state_a, &addr_a);

    let prompt_id = "ordinary-acceptance.md";
    let source_path = tmp_a.path().join("prompt_library").join(prompt_id);
    let target_path = tmp_b.path().join("prompt_library").join(prompt_id);
    let mut visibility_ms = Vec::with_capacity(ITERATIONS);
    let mut direct_bytes = 0u64;
    let mut relay_bytes = 0u64;
    let mut last_route = rua_sync::transport::SyncRouteKind::Unknown;
    let mut last_repair = rua_sync::inbound_inbox::RepairScanReport::default();
    let mut last_correlation_id = String::new();

    for iteration in 0..ITERATIONS {
        let content = format!("ordinary acceptance version {iteration}");
        std::fs::write(&source_path, &content).expect("commit source Prompt");
        let visibility_started = Instant::now();
        let publication = publish_sync_fact(
            &state_a,
            SyncPublicationFact::PromptLibraryFilesCommitted {
                changes: vec![rua_prompts::file_store::PromptFileChange {
                    path: prompt_id.to_string(),
                    content: Some(content.clone()),
                    version: rua_prompts::file_store::PromptFileVersion::local(
                        chrono::Utc::now().timestamp_millis() as u64,
                    ),
                    deleted_before: None,
                }],
            },
        )
        .await;
        assert_eq!(publication.status, PublicationStatus::Published);
        let source_sample = state_a
            .sync_stage_recorder
            .snapshot()
            .recent_publications
            .into_iter()
            .next()
            .expect("source publication stage");
        last_correlation_id.clone_from(&source_sample.correlation_id);

        let transport =
            rua_sync::transport::sync_now(&node_b, addr_a.clone(), Duration::from_secs(10))
                .await
                .expect("ordinary Prompt Sync Cycle");
        last_route = transport.path_observation.route;
        direct_bytes = direct_bytes
            .saturating_add(transport.path_observation.direct_tx_bytes)
            .saturating_add(transport.path_observation.direct_rx_bytes);
        relay_bytes = relay_bytes
            .saturating_add(transport.path_observation.relay_tx_bytes)
            .saturating_add(transport.path_observation.relay_rx_bytes);
        last_repair = state_b
            .repair_scan_and_drain_sync_inbox()
            .await
            .expect("apply ordinary Prompt");

        assert_eq!(
            std::fs::read_to_string(&target_path).expect("receiving Prompt visibility"),
            content
        );
        visibility_ms.push(visibility_started.elapsed().as_millis() as u64);
        let inbound = state_b
            .sync_stage_recorder
            .snapshot()
            .recent_inbound
            .into_iter()
            .find(|sample| sample.correlation_id == source_sample.correlation_id)
            .expect("correlated receiver stage");
        assert_eq!(
            inbound.source_commit_observed_at_ms,
            Some(source_sample.commit_observed_at_ms)
        );
        assert!(inbound.transport_arrival_at_ms <= inbound.inbox_recorded_at_ms);
        assert!(inbound.inbox_recorded_at_ms <= inbound.content_ready_at_ms);
        assert!(inbound.content_ready_at_ms <= inbound.decrypt_read_completed_at_ms);
        assert!(inbound.decrypt_read_completed_at_ms <= inbound.domain_apply_completed_at_ms);
        assert!(inbound.domain_apply_completed_at_ms <= inbound.cache_refresh_completed_at_ms);
        assert!(inbound.cache_refresh_completed_at_ms <= inbound.canonical_visible_at_ms);
    }

    visibility_ms.sort_unstable();
    let report = OrdinarySyncAcceptanceReport {
        iterations: ITERATIONS,
        p50_ms: nearest_rank(&visibility_ms, 50),
        p95_ms: nearest_rank(&visibility_ms, 95),
        max_ms: visibility_ms.last().copied().unwrap_or_default(),
        route: last_route,
        direct_bytes,
        relay_bytes,
        selected_peer_node_id: node_a.node_id().to_string(),
        repair_scanned: last_repair.scanned,
        repair_recorded: last_repair.recorded,
        failure_stage: state_b
            .sync_stage_recorder
            .snapshot()
            .last_failure
            .filter(|failure| {
                failure.correlation_id.as_deref() == Some(last_correlation_id.as_str())
            })
            .map(|failure| failure.stage),
    };
    println!(
        "SYNC_ORDINARY_ACCEPTANCE {}",
        serde_json::to_string(&report).expect("serialize acceptance report")
    );
    assert_eq!(report.failure_stage, None);
    assert!(
        report.max_ms < 10_000,
        "automated acceptance catches gross latency regressions; real path budgets are Issue 13"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_sync_merges_both_frozen_views_without_republishing_remote_records() {
    let network = rua_sync::identity::NetworkSecret::generate();
    let (state_a, tmp_a) = build_first_sync_state(&network, "a").await;
    let (state_b, tmp_b) = build_first_sync_state(&network, "b").await;

    state_a
        .ensure_device_network_runtime()
        .await
        .expect("start Device A network runtime");
    state_b
        .ensure_device_network_runtime()
        .await
        .expect("start Device B network runtime");
    let node_a = state_a
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device A SyncNode");
    let node_b = state_b
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device B SyncNode");
    let addr_a = local_endpoint_addr(&node_a);
    let addr_b = local_endpoint_addr(&node_b);
    node_a
        .register_peer(addr_b.clone())
        .await
        .expect("register Device B");
    node_b
        .register_peer(addr_a)
        .await
        .expect("register Device A");
    write_two_device_list(&state_a, &state_b, &addr_b);
    write_two_device_list(&state_b, &state_a, &local_endpoint_addr(&node_a));

    let frozen_a = sync_ops::capture_full_export_v4_bootstrap(&state_a)
        .await
        .expect("capture Device A state");
    let frozen_b = sync_ops::capture_full_export_v4_bootstrap(&state_b)
        .await
        .expect("capture Device B state");
    assert!(sync_ops::publish_full_export_snapshot(&state_a, frozen_a).await);
    assert!(sync_ops::publish_full_export_snapshot(&state_b, frozen_b).await);

    rua_sync::transport::sync_now(&node_a, addr_b, Duration::from_secs(30))
        .await
        .expect("First Sync Device A with Device B");
    state_a
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("apply Device B state on Device A");
    state_b
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("apply Device A state on Device B");

    for tmp in [&tmp_a, &tmp_b] {
        let prompts = tmp.path().join("prompt_library");
        assert_eq!(
            std::fs::read_to_string(prompts.join("local-a.md")).unwrap(),
            "local user state a"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("local-b.md")).unwrap(),
            "local user state b"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("modified-a.md")).unwrap(),
            "custom seed a"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("modified-b.md")).unwrap(),
            "custom seed b"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("untouched.md")).unwrap(),
            "untouched builtin"
        );
    }

    let merged = collect_all_v4_changes(&state_a).await;
    assert!(!merged
        .iter()
        .any(|change| { change.domain == "prompts" && change.record_id == "untouched.md" }));
    for record_id in ["local-a.md", "local-b.md", "modified-a.md", "modified-b.md"] {
        assert_eq!(
            merged
                .iter()
                .filter(|change| change.domain == "prompts" && change.record_id == record_id)
                .count(),
            1,
            "First Sync must not echo {record_id} under the receiving Device author"
        );
    }
    println!(
        "SYNC_FIRST_SYNC_ACCEPTANCE {}",
        serde_json::json!({
            "devices": 2,
            "frozen_views": 2,
            "untouched_seed_exported": false,
            "modified_seed_visible_on_both": true,
            "independent_state_visible_on_both": true
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_sync_retries_until_late_peer_runtime_is_addressable() {
    let network = rua_sync::identity::NetworkSecret::generate();
    let (state_a, _tmp_a) = build_first_sync_state(&network, "late-a").await;
    let (state_b, tmp_b) = build_first_sync_state(&network, "late-b").await;

    state_a
        .ensure_device_network_runtime()
        .await
        .expect("start Device A runtime");
    let node_a = state_a
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device A SyncNode");
    let remote_public_key = *state_b
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("Device B identity")
        .public_key()
        .as_bytes();
    let unresolved_b = iroh::EndpointAddr::new(
        iroh::PublicKey::from_bytes(&remote_public_key).expect("Device B public key"),
    );
    write_two_device_list(&state_a, &state_b, &unresolved_b);

    let frozen_a = sync_ops::capture_full_export_v4_bootstrap(&state_a)
        .await
        .expect("capture Device A state");
    assert!(sync_ops::publish_full_export_snapshot(&state_a, frozen_a).await);

    let late_state_b = state_b.clone();
    let late_state_a = state_a.clone();
    let late_node_a = node_a.clone();
    let late_peer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        late_state_b
            .ensure_device_network_runtime()
            .await
            .expect("start delayed Device B runtime");
        let node_b = late_state_b
            .sync_node
            .lock()
            .await
            .as_ref()
            .cloned()
            .expect("Device B SyncNode");
        let addr_a = local_endpoint_addr(&late_node_a);
        let addr_b = local_endpoint_addr(&node_b);
        write_two_device_list(&late_state_a, &late_state_b, &addr_b);
        write_two_device_list(&late_state_b, &late_state_a, &addr_a);
        node_b
            .register_peer(addr_a)
            .await
            .expect("register Device A on delayed Device B");
    });

    let result = rua_server::sync_cycle::run_first_sync_cycle_to_all_device_list_peers(
        &state_a.sync_node,
        &state_a.rua_root.join("sync"),
    )
    .await
    .expect("First Sync should survive delayed peer startup");
    assert_eq!(result.attempted, 1);
    assert_eq!(result.succeeded, 1);
    late_peer.await.expect("delayed Device B task");

    let target = tmp_b.path().join("prompt_library").join("local-late-a.md");
    for _ in 0..20 {
        let _ = state_b.repair_scan_and_drain_sync_inbox().await;
        if target.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        std::fs::read_to_string(&target).unwrap_or_else(|error| {
            panic!(
                "read delayed First Sync result: {error}; events_sent={}, events_received={}; inbox={:?}",
                result.events_sent,
                result.events_received,
                state_b.inbound_inbox.diagnostics(10)
            )
        }),
        "local user state late-a"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn network_merge_exports_loser_state_captured_before_namespace_switch() {
    let winner_network = rua_sync::identity::NetworkSecret::generate();
    let loser_network = rua_sync::identity::NetworkSecret::generate();
    let (winner, winner_tmp) = build_first_sync_state(&winner_network, "winner").await;
    let (loser, loser_tmp) = build_first_sync_state(&loser_network, "loser").await;

    winner
        .ensure_device_network_runtime()
        .await
        .expect("start winner runtime");
    loser
        .ensure_device_network_runtime()
        .await
        .expect("start loser runtime");
    let winner_node = winner
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("winner SyncNode");
    let loser_node = loser
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("loser SyncNode");

    let frozen_winner = sync_ops::capture_full_export_v4_bootstrap(&winner)
        .await
        .expect("capture winner state");
    let frozen_loser = sync_ops::capture_full_export_v4_bootstrap_for_network(
        &loser,
        loser_node.blobs_store().clone(),
        &winner_network,
    )
    .await
    .expect("capture loser state for winner network");
    let before_switch = rua_sync::identity::NetworkSecret::load(&loser.rua_root.join("sync"))
        .expect("load loser network before switch")
        .expect("loser network exists");
    assert_eq!(
        before_switch.namespace_id(),
        loser_network.namespace_id(),
        "capturing pre-merge state must not expose a partially switched namespace"
    );

    loser.clear_sync_node().await;
    rua_sync::identity::NetworkSecret::from_bytes(&winner_network.to_bytes())
        .save(&loser.rua_root.join("sync"))
        .expect("switch loser to winner namespace");
    loser
        .ensure_device_network_runtime()
        .await
        .expect("bind loser to winner namespace");
    let merged_loser_node = loser
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("merged loser SyncNode");
    let winner_addr = local_endpoint_addr(&winner_node);
    let loser_addr = local_endpoint_addr(&merged_loser_node);
    winner_node
        .register_peer(loser_addr.clone())
        .await
        .expect("register merged loser");
    merged_loser_node
        .register_peer(winner_addr.clone())
        .await
        .expect("register winner");
    write_two_device_list(&winner, &loser, &loser_addr);
    write_two_device_list(&loser, &winner, &winner_addr);

    assert!(sync_ops::publish_full_export_snapshot(&winner, frozen_winner).await);
    assert!(sync_ops::publish_full_export_snapshot(&loser, frozen_loser).await);
    rua_sync::transport::sync_now(&winner_node, loser_addr, Duration::from_secs(30))
        .await
        .expect("sync merged namespace");
    winner
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("apply loser state on winner");
    loser
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("apply winner state on loser");

    for tmp in [&winner_tmp, &loser_tmp] {
        let prompts = tmp.path().join("prompt_library");
        assert_eq!(
            std::fs::read_to_string(prompts.join("local-winner.md")).unwrap(),
            "local user state winner"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("local-loser.md")).unwrap(),
            "local user state loser"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("modified-winner.md")).unwrap(),
            "custom seed winner"
        );
        assert_eq!(
            std::fs::read_to_string(prompts.join("modified-loser.md")).unwrap(),
            "custom seed loser"
        );
    }

    let merged = collect_all_v4_changes(&winner).await;
    for record_id in [
        "local-winner.md",
        "local-loser.md",
        "modified-winner.md",
        "modified-loser.md",
    ] {
        assert_eq!(
            merged
                .iter()
                .filter(|change| change.domain == "prompts" && change.record_id == record_id)
                .count(),
            1,
            "network merge must not echo {record_id} under the receiving Device author"
        );
    }
}

async fn build_first_sync_state(
    network: &rua_sync::identity::NetworkSecret,
    label: &str,
) -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("First Sync tempdir");
    seed_sync_identity_with_network(&tmp, network);
    seed_portable_state(&tmp).await;
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);

    let prompts = tmp.path().join("prompt_library");
    if prompts.exists() {
        std::fs::remove_dir_all(&prompts).expect("clear packaged prompts");
    }
    std::fs::create_dir_all(&prompts).expect("create First Sync prompt dir");
    std::fs::write(prompts.join("untouched.md"), "untouched builtin")
        .expect("write untouched seed");
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        &format!("prompt:untouched:{label}"),
        "First Sync test",
        &["prompt_library/untouched.md"],
    )
    .expect("record untouched seed");
    let modified_name = format!("modified-{label}.md");
    std::fs::write(
        prompts.join(&modified_name),
        format!("packaged seed {label}"),
    )
    .expect("write packaged seed");
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        &format!("prompt:modified:{label}"),
        "First Sync test",
        &[&format!("prompt_library/{modified_name}")],
    )
    .expect("record modified seed");
    std::fs::write(prompts.join(&modified_name), format!("custom seed {label}"))
        .expect("modify seed");
    std::fs::write(
        prompts.join(format!("local-{label}.md")),
        format!("local user state {label}"),
    )
    .expect("write local user prompt");
    (state, tmp)
}

fn local_endpoint_addr(node: &rua_sync::endpoint::SyncNode) -> iroh::EndpointAddr {
    let mut endpoint_addr = iroh::EndpointAddr::new(node.node_id());
    for socket in node.endpoint().bound_sockets() {
        let ip = match socket.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        endpoint_addr
            .addrs
            .insert(iroh::TransportAddr::Ip(SocketAddr::new(ip, socket.port())));
    }
    endpoint_addr
}

fn nearest_rank(values: &[u64], percentile: usize) -> u64 {
    let rank = values.len().saturating_mul(percentile).saturating_add(99) / 100;
    values[rank.saturating_sub(1).min(values.len() - 1)]
}

fn write_two_device_list(local: &AppState, remote: &AppState, remote_addr: &iroh::EndpointAddr) {
    let local_device = local
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .cloned()
        .expect("local Device");
    let remote_device = remote
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .cloned()
        .expect("remote Device");
    let entry =
        |device: &rua_sync::identity::DeviceIdentity, direct_addrs: Vec<String>| -> DeviceEntry {
            DeviceEntry {
                device_id: device.device_id_short(),
                public_key: *device.public_key().as_bytes(),
                label: "test".to_string(),
                friendly_name: hex::encode(device.device_id_short()),
                added_at_ms: 1,
                last_seen_ms: 1,
                direct_addrs,
            }
        };
    let list = DeviceList {
        devices: vec![
            entry(&local_device, Vec::new()),
            entry(
                &remote_device,
                remote_addr.ip_addrs().map(ToString::to_string).collect(),
            ),
        ],
        ..Default::default()
    };
    rua_sync::control_store::SyncControlStore::from_data_dir(&local.rua_root)
        .write_device_list(&list)
        .expect("write paired Device List");
}
