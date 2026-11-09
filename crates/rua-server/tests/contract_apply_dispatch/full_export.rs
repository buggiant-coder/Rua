#[test]
fn pairing_full_export_only_calls_v4_export() {
    let src = include_str!("../../src/endpoints/pairing.rs");
    let run_bootstrap_export = src
        .split("async fn run_bootstrap_export")
        .nth(1)
        .and_then(|tail| tail.split("async fn").next())
        .expect("run_bootstrap_export function should exist");

    assert!(run_bootstrap_export
        .contains("sync_ops::publish_full_export_snapshot(state, snapshot).await"));
    assert!(
        !run_bootstrap_export.contains("legacy_v2_full_export")
            && !run_bootstrap_export.contains("write_entity(")
            && !run_bootstrap_export.contains("write_tombstone("),
        "pairing full export must not seed or copy legacy sync business data"
    );
}

#[test]
fn bootstrap_export_is_not_prompt_only() {
    let src = include_str!("../../src/services/sync_ops.rs");
    let includes_domain = src
        .split("fn includes_domain")
        .nth(1)
        .and_then(|tail| tail.split("async fn full_export_v4_scope").next())
        .expect("FullExportScope::includes_domain should exist");

    assert!(
        !includes_domain
            .contains("Self::Bootstrap => domain == rua_sync::prompt_adapter::PROMPT_DOMAIN"),
        "pairing bootstrap must not report success after exporting only prompts"
    );
    assert!(
        includes_domain.contains("Self::Bootstrap => true"),
        "pairing bootstrap should seed all indexed sync domains"
    );
}

#[test]
fn pairing_freezes_full_export_before_starting_inbound_apply() {
    let src = include_str!("../../src/endpoints/pairing.rs");
    let handle_pairing_outcome = src
        .split("async fn handle_pairing_outcome")
        .nth(1)
        .and_then(|tail| tail.split("async fn reinit_sync_after_pairing").next())
        .expect("handle_pairing_outcome function should exist");
    let reinit_sync_after_pairing = src
        .split("async fn reinit_sync_after_pairing")
        .nth(1)
        .and_then(|tail| tail.split("async fn run_network_merge").next())
        .expect("reinit_sync_after_pairing function should exist");
    let run_network_merge = src
        .split("async fn run_network_merge")
        .nth(1)
        .and_then(|tail| tail.split("async fn write_migration_marker").next())
        .expect("run_network_merge function should exist");

    assert!(
        handle_pairing_outcome.contains("reinit_sync_after_pairing(state, None"),
        "pairing outcome handling must rebind the sync runtime instead of leaving a stale in-memory SyncNode on the old namespace"
    );

    let capture = reinit_sync_after_pairing
        .find("capture_full_export_v4_bootstrap(state)")
        .expect("First Sync must capture pre-pair state");
    let start_runtime = reinit_sync_after_pairing
        .find("ensure_sync_runtime_active()")
        .expect("First Sync must restart runtime");
    assert!(
        capture < start_runtime
            && reinit_sync_after_pairing.contains("schedule_post_pairing_full_export("),
        "First Sync must freeze export input before LiveActor can apply inbound state, then publish the frozen snapshot in the background"
    );

    let merge_capture = run_network_merge
        .find("capture_full_export_v4_bootstrap_for_network")
        .expect("network merge must capture pre-merge state");
    let namespace_switch = run_network_merge
        .find("winner_ns.save(sync_dir)")
        .expect("network merge must switch namespace");
    assert!(
        merge_capture < namespace_switch
            && run_network_merge.contains("Some(frozen_export)")
            && run_network_merge.contains("schedule_post_pairing_full_export"),
        "network merge must reuse the frozen Full Export boundary before switching namespaces"
    );

    let scheduler = src
        .split("fn schedule_post_pairing_full_export")
        .nth(1)
        .expect("background full export scheduler should exist");
    assert!(scheduler.contains("tokio::spawn"));
    assert!(scheduler.contains("run_bootstrap_export(&state, snapshot).await"));
    assert!(!scheduler.contains("run_bulk_export(&state).await"));
    assert!(
        !scheduler.contains("ServerProfile::MobileApk")
            && !scheduler.contains("should_push_after_export"),
        "foreground pairing must not suppress post-export propagation on Mobile APK"
    );
    assert!(
        scheduler.contains("if exported {")
            && scheduler.contains("run_first_sync_cycle_to_all_device_list_peers")
            && !scheduler.contains("emit_local_device_presence(&state).await")
            && !scheduler.contains("run_sync_cycle_to_all_device_list_peers("),
        "First Sync must publish the frozen Device List through Full Export and start exactly one readiness-aware Cycle"
    );
}

