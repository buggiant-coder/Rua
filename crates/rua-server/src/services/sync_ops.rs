use futures::stream::{self, StreamExt};
use rua_sync::opaque_replica_log::OpaqueReplicaLog;
use tracing::warn;

const FULL_EXPORT_MAX_CONCURRENCY: usize = 16;

pub async fn full_export_v4(state: &crate::state::AppState) -> bool {
    full_export_v4_scope(state, FullExportScope::All).await
}

pub async fn full_export_v4_bootstrap(state: &crate::state::AppState) -> bool {
    full_export_v4_scope(state, FullExportScope::Bootstrap).await
}

pub async fn full_export_v4_bulk(state: &crate::state::AppState) -> bool {
    full_export_v4_scope(state, FullExportScope::Bulk).await
}

#[derive(Debug, Clone)]
pub struct FullExportSnapshot {
    scope: FullExportScope,
    changes: Vec<rua_sync::opaque_replica_log::SyncChange>,
}

#[derive(Debug, Clone, Copy)]
enum FullExportScope {
    All,
    Bootstrap,
    Bulk,
}

impl FullExportScope {
    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Bootstrap => "bootstrap",
            Self::Bulk => "bulk",
        }
    }

    fn includes_domain(self, domain: &str) -> bool {
        match self {
            Self::All => true,

            Self::Bootstrap => true,
            Self::Bulk => domain != rua_sync::prompt_adapter::PROMPT_DOMAIN,
        }
    }
}

async fn full_export_v4_scope(state: &crate::state::AppState, scope: FullExportScope) -> bool {
    let Some((_, blobs_store, sync_key)) =
        build_v4_log_and_blob_context(state, "full export").await
    else {
        return false;
    };
    let Some(snapshot) =
        capture_full_export_v4_with_context(state, scope, blobs_store, sync_key).await
    else {
        return false;
    };
    publish_full_export_snapshot(state, snapshot).await
}

pub async fn capture_full_export_v4_bootstrap(
    state: &crate::state::AppState,
) -> Option<FullExportSnapshot> {
    let (_, blobs_store, sync_key) =
        build_v4_log_and_blob_context(state, "First Sync capture").await?;
    capture_full_export_v4_with_context(state, FullExportScope::Bootstrap, blobs_store, sync_key)
        .await
}

pub async fn capture_full_export_v4_bootstrap_for_network(
    state: &crate::state::AppState,
    blobs_store: rua_sync::endpoint::BlobsApiStore,
    network_secret: &rua_sync::identity::NetworkSecret,
) -> Option<FullExportSnapshot> {
    capture_full_export_v4_with_context(
        state,
        FullExportScope::Bootstrap,
        blobs_store,
        network_secret.derive_sync_key(),
    )
    .await
}

async fn capture_full_export_v4_with_context(
    state: &crate::state::AppState,
    scope: FullExportScope,
    blobs_store: rua_sync::endpoint::BlobsApiStore,
    sync_key: rua_sync::crypto::SyncKey,
) -> Option<FullExportSnapshot> {
    tracing::info!(scope = scope.label(), "[sync-v4] full_export: capturing");

    let Some(sync_meta) = build_sync_meta(state) else {
        warn!("[sync-v4] full_export skipped: sync_meta unavailable");
        return None;
    };

    let device_id = sync_meta.device_id.clone();
    let device_key = *state.device_key.as_bytes();
    let registry = match crate::sync_domain_catalog::build_export_registry(
        state.rua_root.clone(),
        state.secret_dir.clone(),
        device_id,
        device_key,
        blobs_store,
        sync_key,
        std::sync::Arc::clone(&state.provider_manager),
        std::sync::Arc::clone(&state.project_catalog),
    ) {
        Ok(registry) => registry,
        Err(e) => {
            warn!(error = %e, "[sync-v4] full_export: build domain catalog");
            return None;
        }
    };

    let (changes, export_errors) = registry.export_all_best_effort().await;
    for (domain, error) in export_errors {
        warn!(domain = %domain, error = %error, "[sync-v4] full_export: export adapter skipped");
    }
    let changes = changes
        .into_iter()
        .filter(|change| scope.includes_domain(&change.domain))
        .collect::<Vec<_>>();
    tracing::info!(
        scope = scope.label(),
        total = changes.len(),
        "[sync-v4] full_export: captured"
    );
    Some(FullExportSnapshot { scope, changes })
}

pub async fn publish_full_export_snapshot(
    state: &crate::state::AppState,
    snapshot: FullExportSnapshot,
) -> bool {
    let Some((log, _, _)) =
        build_v4_log_and_blob_context(state, "frozen full export publication").await
    else {
        return false;
    };
    let total = snapshot.changes.len();
    let mut appended = 0usize;
    let (device_changes, other_changes): (Vec<_>, Vec<_>) = snapshot
        .changes
        .into_iter()
        .partition(|change| change.domain == rua_sync::device_list_adapter::DEVICE_LIST_DOMAIN);
    if !device_changes.is_empty() {
        // A frozen capture can predate a local rename/removal. Serialize the
        // current Device name read with append so an older export cannot compact
        // a newer committed name out of this author's replica-log row.
        let _publication_guard = crate::sync_publication::DEVICE_LIST_PUBLICATION_LOCK
            .lock()
            .await;
        let current =
            match rua_sync::control_store::SyncControlStore::from_data_dir(&state.rua_root)
                .read_device_list()
            {
                Ok(current) => current,
                Err(error) => {
                    warn!(%error, "[sync-v4] full_export: read current Device names");
                    return false;
                }
            };
        let mut refreshed = Vec::new();
        for change in device_changes {
            let entry: rua_sync::device_list::DeviceEntry =
                match serde_json::from_value(change.payload.clone()) {
                    Ok(entry) => entry,
                    Err(error) => {
                        warn!(%error, "[sync-v4] full_export: invalid captured Device entry");
                        return false;
                    }
                };
            if !current.contains_device(&entry.device_id) {
                continue;
            }
            let name_record = current.name_records.get(&change.record_id).cloned();
            refreshed.push(rua_sync::device_list_adapter::DeviceListSyncAdapter::snapshot_change_with_name_record(
                rua_sync::device_list_adapter::DeviceListSyncAdapter::new_op_id(), entry, name_record, change.device_id, change.hlc,
            ));
        }
        append_full_export_changes(&log, &refreshed, &mut appended).await;
    }
    append_full_export_changes(&log, &other_changes, &mut appended).await;

    tracing::info!(
        scope = snapshot.scope.label(),
        total,
        appended,
        "[sync-v4] full_export: completed"
    );
    true
}

