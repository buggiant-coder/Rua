#[test]
fn settings_publish_root_config_facts_but_default_model_stays_local() {
    let settings_src = include_str!("../../src/endpoints/settings.rs");
    assert!(
        settings_src.contains("publish_sync_fact")
            && settings_src.contains("SyncPublicationFact::RootSettingsCommitted"),
        "settings update must publish RootSettingsCommitted through the Replication Publication Seam"
    );
    assert!(
        !settings_src.contains("sync_ops::emit_settings_v4_upsert"),
        "settings update must not call the legacy root_config/settings emit helper"
    );

    let models_src = include_str!("../../src/endpoints/models.rs");
    assert!(
        !models_src.contains("RootDefaultModelCommitted")
            && !models_src.contains("RootDefaultModelDeleted")
            && !models_src.contains("emit_default_model_v4"),
        "active model select/clear must not publish the device-local default model"
    );
}

#[test]
fn user_timezone_writes_publish_root_config_fact() {
    let root_src = include_str!("../../src/endpoints/config_misc.rs");
    assert!(
        root_src.contains("publish_sync_fact")
            && root_src.contains("SyncPublicationFact::RootUserTimezoneCommitted"),
        "root user-timezone update must publish RootUserTimezoneCommitted"
    );
    assert!(
        !root_src.contains("sync_ops::emit_user_timezone_v4_upsert"),
        "root user-timezone update must not call the legacy user-timezone emit helper"
    );

    let scoped_src = include_str!("../../src/endpoints/config.rs");
    assert!(
        scoped_src.contains("SyncPublicationFact::RootUserTimezoneCommitted"),
        "agent-scoped user-timezone update must publish RootUserTimezoneCommitted"
    );
    assert!(
        !scoped_src.contains("emit_user_timezone_v4_upsert"),
        "agent-scoped user-timezone update must not call the legacy user-timezone emit helper"
    );
}

#[test]
fn process_security_writes_publish_root_security_fact() {
    let security_src = include_str!("../../src/endpoints/security.rs");
    assert!(
        security_src.contains("publish_process_security_config")
            && security_src.contains("SyncPublicationFact::RootProcessSecurityCommitted"),
        "root security file-guard/skill-scanner/whitelist writes must publish RootProcessSecurityCommitted"
    );
    assert!(
        !security_src.contains("emit_security_config_v4_upsert"),
        "process security writes must not call the legacy root_config/security emit helper"
    );

    for name in [
        "update_file_guard",
        "update_skill_scanner",
        "add_whitelist",
        "remove_whitelist",
    ] {
        let body = security_src
            .split(&format!("async fn {name}"))
            .nth(1)
            .and_then(|tail| tail.split("async fn").next())
            .expect("root security mutation handler should exist");
        assert!(
            body.contains("publish_process_security_config"),
            "{name} must publish root_config/security after mutating process security config"
        );
    }

    let config_src = include_str!("../../src/endpoints/config.rs");
    assert!(
        config_src.contains("publish_process_security_config"),
        "agent-scoped aliases for process security must also publish root_config/security facts"
    );

    let catalog_src = include_str!("../../src/sync_domain_catalog.rs");
    assert!(
        catalog_src.contains("RootProcessSecurityCommitted")
            && catalog_src.contains("ROOT_CONFIG_RECORD_SECURITY")
            && catalog_src.contains("allow_no_auth_hosts"),
        "root_config/security publication must strip device-local allow_no_auth_hosts"
    );
}

#[test]
fn approval_decisions_emit_sync_v4_changes() {
    let approvals_src = include_str!("../../src/endpoints/approvals.rs");
    assert!(
        approvals_src.contains("publish_agent_security_config")
            && approvals_src.contains("AuthorityDecisionWire::AllowAgent"),
        "allow_agent approval decisions must publish agent_security facts so approval reuse rules sync"
    );
    assert!(
        !approvals_src.contains("emit_evergreen_chat_v4_snapshot")
            && !approvals_src.contains("emit_evergreen_chat_snapshot"),
        "approval decisions must not schedule a continuation or mutate chat history; terminal tool results emit evergreen_chat snapshots through the normal chat persistence path"
    );
}

#[test]
fn agent_scoped_behavior_writes_publish_agent_config_facts() {
    let src = include_str!("../../src/endpoints/config.rs");

    let heartbeat = src
        .split("async fn update_heartbeat")
        .nth(1)
        .and_then(|tail| tail.split("async fn").next())
        .expect("update_heartbeat handler should exist");
    assert!(
        heartbeat.contains("publish_agent_config_file_committed"),
        "heartbeat update must publish an agent_config fact"
    );

    let llm_routing = src
        .split("async fn update_llm_routing")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub(crate) async fn read_llm_routing_cells")
                .next()
        })
        .expect("update_llm_routing handler should exist");
    assert!(
        llm_routing.contains("publish_agent_config_file_committed"),
        "llm_routing update must publish an agent_config fact"
    );
}

#[test]
fn per_agent_mcp_mutations_publish_agent_config_facts() {
    let src = include_str!("../../src/endpoints/mcp.rs");

    assert!(
        src.contains("async fn publish_agent_config_after_mcp_write")
            && src.contains("publish_agent_config_file_committed")
            && src.contains("\"mcp agent config\""),
        "per-agent MCP writes must publish agent_config facts using the encrypted agent.json payload"
    );

    for name in [
        "set_mcp_tool_selection",
        "create_client",
        "update_client",
        "toggle_client",
        "delete_client",
        "register_client_compat",
        "delete_client_compat",
        "create_client_agent",
        "update_client_agent",
        "toggle_client_agent",
        "delete_client_agent",
    ] {
        let body = src
            .split(&format!("async fn {name}("))
            .nth(1)
            .and_then(|tail| tail.split("async fn").next())
            .expect("MCP mutation handler should exist");
        assert!(
            body.contains("publish_agent_config_after_mcp_write"),
            "{name} must publish an agent_config fact after mutating Agent configuration"
        );
    }
}