#[test]
fn pairing_failure_must_not_be_reported_as_paired_from_stale_device_list() {
    let src = include_str!("../../src/endpoints/pairing.rs");
    let initiate_pairing = src
        .split("async fn initiate_pairing")
        .nth(1)
        .and_then(|tail| tail.split("async fn pairing_status").next())
        .expect("initiate_pairing function should exist");

    let listener_error_branch = initiate_pairing
        .split("Ok(Err(e)) =>")
        .nth(1)
        .and_then(|tail| tail.split("Err(_) =>").next())
        .expect("listener error branch should exist");

    assert!(
        !listener_error_branch.contains("already_paired")
            && !listener_error_branch.contains("device_list shows peer already paired")
            && !listener_error_branch.contains("PairingResult::Paired"),
        "a failed pairing handshake must not be converted to Paired based on stale device_list; \
         otherwise network_secret.key can remain on the wrong namespace while the UI reports paired"
    );
}

#[test]
fn network_merge_does_not_migrate_business_data_from_legacy_sync_store() {
    let src = include_str!("../../src/endpoints/pairing.rs");
    let run_network_merge = src
        .split("async fn run_network_merge")
        .nth(1)
        .and_then(|tail| tail.split("async fn").next())
        .expect("run_network_merge function should exist");

    assert!(
        !run_network_merge.contains("read_all_decrypted")
            && !run_network_merge.contains("legacy_entry_to_v4_change")
            && !run_network_merge.contains("state.entity_store")
            && !run_network_merge.contains("open_entity_store_for_namespace"),
        "network merge must switch namespaces and use v4 full export, not migrate legacy sync business data"
    );
}

#[test]
fn sync_node_no_longer_exposes_cross_namespace_legacy_store_import() {
    let src = include_str!("../../../rua-sync/src/endpoint.rs");

    assert!(
        !src.contains("open_entity_store_for_namespace"),
        "SyncNode must not expose a normal-path helper for cross-namespace legacy business migration"
    );
}

#[test]
fn console_chat_session_sync_emitter_has_no_legacy_store_dependency() {
    let src = include_str!("../../src/services/console_chat.rs");
    let session_sync = src
        .split("pub struct SessionSyncEmitter")
        .nth(1)
        .and_then(|tail| tail.split("impl SessionSyncEmitter").next())
        .expect("SessionSyncEmitter struct should exist");

    assert!(
        !session_sync.contains("EntityStore") && !session_sync.contains("entity_store"),
        "console chat session sync emitter must emit v4 snapshots without carrying legacy store dependencies"
    );
}

#[test]
fn console_chat_evergreen_snapshot_uses_stable_conversation_publication_fact() {
    let src = include_str!("../../src/services/console_chat.rs");
    let emit_evergreen = src
        .split("async fn emit_evergreen_chat_snapshot")
        .nth(1)
        .and_then(|tail| tail.split("async fn emit_chat_index_upsert_json").next())
        .expect("emit_evergreen_chat_snapshot method should exist");

    assert!(
        emit_evergreen.contains("publish_sync_fact_with_runtime")
            && emit_evergreen.contains("SyncPublicationFact::StableConversationState")
            && emit_evergreen.contains("address: &rua_core::ChatAddress")
            && !emit_evergreen.contains("OpaqueReplicaLog::new"),
        "the single Evergreen Chat snapshot emitter must accept a complete ChatAddress and publish through the Replication Publication Seam"
    );
    assert_eq!(
        src.matches("async fn emit_evergreen_chat_snapshot").count(),
        1,
        "SessionSyncEmitter must expose exactly one Evergreen Chat snapshot method"
    );
    assert!(
        !src.contains("emit_evergreen_chat_snapshot_for"),
        "a second scope-preserving snapshot method would allow callers to select the wrong API"
    );
}