async fn append_full_export_changes(
    log: &OpaqueReplicaLog,
    changes: &[rua_sync::opaque_replica_log::SyncChange],
    appended: &mut usize,
) {
    let mut ordered = changes.to_vec();
    ordered.sort_by(|left, right| {
        full_export_priority(&left.domain)
            .cmp(&full_export_priority(&right.domain))
            .then_with(|| left.domain.cmp(&right.domain))
            .then_with(|| left.record_id.cmp(&right.record_id))
    });

    let mut group_start = 0usize;
    while group_start < ordered.len() {
        let priority = full_export_priority(&ordered[group_start].domain);
        let group_len = ordered[group_start..]
            .partition_point(|change| full_export_priority(&change.domain) == priority);
        let group_end = group_start + group_len;
        let results = stream::iter(ordered[group_start..group_end].iter().cloned())
            .map(|change| async move {
                let result = log.append_change(&change).await;
                (change, result)
            })
            .buffer_unordered(FULL_EXPORT_MAX_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

        for (change, result) in results {
            match result {
                Ok(_) => *appended += 1,
                Err(e) => warn!(
                    error = %e,
                    domain = %change.domain,
                    record_id = %change.record_id,
                    "[sync-v4] full_export: append change"
                ),
            }
        }
        group_start = group_end;
    }
}

fn full_export_priority(domain: &str) -> u8 {
    match domain {
        rua_sync::provider_adapter::PROVIDER_SECRET_DOMAIN
        | rua_sync::root_config_adapter::ROOT_CONFIG_DOMAIN
        | rua_sync::device_list_adapter::DEVICE_LIST_DOMAIN
        | rua_sync::project_adapter::PROJECT_CATALOG_DOMAIN
        | rua_sync::project_sidebar_adapter::PROJECT_SIDEBAR_DOMAIN
        | rua_sync::prompt_adapter::PROMPT_DOMAIN => 0,
        rua_sync::agent_adapter::AGENT_CONFIG_DOMAIN
        | rua_sync::agent_security_adapter::AGENT_SECURITY_DOMAIN
        | rua_sync::agent_template_adapter::AGENT_TEMPLATE_DOMAIN
        | rua_sync::shared_mcp_adapter::SHARED_MCP_DOMAIN => 1,
        rua_sync::chat_adapter::CHAT_DOMAIN
        | rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_DOMAIN
        | rua_sync::agent_prompt_file_adapter::AGENT_PROMPT_FILE_DOMAIN => 2,
        rua_sync::skill_adapter::SKILL_BLOB_DOMAIN
        | rua_sync::media_adapter::MEDIA_DOMAIN
        | rua_sync::structured_memory_adapter::STRUCTURED_MEMORY_DOMAIN => 3,
        _ => 4,
    }
}

async fn build_v4_log_and_blob_context(
    state: &crate::state::AppState,
    label: &str,
) -> Option<(
    OpaqueReplicaLog,
    rua_sync::endpoint::BlobsApiStore,
    rua_sync::crypto::SyncKey,
)> {
    let bound_network = match state.bound_sync_network().await {
        Ok(bound_network) => bound_network,
        Err(error) => {
            warn!(label, error = %error, "[sync-v4] append skipped: bound sync network unavailable");
            return None;
        }
    };
    let log = bound_network.opaque_replica_log();
    let (blobs_store, sync_key) = bound_network.blob_context();
    Some((log, blobs_store, sync_key))
}

pub fn build_sync_meta(state: &crate::state::AppState) -> Option<rua_sync::event::SyncMeta> {
    let device_guard = match state.sync_device.read() {
        Ok(g) => g,
        Err(_) => {
            warn!("[sync-v4] build_sync_meta: sync_device RwLock poisoned");
            return None;
        }
    };
    let device = match device_guard.as_ref() {
        Some(d) => d,
        None => {
            warn!("[sync-v4] build_sync_meta: sync_device not set");
            return None;
        }
    };
    let device_id_bytes = device.device_id_short();
    let hlc = {
        let mut guard = match state.sync_hlc_clock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("[sync-v4] build_sync_meta: sync_hlc_clock Mutex poisoned");
                poisoned.into_inner()
            }
        };
        let reset_clock = guard
            .as_ref()
            .map(|(current_device, _)| *current_device != device_id_bytes)
            .unwrap_or(true);
        if reset_clock {
            *guard = Some((
                device_id_bytes,
                rua_sync::hlc::HlcClock::new(device_id_bytes),
            ));
        }
        guard
            .as_mut()
            .map(|(_, clock)| clock.now())
            .unwrap_or_else(|| rua_sync::hlc::HlcClock::new(device_id_bytes).now())
    };
    Some(rua_sync::event::SyncMeta {
        device_id: hex::encode(device_id_bytes),
        hlc,
        modified_at: chrono::Utc::now().to_rfc3339(),
    })
}