#[test]
fn per_agent_tool_mutations_publish_agent_config_facts() {
    let src = include_str!("../../src/endpoints/tools.rs");

    assert!(
        src.contains("async fn publish_agent_config_after_tools_write")
            && src.contains("publish_agent_config_file_committed")
            && src.contains("\"tools agent config\""),
        "per-agent tool writes must publish agent_config facts from agent.json"
    );

    for name in [
        "toggle_tool",
        "set_async_execution",
        "toggle_tool_agent",
        "set_async_execution_agent",
    ] {
        let body = src
            .split(&format!("async fn {name}"))
            .nth(1)
            .and_then(|tail| tail.split("async fn").next())
            .expect("tool mutation handler should exist");
        assert!(
            body.contains("publish_agent_config_after_tools_write"),
            "{name} must publish an agent_config fact after mutating agent tool config"
        );
    }
}

#[test]
fn agent_lifecycle_mutations_publish_agent_facts() {
    let src = include_str!("../../src/endpoints/agents.rs");

    for (name, expected) in [
        ("create_agent", "publish_agent_config_committed"),
        ("create_agent", "publish_agent_security_committed"),
        ("create_agent", "publish_agents_config"),
        ("update_agent", "publish_agent_config_committed"),
        ("delete_agent", "publish_agent_config_deleted"),
        ("delete_agent", "publish_agent_security_deleted"),
        ("delete_agent", "publish_agents_config"),
        ("save_agent_order", "publish_agents_config"),
        ("toggle_agent", "publish_agents_config"),
        ("rename_agent", "publish_agent_config_committed"),
        ("rename_agent", "publish_agents_config"),
    ] {
        let body = src
            .split(&format!("async fn {name}"))
            .nth(1)
            .and_then(|tail| tail.split("async fn").next())
            .expect("agent mutation handler should exist");
        assert!(
            body.contains(expected),
            "{name} must call {expected} so agent config/registry/security changes publish through the seam"
        );
    }
}

#[test]
fn agent_security_writes_publish_agent_security_facts() {
    let config_src = include_str!("../../src/endpoints/config.rs");
    assert!(
        config_src.contains("publish_agent_security_committed"),
        "agent-scoped config/security tool-guard updates must publish agent_security facts"
    );

    let security_src = include_str!("../../src/endpoints/security.rs");
    assert!(
        security_src.contains("publish_agent_security_committed"),
        "direct agent security endpoints must publish agent_security facts"
    );

    let agents_src = include_str!("../../src/endpoints/agents.rs");
    assert!(
        agents_src.contains("publish_agent_security_committed")
            && agents_src.contains("publish_agent_security_deleted"),
        "agent create/update/delete must publish agent_security facts for security.json"
    );
}

#[test]
fn subagent_template_writes_publish_subagent_template_fact() {
    let src = include_str!("../../src/endpoints/subagent_templates.rs");
    assert!(
        src.contains("publish_sync_fact")
            && src.contains("SyncPublicationFact::SubagentTemplatesCommitted"),
        "subagent template create/update/delete must publish SubagentTemplatesCommitted through the Replication Publication Seam"
    );
    assert!(
        !src.contains("sync_ops::emit_subagent_templates_v4_upsert"),
        "subagent template create/update/delete must not call the legacy root_config emit helper"
    );

    for name in ["create_template", "update_template", "delete_template"] {
        let body = src
            .split(&format!("async fn {name}"))
            .nth(1)
            .and_then(|tail| tail.split("async fn").next())
            .expect("subagent template handler should exist");
        assert!(
            body.contains("publish_subagent_templates_after_write"),
            "{name} must call the shared subagent-template publication helper"
        );
    }
}

#[test]
fn device_list_mutations_use_correct_trust_boundary() {
    let src = include_str!("../../src/endpoints/sync_devices.rs");
    let pairing_src = include_str!("../../src/endpoints/pairing.rs");
    let inbound_apply_src = include_str!("../../src/sync_inbound_apply.rs");

    assert!(
        pairing_src.contains(".write_device_list(device_list)")
            && pairing_src.contains("capture_full_export_v4_bootstrap")
            && pairing_src.contains("run_first_sync_cycle_to_all_device_list_peers")
            && !pairing_src.contains("SyncPublicationFact::DeviceListEntrySnapshot"),
        "pairing must persist Device List control state, then publish its frozen snapshot through the readiness-aware First Sync"
    );

    let remove = src
        .split("async fn remove_device")
        .nth(1)
        .and_then(|tail| tail.split("async fn rename_device").next())
        .expect("remove_device should exist");
    assert!(
        remove.contains("SyncPublicationFact::DeviceListDeviceRemoved"),
        "device remove must publish a device_list tombstone for explicit iroh-docs fanout"
    );
    assert!(
        !remove.contains("NetworkSecret::generate")
            && !remove.contains("deliver_network_rekey")
            && !remove.contains("NetworkRekeyCommand"),
        "device remove must not use custom rekey/control protocols"
    );
    assert!(
        inbound_apply_src.contains("DEVICE_LIST_DOMAIN")
            && inbound_apply_src.contains("reconcile_peer_directory"),
        "inbound Device List apply must reconcile and close removed Device Sessions"
    );

    let rename = src
        .split("async fn rename_device")
        .nth(1)
        .expect("rename_device should exist");
    assert!(
        rename.contains("SyncPublicationFact::DeviceListDeviceRenamed"),
        "device rename must publish a device_list rename fact"
    );
}