#[test]
fn submitted_user_turn_does_not_wait_for_best_effort_sync_publication() {
    let src = include_str!("../../src/services/console_chat.rs");
    let persisted_turn = src
        .split("if submitted_user_turn.is_some()")
        .nth(1)
        .and_then(|tail| tail.split("let submitted_turn_id").next())
        .expect("submitted user turn publication block should exist");

    assert!(
        persisted_turn.contains("spawn_chat_state_publication_for"),
        "a durable submitted user turn must schedule best-effort sync publication off the Live/runtime critical path"
    );
    assert!(
        !persisted_turn.contains("emit_evergreen_chat_snapshot"),
        "ordinary submitted user turns must not await Evergreen snapshot publication before materializing the Turn"
    );
    assert!(
        src.contains("publish_conversation_boundary"),
        "stable Model Call and terminal publication boundaries must remain explicit"
    );
}

#[test]
fn history_replacement_snapshot_preserves_project_chat_scope() {
    let src = include_str!("../../src/services/history_replacement.rs");
    let successful_publication = src
        .split("match result {")
        .nth(1)
        .and_then(|tail| tail.split("Ok(Some(_)) =>").nth(1))
        .and_then(|tail| tail.split("Ok(None) =>").next())
        .expect("successful history replacement publication branch should exist");

    assert!(
        successful_publication.contains(
            "emit_evergreen_chat_snapshot(&input.workspace_dir, &input.chat_address)",
        ),
        "history replacement snapshots must pass the complete ChatAddress to the single snapshot emitter"
    );
}

#[test]
fn publication_seam_routes_fact_change_construction_through_domain_catalog() {
    let src = include_str!("../../src/sync_publication.rs");
    let publication = src
        .split("pub async fn publish_sync_fact_with_runtime")
        .nth(1)
        .and_then(|tail| tail.split("async fn bound_sync_network").next())
        .expect("publication method should exist");

    assert!(
        publication.contains("build_publication_changes")
            && !publication.contains("ProviderSecretSyncAdapter::upsert_change")
            && !publication.contains("EvergreenChatSyncAdapter::snapshot_changes_for_chat"),
        "publication seam must route fact-to-v4-change construction through Sync Domain Catalog"
    );
}

#[test]
fn sync_ops_does_not_expose_v2_full_export_helper() {
    let src = include_str!("../../src/services/sync_ops.rs");

    assert!(!src.contains("legacy_v2_full_export"));
    assert!(!src.contains("pub async fn full_export("));
    assert!(!src.contains("write_entity("));
    assert!(!src.contains("write_tombstone("));
}

#[test]
fn full_export_preserves_priority_with_bounded_per_record_concurrency() {
    let src = include_str!("../../src/services/sync_ops.rs");
    let append_full_export_changes = src
        .split("async fn append_full_export_changes")
        .nth(1)
        .and_then(|tail| tail.split("fn full_export_priority").next())
        .expect("append_full_export_changes function should exist");

    assert!(
        src.contains("const FULL_EXPORT_MAX_CONCURRENCY: usize = 16"),
        "full export should keep an explicit concurrency bound"
    );
    assert!(
        append_full_export_changes.contains("full_export_priority")
            && append_full_export_changes.contains("group_end")
            && append_full_export_changes
                .contains(".buffer_unordered(FULL_EXPORT_MAX_CONCURRENCY)"),
        "full export should finish each priority group before starting bounded concurrent writes for the next group"
    );
    assert!(
        append_full_export_changes.contains("log.append_change(&change)")
            && !append_full_export_changes.contains("append_change_batch")
            && !append_full_export_changes.contains("append_packed_change_batch"),
        "full export execution grouping must still write one replica entry per record"
    );
    assert!(
        !append_full_export_changes.contains("MAX_BYTES")
            && !append_full_export_changes.contains("payload_size")
            && !append_full_export_changes.contains("serde_json::to_vec"),
        "execution concurrency must not grow a second payload-sizing policy"
    );
}

