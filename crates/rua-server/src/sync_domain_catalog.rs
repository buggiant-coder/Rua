use rua_sync::project_sidebar_adapter::{ProjectSidebarSyncAdapter, PROJECT_SIDEBAR_DOMAIN};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_sync::agent_adapter::{AgentConfigSyncAdapter, AGENT_CONFIG_DOMAIN};
use rua_sync::agent_prompt_file_adapter::{AgentPromptFileSyncAdapter, AGENT_PROMPT_FILE_DOMAIN};
use rua_sync::agent_security_adapter::{AgentSecuritySyncAdapter, AGENT_SECURITY_DOMAIN};
use rua_sync::agent_template_adapter::{AgentTemplateSyncAdapter, AGENT_TEMPLATE_DOMAIN};
use rua_sync::chat_adapter::{ChatIndexSyncAdapter, CHAT_DOMAIN};
use rua_sync::crypto::SyncKey;
use rua_sync::device_list_adapter::{DeviceListSyncAdapter, DEVICE_LIST_DOMAIN};
use rua_sync::endpoint::BlobsApiStore;
use rua_sync::event::SyncMeta;
use rua_sync::evergreen_chat_adapter::{EvergreenChatSyncAdapter, EVERGREEN_CHAT_DOMAIN};
use rua_sync::media_adapter::{MediaSyncAdapter, MEDIA_DOMAIN};
use rua_sync::network_migration_adapter::{NetworkMigrationSyncAdapter, NETWORK_MIGRATION_DOMAIN};
use rua_sync::project_adapter::{ProjectCatalogSyncAdapter, PROJECT_CATALOG_DOMAIN};
use rua_sync::prompt_adapter::{PromptLibrarySyncAdapter, PROMPT_DOMAIN};
use rua_sync::provider_adapter::{ProviderSecretSyncAdapter, PROVIDER_SECRET_DOMAIN};
use rua_sync::root_config_adapter::{
    RootConfigSyncAdapter, ROOT_CONFIG_DOMAIN, ROOT_CONFIG_RECORD_AGENTS,
    ROOT_CONFIG_RECORD_SECURITY, ROOT_CONFIG_RECORD_SETTINGS,
    ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES, ROOT_CONFIG_RECORD_USER_TIMEZONE,
};
use rua_sync::shared_mcp_adapter::{SharedMcpSyncAdapter, SHARED_MCP_DOMAIN};
use rua_sync::skill_adapter::{SkillBlobManifestSyncAdapter, SKILL_BLOB_DOMAIN};
use rua_sync::structured_memory_adapter::{StructuredMemorySyncAdapter, STRUCTURED_MEMORY_DOMAIN};
use rua_sync::sync_registry::SyncRegistry;

use crate::sync_publication::SyncPublicationFact;

pub const EXPORT_DOMAIN_IDS: &[&str] = &[
    AGENT_CONFIG_DOMAIN,
    AGENT_SECURITY_DOMAIN,
    AGENT_TEMPLATE_DOMAIN,
    CHAT_DOMAIN,
    EVERGREEN_CHAT_DOMAIN,
    DEVICE_LIST_DOMAIN,
    PROJECT_CATALOG_DOMAIN,
    PROJECT_SIDEBAR_DOMAIN,
    PROMPT_DOMAIN,
    ROOT_CONFIG_DOMAIN,
    SHARED_MCP_DOMAIN,
    PROVIDER_SECRET_DOMAIN,
    SKILL_BLOB_DOMAIN,
    MEDIA_DOMAIN,
    AGENT_PROMPT_FILE_DOMAIN,
    STRUCTURED_MEMORY_DOMAIN,
];

pub const APPLY_DOMAIN_IDS: &[&str] = &[
    AGENT_CONFIG_DOMAIN,
    AGENT_SECURITY_DOMAIN,
    AGENT_TEMPLATE_DOMAIN,
    CHAT_DOMAIN,
    EVERGREEN_CHAT_DOMAIN,
    DEVICE_LIST_DOMAIN,
    PROJECT_CATALOG_DOMAIN,
    PROJECT_SIDEBAR_DOMAIN,
    NETWORK_MIGRATION_DOMAIN,
    PROMPT_DOMAIN,
    AGENT_PROMPT_FILE_DOMAIN,
    STRUCTURED_MEMORY_DOMAIN,
    ROOT_CONFIG_DOMAIN,
    SHARED_MCP_DOMAIN,
    PROVIDER_SECRET_DOMAIN,
    SKILL_BLOB_DOMAIN,
    MEDIA_DOMAIN,
];