#[test]
fn app_state_loads_stable_device_credentials_outside_sync_dir() {
    let src = include_str!("../../src/state.rs");
    let load = src
        .split("pub fn load(")
        .nth(1)
        .and_then(|tail| tail.split("let providers_loader").next())
        .expect("AppState::load function should exist");

    assert!(
        load.contains("DeviceKey::generate_or_load(&secret_dir)"),
        "device-local encryption key must live under secret_dir, independent of sync state"
    );
    assert!(
        !load.contains("DeviceKey::generate_or_load(&sync_dir)"),
        "clearing data_dir/sync must not rotate the local encryption key"
    );
    assert!(
        load.contains("DeviceIdentity::generate_or_load(&data_dir)"),
        "stable Device Identity must live at data_dir/device_identity.json"
    );
    assert!(
        !load.contains("DeviceIdentity::generate_or_load(&sync_dir)"),
        "clearing data_dir/sync must not rotate the stable Device Identity"
    );
}

#[test]
fn v4_full_export_registers_all_portable_domain_adapters() {
    let domains = rua_server::sync_domain_catalog::EXPORT_DOMAIN_IDS;

    for domain in [
        "agent_config",
        "agent_security",
        "agent_template",
        "chat",
        "evergreen_chat",
        "device_list",
        "prompts",
        "root_config",
        "shared_mcp",
        "provider_secret",
        "skill_blob_manifest",
        "media",
        "agent_prompt_file",
        "structured_memory",
    ] {
        assert!(
            domains.contains(&domain),
            "export catalog must register {domain}"
        );
    }
}

#[test]
fn live_inbound_registry_registers_all_v4_apply_adapters() {
    let domains = rua_server::sync_domain_catalog::APPLY_DOMAIN_IDS;

    for domain in [
        "agent_config",
        "agent_security",
        "agent_template",
        "chat",
        "evergreen_chat",
        "device_list",
        "network_migration",
        "prompts",
        "agent_prompt_file",
        "structured_memory",
        "root_config",
        "shared_mcp",
        "provider_secret",
        "skill_blob_manifest",
        "media",
    ] {
        assert!(
            domains.contains(&domain),
            "apply catalog must register {domain}"
        );
    }
}

#[test]
fn inbound_apply_runtime_orchestration_is_not_embedded_in_app_state() {
    let state_src = include_str!("../../src/state.rs");

    assert!(
        !state_src.contains("struct InboundDrainContext")
            && !state_src.contains("async fn drain_inbound_inbox")
            && !state_src.contains("struct ActiveBlobDownload")
            && !state_src.contains("fn is_retryable_replica_log_content_missing_error"),
        "inbound apply orchestration should live in sync_inbound_apply, not AppState"
    );
    assert!(
        state_src.contains("sync_inbound_apply"),
        "AppState should delegate inbound apply work to the Inbound Apply Runtime module"
    );
}

#[test]
fn inbound_apply_runtime_prioritizes_metadata_and_does_not_let_blob_reads_block_queue() {
    let src = include_str!("../../src/sync_inbound_apply.rs");

    assert!(
        src.contains("domain_apply_priority")
            && src.contains("PROVIDER_SECRET_DOMAIN")
            && src.contains("ROOT_CONFIG_DOMAIN")
            && src.contains("AGENT_CONFIG_DOMAIN")
            && src.contains("CHAT_DOMAIN")
            && src.contains("PROMPT_DOMAIN")
            && src.contains("SKILL_BLOB_DOMAIN")
            && src.contains("MEDIA_DOMAIN"),
        "inbound apply must sort readable changes so provider/root/agent/chat/prompt metadata is applied before skill/media blob domains"
    );
    assert!(
        src.contains("tokio::time::timeout(timeout, read_future)")
            && src.contains("INBOUND_READ_TIMEOUT")
            && src.contains("INBOUND_ACTIVE_DOWNLOAD_TIMEOUT")
            && src.contains("v4 read/decrypt timed out")
            && src.contains("buffer_unordered(MAX_INBOUND_DRAIN_READ_CONCURRENCY")
            && src.contains("mark_inbound_retryable"),
        "inbound apply must read/decrypt entries concurrently, keep local reads short, give active replica-log content downloads a longer timeout, and mark slow reads retryable instead of blocking the batch"
    );
}

#[test]
fn inbound_apply_runtime_drains_multiple_batches_with_time_budget() {
    let src = include_str!("../../src/sync_inbound_apply.rs");

    assert!(
        src.contains("INBOUND_DRAIN_TIME_BUDGET")
            && src.contains("loop {")
            && src.contains("pending_ready_limited(now, MAX_INBOUND_DRAIN_PER_PASS)")
            && src.contains("started.elapsed() >= INBOUND_DRAIN_TIME_BUDGET"),
        "inbound apply must keep draining ready inbox batches until empty or a bounded time budget is reached"
    );
}

#[test]
fn inbound_apply_uses_typed_source_exhaustion_and_dormant_recovery_state() {
    let src = include_str!("../../src/sync_inbound_apply.rs");

    assert!(
        src.contains("SyncError::ReplicaLogContentUnavailable")
            && src.contains("MAX_AUTO_APPLY_ATTEMPTS")
            && src.contains("InboundReadResult::Dormant")
            && src.contains("RetryExhaustion::Dormant")
            && !src.contains("fn is_retryable_replica_log_content_missing_error"),
        "replica-log source exhaustion must use a typed error and enter a visible dormant recovery state instead of depending on error-message text or permanently discarding recoverable content"
    );
}

#[tokio::test]
async fn inbound_agent_config_apply_preserves_loaded_runtime_and_defers_unloaded_agents() {
    let temp = tempfile::tempdir().unwrap();
    let state = rua_server::AppState::from_tempdir(
        temp.path().to_path_buf(),
        rua_server::ServerProfile::Test,
    );
    state.wait_platform_initialization().await.unwrap();
    let inbound = rua_server::sync_inbound_apply::InboundApplyRuntime::from_state(&state);
    for agent_id in ["default", "unloaded-sync-agent"] {
        let before = state.registry.get(agent_id).await;
        let persisted = serde_json::json!({"clients":{"synced-fixture":{
            "name":"Synced fixture", "enabled":false, "transport":"stdio", "command":"fixture", "auto_connect":"manual"
        }}});
        state
            .agent_mcp_cells
            .cell(agent_id)
            .write_value(persisted)
            .await
            .unwrap();
        let change = rua_sync::opaque_replica_log::SyncChange::new(
            "local-fixture",
            "agent_config",
            agent_id,
            "upsert",
            "fixture",
            rua_sync::hlc::HlcTimestamp {
                wall_ms: 1,
                counter: 0,
                device_id: [0; 8],
            },
            serde_json::json!({}),
        );
        let applied = rua_sync::sync_registry::RegistryApplyResult {
            domain: "agent_config".to_string(),
            record_id: agent_id.to_string(),
            status: rua_sync::sync_registry::RegistryApplyStatus::Applied,
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            inbound.apply_agent_configuration_resources(&change, &applied),
        )
        .await
        .expect("configuration resource application must be bounded")
        .unwrap();
        let after = state.registry.get(agent_id).await;
        if let Some(runtime) = after.as_ref() {
            assert!(runtime
                .mcp_manager()
                .await
                .unwrap()
                .configuration()
                .await
                .unwrap()
                .clients
                .contains_key(&mcp_manager::types::McpServerId::new("synced-fixture")));
        }
        match (before, after) {
            (Some(before), Some(after)) => assert!(std::sync::Arc::ptr_eq(&before, &after)),
            (None, None) => {}
            _ => panic!("sync application changed Runtime presence"),
        }
    }
}