#[derive(Clone)]
pub struct BlobDownloadContext {
    pub downloader: rua_sync::sync_blob_store::StatefulBlobDownloader,
    pub providers: Vec<iroh::EndpointId>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_export_registry(
    data_dir: PathBuf,
    secret_dir: PathBuf,
    device_id: String,
    device_key: [u8; 32],
    blobs_store: BlobsApiStore,
    sync_key: SyncKey,
    provider_manager: Arc<rua_provider::manager::ProviderManager>,
    project_catalog: Arc<rua_project::ProjectCatalog>,
) -> rua_sync::SyncResult<SyncRegistry> {
    let mut registry = SyncRegistry::new();
    registry.register(Arc::new(
        AgentConfigSyncAdapter::new(data_dir.clone(), device_id.clone())
            .with_local_mcp_key(device_key),
    ))?;
    registry.register(Arc::new(AgentSecuritySyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(AgentTemplateSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(ChatIndexSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(EvergreenChatSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(DeviceListSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(ProjectSidebarSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(ProjectCatalogSyncAdapter::new(
        project_catalog,
        device_id.clone(),
    )))?;
    registry.register(Arc::new(PromptLibrarySyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(RootConfigSyncAdapter::new(
        data_dir.clone(),
        secret_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(
        SharedMcpSyncAdapter::new(data_dir.clone(), device_id.clone())
            .with_local_mcp_key(device_key),
    ))?;
    registry.register(Arc::new(
        ProviderSecretSyncAdapter::new(secret_dir, device_id.clone())
            .with_local_secret_key(device_key)
            .with_provider_manager(provider_manager),
    ))?;
    registry.register(Arc::new(
        SkillBlobManifestSyncAdapter::new(data_dir.clone(), device_id.clone())
            .with_blob_store(blobs_store.clone(), sync_key.clone()),
    ))?;
    registry.register(Arc::new(
        MediaSyncAdapter::new(data_dir.clone(), device_id.clone())
            .with_blob_store(blobs_store, sync_key),
    ))?;
    registry.register(Arc::new(AgentPromptFileSyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    registry.register(Arc::new(StructuredMemorySyncAdapter::new(
        data_dir.clone(),
        device_id.clone(),
    )))?;
    Ok(registry)
}

#[allow(clippy::too_many_arguments)]
pub fn build_apply_registry(
    data_dir: PathBuf,
    secret_dir: PathBuf,
    local_device_id: String,
    device_key: [u8; 32],
    provider_manager: Arc<rua_provider::manager::ProviderManager>,
    _security: Arc<rua_security::SecurityModule>,
    blob_context: Option<(BlobsApiStore, SyncKey)>,
    blob_download: Option<BlobDownloadContext>,
    project_catalog: Arc<rua_project::ProjectCatalog>,
) -> rua_sync::SyncResult<SyncRegistry> {
    let mut registry = SyncRegistry::new();
    registry.register(Arc::new(
        AgentConfigSyncAdapter::new(data_dir.clone(), local_device_id.clone())
            .with_local_mcp_key(device_key),
    ))?;
    registry.register(Arc::new(AgentSecuritySyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(AgentTemplateSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(ChatIndexSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(EvergreenChatSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(DeviceListSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(ProjectSidebarSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(ProjectCatalogSyncAdapter::new(
        project_catalog,
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(NetworkMigrationSyncAdapter::new(data_dir.clone())))?;
    registry.register(Arc::new(PromptLibrarySyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(AgentPromptFileSyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(StructuredMemorySyncAdapter::new(
        data_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(RootConfigSyncAdapter::new(
        data_dir.clone(),
        secret_dir.clone(),
        local_device_id.clone(),
    )))?;
    registry.register(Arc::new(
        SharedMcpSyncAdapter::new(data_dir.clone(), local_device_id.clone())
            .with_local_mcp_key(device_key),
    ))?;
    registry.register(Arc::new(
        ProviderSecretSyncAdapter::new(secret_dir, local_device_id.clone())
            .with_local_secret_key(device_key)
            .with_provider_manager(provider_manager),
    ))?;

    if let Some((blobs_store, sync_key)) = blob_context {
        let skill_adapter =
            SkillBlobManifestSyncAdapter::new(data_dir.clone(), local_device_id.clone())
                .with_blob_store(blobs_store.clone(), sync_key.clone());
        let skill_adapter = if let Some(download) = blob_download.clone() {
            skill_adapter.with_blob_download(download.downloader, download.providers)
        } else {
            skill_adapter
        };
        registry.register(Arc::new(skill_adapter))?;

        let media_adapter =
            MediaSyncAdapter::new(data_dir, local_device_id).with_blob_store(blobs_store, sync_key);
        let media_adapter = if let Some(download) = blob_download {
            media_adapter.with_blob_download(download.downloader, download.providers)
        } else {
            media_adapter
        };
        registry.register(Arc::new(media_adapter))?;
    }

    Ok(registry)
}

#[derive(Clone)]
pub struct PublicationChangeBuildContext<'a> {
    pub data_dir: &'a Path,
    pub blob_context: Option<(BlobsApiStore, SyncKey)>,
}

pub async fn build_publication_changes(
    fact: SyncPublicationFact,
    sync_meta: SyncMeta,
    context: PublicationChangeBuildContext<'_>,
) -> Result<Vec<rua_sync::opaque_replica_log::SyncChange>, String> {
    Ok(match fact {
        SyncPublicationFact::ProviderConfigCommitted {
            provider_id,
            config,
        } => vec![ProviderSecretSyncAdapter::upsert_change(
            ProviderSecretSyncAdapter::new_op_id(),
            provider_id,
            config,
            sync_meta.device_id,
            sync_meta.hlc,
        )],
        SyncPublicationFact::ProviderConfigDeleted { provider_id } => {
            vec![ProviderSecretSyncAdapter::delete_change(
                ProviderSecretSyncAdapter::new_op_id(),
                provider_id,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::ChatIndexUpserted { address, chat } => {
            vec![ChatIndexSyncAdapter::index_change(
                ChatIndexSyncAdapter::new_op_id(),
                address,
                chat,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::ChatIndexDeleted { address } => {
            vec![ChatIndexSyncAdapter::delete_change(
                ChatIndexSyncAdapter::new_op_id(),
                &address,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::ChatDeleted {
            scope_root,
            address,
        } => {
            let mut changes =
                rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots(
                    &scope_root,
                    address.chat_id().as_str(),
                    &sync_meta.device_id,
                )
                .into_iter()
                .map(|snapshot| {
                    EvergreenChatSyncAdapter::delete_change(
                        EvergreenChatSyncAdapter::new_op_id(),
                        &address,
                        snapshot.relative,
                        sync_meta.device_id.clone(),
                        sync_meta.hlc,
                    )
                })
                .collect::<Vec<_>>();
            changes.push(ChatIndexSyncAdapter::delete_change(
                ChatIndexSyncAdapter::new_op_id(),
                &address,
                sync_meta.device_id,
                sync_meta.hlc,
            ));
            changes
        }
        SyncPublicationFact::MediaDeleted { owner, media_id } => {
            vec![MediaSyncAdapter::delete_change(
                MediaSyncAdapter::new_op_id(),
                &owner,
                &media_id,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::PromptLibraryFilesCommitted { changes } => changes
            .into_iter()
            .map(|change| PromptLibrarySyncAdapter::committed_change(change, &sync_meta.device_id))
            .collect(),
        SyncPublicationFact::AgentTemplateCommitted {
            template_id,
            template,
        } => vec![AgentTemplateSyncAdapter::upsert_change(
            AgentTemplateSyncAdapter::new_op_id(),
            template_id,
            template,
            sync_meta.device_id,
            sync_meta.hlc,
        )],
        SyncPublicationFact::AgentTemplateDeleted { template_id } => {
            vec![AgentTemplateSyncAdapter::delete_change(
                AgentTemplateSyncAdapter::new_op_id(),
                template_id,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::SubagentTemplatesCommitted { templates } => {
            vec![RootConfigSyncAdapter::upsert_change(
                RootConfigSyncAdapter::new_op_id(),
                ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES,
                templates,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::RootSettingsCommitted { settings } => {
            vec![RootConfigSyncAdapter::upsert_change(
                RootConfigSyncAdapter::new_op_id(),
                ROOT_CONFIG_RECORD_SETTINGS,
                settings,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::RootAgentRegistryCommitted { agents } => {
            vec![RootConfigSyncAdapter::upsert_change(
                RootConfigSyncAdapter::new_op_id(),
                ROOT_CONFIG_RECORD_AGENTS,
                agents,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::RootUserTimezoneCommitted { timezone } => {
            vec![RootConfigSyncAdapter::upsert_change(
                RootConfigSyncAdapter::new_op_id(),
                ROOT_CONFIG_RECORD_USER_TIMEZONE,
                timezone,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::RootProcessSecurityCommitted { security } => {
            vec![RootConfigSyncAdapter::upsert_change(
                RootConfigSyncAdapter::new_op_id(),
                ROOT_CONFIG_RECORD_SECURITY,
                strip_root_security_device_local(security),
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::AgentConfigCommitted { agent_id, config } => {
            vec![AgentConfigSyncAdapter::upsert_change(
                AgentConfigSyncAdapter::new_op_id(),
                agent_id,
                config,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::AgentConfigDeleted { agent_id } => {
            vec![AgentConfigSyncAdapter::delete_change(
                AgentConfigSyncAdapter::new_op_id(),
                agent_id,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::AgentSecurityCommitted { agent_id, config } => {
            vec![AgentSecuritySyncAdapter::upsert_change(
                AgentSecuritySyncAdapter::new_op_id(),
                agent_id,
                config,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::AgentSecurityDeleted { agent_id } => {
            vec![AgentSecuritySyncAdapter::delete_change(
                AgentSecuritySyncAdapter::new_op_id(),
                agent_id,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::SharedMcpCatalogCommitted { catalog } => {
            vec![SharedMcpSyncAdapter::upsert_change(
                SharedMcpSyncAdapter::new_op_id(),
                catalog,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::SharedMcpClientDeleted { client_key } => {
            vec![SharedMcpSyncAdapter::client_delete_change(
                SharedMcpSyncAdapter::new_op_id(),
                &client_key,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::AgentPromptFilesCommitted { agent_id, changes } => changes
            .into_iter()
            .filter(|change| {
                !change
                    .path
                    .split('/')
                    .next()
                    .is_some_and(|dir| dir.eq_ignore_ascii_case("MEMORY"))
            })
            .map(|change| {
                AgentPromptFileSyncAdapter::committed_change(
                    &agent_id,
                    change,
                    &sync_meta.device_id,
                )
            })
            .collect(),
        SyncPublicationFact::StructuredMemoryRecordCommitted { record } => {
            vec![StructuredMemorySyncAdapter::snapshot_change(
                StructuredMemorySyncAdapter::new_op_id(),
                &record,
                sync_meta.device_id,
                sync_meta.hlc,
            )
            .map_err(|e| format!("structured memory snapshot change: {e}"))?]
        }
        SyncPublicationFact::StructuredMemoryVisibilityCommitted { visibility } => {
            vec![StructuredMemorySyncAdapter::visibility_snapshot_change(
                StructuredMemorySyncAdapter::new_op_id(),
                &visibility,
                sync_meta.device_id,
                sync_meta.hlc,
            )
            .map_err(|e| format!("structured memory visibility snapshot change: {e}"))?]
        }
        SyncPublicationFact::StructuredMemoryRecordDeleted {
            agent_id,
            memory_id,
        } => vec![StructuredMemorySyncAdapter::delete_change(
            StructuredMemorySyncAdapter::new_op_id(),
            &agent_id,
            &memory_id.to_string(),
            sync_meta.device_id,
            sync_meta.hlc,
        )],
        SyncPublicationFact::SkillPoolCommitted { skill_name } => {
            let (blobs_store, sync_key) = context
                .blob_context
                .clone()
                .ok_or_else(|| "skill publication requires blob context".to_string())?;
            let adapter = SkillBlobManifestSyncAdapter::new(
                context.data_dir.to_path_buf(),
                sync_meta.device_id.clone(),
            )
            .with_blob_store(blobs_store, sync_key);
            vec![adapter
                .snapshot_change_for_skill(
                    &skill_name,
                    SkillBlobManifestSyncAdapter::new_op_id(),
                    sync_meta.device_id,
                    sync_meta.hlc,
                )
                .await
                .map_err(|error| error.to_string())?]
        }
        SyncPublicationFact::SkillPoolDeleted { skill_name } => {
            vec![SkillBlobManifestSyncAdapter::delete_change(
                SkillBlobManifestSyncAdapter::new_op_id(),
                skill_name,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::WorkspaceSkillCommitted {
            agent_id,
            skill_name,
        } => {
            let (blobs_store, sync_key) = context
                .blob_context
                .clone()
                .ok_or_else(|| "workspace skill publication requires blob context".to_string())?;
            let adapter = SkillBlobManifestSyncAdapter::new(
                context.data_dir.to_path_buf(),
                sync_meta.device_id.clone(),
            )
            .with_blob_store(blobs_store, sync_key);
            vec![adapter
                .snapshot_change_for_workspace_skill(
                    &agent_id,
                    &skill_name,
                    SkillBlobManifestSyncAdapter::new_op_id(),
                    sync_meta.device_id,
                    sync_meta.hlc,
                )
                .await
                .map_err(|error| error.to_string())?]
        }
        SyncPublicationFact::WorkspaceSkillDeleted {
            agent_id,
            skill_name,
        } => vec![SkillBlobManifestSyncAdapter::workspace_delete_change(
            SkillBlobManifestSyncAdapter::new_op_id(),
            agent_id,
            skill_name,
            sync_meta.device_id,
            sync_meta.hlc,
        )],
        SyncPublicationFact::DeviceListEntrySnapshot { entry } => {
            vec![DeviceListSyncAdapter::snapshot_change(
                DeviceListSyncAdapter::new_op_id(),
                entry,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::DeviceListDeviceRemoved { device_id_hex } => {
            vec![DeviceListSyncAdapter::remove_change(
                DeviceListSyncAdapter::new_op_id(),
                device_id_hex,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::DeviceListDeviceRenamed {
            device_id_hex,
            friendly_name,
        } => {
            vec![DeviceListSyncAdapter::rename_change(
                DeviceListSyncAdapter::new_op_id(),
                device_id_hex,
                friendly_name,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::ProjectSidebarRecordsCommitted { records } => records
            .into_iter()
            .map(|record| {
                ProjectSidebarSyncAdapter::record_change(
                    record,
                    sync_meta.device_id.clone(),
                    sync_meta.hlc,
                )
            })
            .collect(),
        SyncPublicationFact::ProjectDescriptorCommitted { descriptor } => {
            vec![ProjectCatalogSyncAdapter::descriptor_change(
                ProjectCatalogSyncAdapter::new_op_id(),
                descriptor,
                sync_meta.device_id,
                sync_meta.hlc,
            )]
        }
        SyncPublicationFact::ProjectPermanentlyDeleted {
            project_id,
            scope_root,
            chat_addresses,
        } => {
            let mut changes = Vec::new();
            for address in chat_addresses {
                changes.extend(
                    rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots(
                        &scope_root,
                        address.chat_id().as_str(),
                        &sync_meta.device_id,
                    )
                    .into_iter()
                    .map(|snapshot| {
                        EvergreenChatSyncAdapter::delete_change(
                            EvergreenChatSyncAdapter::new_op_id(),
                            &address,
                            snapshot.relative,
                            sync_meta.device_id.clone(),
                            sync_meta.hlc,
                        )
                    }),
                );
                changes.push(ChatIndexSyncAdapter::delete_change(
                    ChatIndexSyncAdapter::new_op_id(),
                    &address,
                    sync_meta.device_id.clone(),
                    sync_meta.hlc,
                ));
            }
            changes.push(ProjectCatalogSyncAdapter::delete_change(
                ProjectCatalogSyncAdapter::new_op_id(),
                project_id.to_string(),
                sync_meta.device_id,
                sync_meta.hlc,
            ));
            changes
        }
        SyncPublicationFact::StableConversationState {
            scope_root,
            address,
        } => EvergreenChatSyncAdapter::snapshot_changes_for_chat(
            &scope_root,
            &address,
            sync_meta.device_id,
            sync_meta.hlc,
        )
        .map_err(|error| error.to_string())?,
        SyncPublicationFact::ConversationStateDeleted {
            scope_root,
            address,
        } => rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots(
            &scope_root,
            address.chat_id().as_str(),
            &sync_meta.device_id,
        )
        .into_iter()
        .map(|snapshot| {
            EvergreenChatSyncAdapter::delete_change(
                EvergreenChatSyncAdapter::new_op_id(),
                &address,
                snapshot.relative,
                sync_meta.device_id.clone(),
                sync_meta.hlc,
            )
        })
        .collect(),
    })
}

fn strip_root_security_device_local(mut security: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = security.as_object_mut() {
        obj.remove("allow_no_auth_hosts");
    }
    security
}