#[test]
fn inbound_evergreen_chat_apply_publishes_chat_activity_wake() {
    let src = include_str!("../../src/sync_inbound_apply.rs");

    assert!(
        src.contains("publish_evergreen_chat_sync_wake")
            && src.contains("EVERGREEN_CHAT_DOMAIN")
            && src.contains("active_stream_broadcast_hub")
            && src.contains("\"chat_sync_wake\""),
        "inbound evergreen_chat applies must wake active chat viewers so synced transcript files render without waiting for the next submit"
    );
}

#[test]
fn inbound_session_file_apply_publishes_local_placement_wake() {
    let src = include_str!("../../src/sync_inbound_apply.rs");
    assert!(
        src.contains("publish_chat_placement_sync_wake")
            && src.contains("EVERGREEN_CHAT_DOMAIN")
            && src.contains("relative != \"session\" && relative != \"session.json\"")
            && src.contains("ChatLiveWake::placement_changed"),
        "inbound Session File placement apply must wake mounted local viewers without reading Chat Index placement"
    );
}

#[test]
fn chat_options_publish_agent_config_from_authority_service() {
    let intake = include_str!("../../src/services/conversation_input_intake.rs");
    let endpoints = include_str!("../../src/endpoints/chats.rs");
    let update_options = endpoints
        .split("async fn update_chat_core")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn update_authority_placement_core")
                .next()
        })
        .expect("Chat options handler source");
    assert!(
        intake.contains("emit_agent_config_committed(")
            && intake.contains("serde_json::to_value(agent).unwrap_or_default()"),
        "authority-side running config update must publish Agent config"
    );
    assert!(
        !update_options.contains("publish_agent_config_committed"),
        "controller Chat options endpoint must not publish a second Agent config"
    );
}

#[test]
fn manual_sync_cycle_waits_for_local_repair_and_apply() {
    let sync_endpoint_src = include_str!("../../src/endpoints/sync.rs");
    let manual_cycle = sync_endpoint_src
        .split("async fn trigger_sync_cycle")
        .nth(1)
        .and_then(|tail| tail.split("async fn trigger_targeted_sync_cycle").next())
        .expect("trigger_sync_cycle function should exist");

    assert!(
        manual_cycle.contains("repair_scan_and_drain_sync_inbox_staged()")
            && manual_cycle.contains(".await"),
        "manual Sync now must wait for this Device's Repair Scan and Inbound Inbox drain"
    );
    assert!(
        !manual_cycle.contains("schedule_repair_scan_and_drain_sync_inbox")
            && manual_cycle.contains("\"repair_scan_failed\"")
            && manual_cycle.contains("\"inbox_drain_failed\"")
            && !manual_cycle.contains("SyncRuntimeHostProfile::MobileApk"),
        "manual Sync now must return precise local-completion failures instead of scheduling detached work"
    );
}

#[test]
fn ordinary_sync_cycle_attempts_only_the_best_peer() {
    let src = include_str!("../../src/sync_cycle.rs");
    let run_sync_cycle = src
        .split("pub async fn run_sync_cycle(")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub async fn run_sync_cycle_to_device_id")
                .next()
        })
        .expect("run_sync_cycle function should exist");

    assert!(
        run_sync_cycle.contains("ordered_peer_candidates_for_cycle")
            && run_sync_cycle.contains(".next()")
            && run_sync_cycle.contains("run_sync_cycle_candidate(")
            && !run_sync_cycle.contains("for candidate")
            && !run_sync_cycle.contains("run_sync_cycle_candidates"),
        "ordinary and manual Sync Cycles must attempt exactly one health-ordered best peer"
    );
}

#[test]
fn first_sync_waits_for_endpoint_readiness_and_retries_late_peer_startup() {
    let src = include_str!("../../src/sync_cycle.rs");
    let first_sync = src
        .split("pub async fn run_first_sync_cycle_to_all_device_list_peers")
        .nth(1)
        .and_then(|tail| tail.split("enum AllPeerRetryMode").next())
        .expect("First Sync cycle function should exist");
    let retry = src
        .split("async fn run_sync_cycle_candidate_with_retry")
        .nth(1)
        .expect("Sync candidate retry function should exist");

    assert!(
        first_sync.contains("node.endpoint().online()")
            && first_sync.contains("AllPeerRetryMode::FirstSyncReadiness"),
        "First Sync must give the new Endpoint a bounded online window before dialing peers"
    );
    assert!(
        retry.contains("retry_mode == AllPeerRetryMode::FirstSyncReadiness")
            && retry.contains("FIRST_SYNC_RETRY_DELAY"),
        "a peer Runtime that starts after its partner must be retried inside the same First Sync Cycle"
    );
}

#[test]
fn sync_transports_use_shared_address_resolution_without_device_session_streams() {
    let sync_cycle = include_str!("../../src/sync_cycle.rs");
    let inbound_apply = include_str!("../../src/sync_inbound_apply.rs");
    let peer_directory = include_str!("../../../rua-device-network/src/peer_directory.rs");
    let device_network_runtime = include_str!("../../../rua-device-network/src/runtime.rs");
    let device_network_gossip = include_str!("../../../rua-device-network/src/gossip.rs");
    let session = include_str!("../../../rua-device-network/src/session.rs");

    assert!(
        sync_cycle.contains("network_runtime()")
            && sync_cycle.contains("resolve_peer_addr(candidate.node_id)")
            && !sync_cycle.contains("EndpointAddr::new(candidate.node_id)"),
        "Docs Sync must use Device Network Runtime's shared peer-address resolver"
    );
    assert!(
        inbound_apply.contains("network_runtime.resolve_peer_addr(*provider).await"),
        "Blob providers must seed the shared address lookup before downloader connections"
    );
    let resolver = peer_directory
        .split("async fn resolve_entry")
        .nth(1)
        .and_then(|tail| tail.split("impl DevicePeerDirectory").next())
        .expect("Device peer resolver should exist");
    assert!(
        !resolver.contains("home_relay_status"),
        "a Device's local home Relay must never be installed as the remote peer's Relay"
    );
    assert!(
        !device_network_runtime.contains("let relay_url = self")
            && !device_network_gossip.contains("relay_url: Option<iroh::RelayUrl>"),
        "Device Network Gossip bootstrap must not snapshot the local Relay as peer addressing"
    );
    assert!(
        !session.contains("SYNC_CAPABILITY")
            && !session.contains("SyncNotification")
            && !session.contains("Sync Stream"),
        "shared address knowledge must not introduce a Device Session Sync message or stream"
    );
}

#[test]
fn sync_blob_downloader_is_stateful_and_preserves_provider_order() {
    let endpoint = include_str!("../../../rua-sync/src/endpoint.rs");
    let blob_store = include_str!("../../../rua-sync/src/sync_blob_store.rs");
    let replica_log = include_str!("../../../rua-sync/src/opaque_replica_log.rs");
    let sync_endpoint = include_str!("../../src/endpoints/sync.rs");

    assert!(
        endpoint.contains("blob_downloader: StatefulBlobDownloader")
            && endpoint.contains("blob_downloader.raw_downloader()")
            && endpoint.contains("pub fn blob_downloader(&self)"),
        "SyncNode must retain one downloader and share it with Docs plus missing-content reads"
    );
    assert!(
        blob_store.contains(
            "DownloadOptions::new(HashAndFormat::raw(hash), providers, SplitStrategy::None)"
        ) && !blob_store.contains("Shuffled::new")
            && !replica_log.contains("prime_blob_provider_route"),
        "Blob download must preserve health order and rely on the downloader connection pool"
    );
    assert!(
        sync_endpoint.contains("blob: rua_sync::sync_blob_store::BlobDownloadDiagnostics")
            && sync_endpoint.contains("node.blob_downloader().diagnostics()"),
        "Blob diagnostics must remain separate from Docs transport diagnostics"
    );
}

#[test]
fn app_state_coalesces_background_repair_scan_work() {
    let state_src = include_str!("../../src/state.rs");

    assert!(
        state_src.contains("SyncRepairLane")
            && state_src.contains("pending_lane")
            && state_src.contains("guard.schedule(lane)")
            && state_src.contains("guard.finish_active()")
            && state_src.contains("background repair scan/drain already active; queued lane"),
        "background repair scan/drain should be coalesced and lane-aware so repeated manual syncs do not spawn unbounded concurrent drains and interactive work is not dropped"
    );
}

#[test]
fn liveactor_repair_scan_enters_v4_inbox_not_legacy_store_scan() {
    let inbound_apply_src = include_str!("../../src/sync_inbound_apply.rs");
    let repair_scan_branch = inbound_apply_src
        .split("ApplyMode::RepairScan { force } =>")
        .nth(1)
        .and_then(|tail| tail.split("ApplyMode::Incremental").next())
        .expect("RepairScan branch should exist");

    assert!(
        repair_scan_branch.contains("run_repair_scan_if_needed")
            && inbound_apply_src.contains("repair_scan_doc_entries")
            && repair_scan_branch.contains("drain_bound_network"),
        "Repair Scan must run only when required, record entries into the durable inbox, and drain through v4 registry"
    );
    assert!(
        !repair_scan_branch.contains("sync_apply::apply_all")
            && !repair_scan_branch.contains("read_all_decrypted"),
        "Repair Scan must not directly scan legacy sync store"
    );
}

#[test]
fn runtime_state_does_not_hold_legacy_store_as_normal_sync_source() {
    let state_src = include_str!("../../src/state.rs");

    assert!(
        !state_src.contains("pub entity_store")
            && !state_src.contains("self.entity_store")
            && !state_src.contains("state.entity_store"),
        "AppState must not hold legacy sync store as a normal runtime source"
    );

    let pairing_src = include_str!("../../src/endpoints/pairing.rs");
    assert!(
        !pairing_src.contains("EntityStore::new") && !pairing_src.contains("state.entity_store"),
        "pairing must not create or persist legacy sync store state"
    );
}

#[test]
fn server_legacy_sync_store_symbols_are_absent() {
    let sources = [
        ("server/state", include_str!("../../src/state.rs")),
        (
            "server/pairing",
            include_str!("../../src/endpoints/pairing.rs"),
        ),
        ("server/sync", include_str!("../../src/endpoints/sync.rs")),
        (
            "server/sync_ops",
            include_str!("../../src/services/sync_ops.rs"),
        ),
        ("sync/lib", include_str!("../../../rua-sync/src/lib.rs")),
        (
            "sync/endpoint",
            include_str!("../../../rua-sync/src/endpoint.rs"),
        ),
    ];

    for (name, src) in sources {
        assert!(
            !src.contains("write_entity(") && !src.contains("write_tombstone("),
            "{name} must not write legacy sync records"
        );
        assert!(
            !src.contains("EntityStore::new")
                && !src.contains("read_single_decrypted")
                && !src.contains("read_all_decrypted")
                && !src.contains("LegacyEntityStoreMigration")
                && !src.contains("legacy_entity_migration")
                && !src.contains("sync_apply"),
            "{name} must not construct, scan, migrate, or dispatch legacy sync store data"
        );
    }
}
