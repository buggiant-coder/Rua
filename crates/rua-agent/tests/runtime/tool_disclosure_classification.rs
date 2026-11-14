use std::sync::Arc;

use async_trait::async_trait;
use react_agent::skill_tool::{register_skill_pseudo_tool, SkillInfo};
use react_agent::toolkit::{DisclosureClass, ToolDisclosureMeta, ToolSourceKind};
use react_agent::ToolDisclosureMode;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_plugin_runtime::{PluginLoader, PluginRegistry, ToolSpec};

struct FakeMcpTool;

#[async_trait]
impl rua_contract::Tool for FakeMcpTool {
    fn name(&self) -> &str {
        "fake_mcp"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fake_mcp".into(),
            description: "Fake MCP tool".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        Ok(ToolResult::text("fake", "ok"))
    }
}

fn assert_meta(
    meta: &ToolDisclosureMeta,
    class: DisclosureClass,
    source_kind: ToolSourceKind,
    category: &str,
) {
    assert_eq!(meta.class, class);
    assert_eq!(meta.source_kind, source_kind);
    assert_eq!(meta.category.as_deref(), Some(category));
}

#[test]
fn default_toolkit_classifies_core_tools_as_direct_and_complex_bundled_tools_as_deferred() {
    let toolkit = rua_agent::toolkit::build_core_toolkit_with_recall(
        rua_tool::default_command_executor(),
        rua_tool::ToolProfile::DesktopFull,
    );

    for name in ["bash", "read", "edit", "grep", "glob"] {
        assert_meta(
            toolkit
                .disclosure_for(name)
                .unwrap_or_else(|| panic!("{name} should be registered")),
            DisclosureClass::Direct,
            ToolSourceKind::Builtin,
            "core",
        );
    }

    for name in ["python", "task_create", "task_list", "task_read"] {
        assert_meta(
            toolkit
                .disclosure_for(name)
                .unwrap_or_else(|| panic!("{name} should be registered")),
            DisclosureClass::DeferredCandidate,
            ToolSourceKind::Builtin,
            "bundled",
        );
    }
}

#[test]
fn skill_pseudo_tool_is_direct_skill_disclosure() {
    let mut toolkit = react_agent::AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![SkillInfo {
            name: "diagnose".into(),
            description: "Debug failures".into(),
            body: "Use disciplined diagnosis.".into(),
            source: react_agent::SkillRuntimeSource::Agent,
            base_dir: std::env::current_dir().unwrap(),
        }],
    )
    .unwrap();

    assert_meta(
        toolkit
            .disclosure_for("skill")
            .expect("skill pseudo-tool should be registered"),
        DisclosureClass::Direct,
        ToolSourceKind::Skill,
        "skill",
    );
}

#[tokio::test]
async fn mcp_and_plugin_tools_are_deferred_external_tools() {
    let mut mcp_toolkit = react_agent::AgentToolkit::new();
    rua_agent::toolkit::register_mcp_tools(&mut mcp_toolkit, vec![Arc::new(FakeMcpTool)]);

    assert_meta(
        mcp_toolkit
            .disclosure_for("fake_mcp")
            .expect("MCP tool should be registered"),
        DisclosureClass::DeferredCandidate,
        ToolSourceKind::Mcp,
        "external",
    );

    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("test-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"id":"test-plugin","name":"Test Plugin","version":"1.0.0"}"#,
    )
    .unwrap();

    let registry = Arc::new(PluginRegistry::new());
    registry
        .add(PluginLoader::load(&plugin_dir).expect("plugin load"))
        .expect("plugin add");
    registry
        .register_tool(
            "test-plugin",
            ToolSpec {
                name: "lookup".into(),
                description: "External lookup".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        )
        .expect("register plugin tool");

    let mut plugin_toolkit = react_agent::AgentToolkit::new();
    rua_agent::plugin_tool_handler::populate_plugin_tools(&mut plugin_toolkit, &registry);

    assert_meta(
        plugin_toolkit
            .disclosure_for("plugin__test-plugin__lookup")
            .expect("plugin tool should be registered"),
        DisclosureClass::DeferredCandidate,
        ToolSourceKind::Plugin,
        "external",
    );
}

#[test]
fn running_config_resolves_to_react_tool_disclosure_config() {
    let running: rua_config::AgentsRunningConfig = serde_json::from_value(serde_json::json!({
        "tool_disclosure": {
            "enabled": "off",
            "threshold_pct": 12.5,
            "search_default_limit": 2,
            "search_max_limit": 8
        }
    }))
    .unwrap();

    let config = rua_agent::tool_disclosure::react_tool_disclosure_config(&running);

    assert_eq!(config.enabled, ToolDisclosureMode::Off);
    assert_eq!(config.threshold_pct, 12.5);
    assert_eq!(config.search_default_limit, 2);
    assert_eq!(config.search_max_limit, 8);
}

#[test]
fn phone_readiness_keeps_status_and_separate_permissions() {
    let mut status = rua_core::phone_use::PhoneStatus {
        generation: "test".into(),
        platform: "android".into(),
        state: "ready".into(),
        accessibility: false,
        camera_permission: false,
        extra: Default::default(),
    };
    let reason = rua_agent::tool_disclosure::phone_tool_unavailability;
    assert!(reason("phone_status", &status).is_none());
    assert!(reason("phone_list_apps", &status).is_none());
    assert!(reason("phone_list_cameras", &status).is_none());
    assert!(reason("phone_observe", &status).is_some());
    assert!(reason("phone_take_photo", &status).is_some());
    status.camera_permission = true;
    assert!(reason("phone_observe", &status).is_some());
    assert!(reason("phone_take_photo", &status).is_none());
    status.accessibility = true;
    assert!(reason("phone_observe", &status).is_none());
}

#[test]
fn registry_namespace_cannot_claim_product_ownership_through_a_tool_name() {
    let mut registry = rua_tool::ToolRegistry::new();
    registry
        .register(
            Arc::new(rua_tool::builtin::phone_use::PhoneStatusTool),
            rua_tool::builtin::phone_use::PhoneStatusTool::spec(),
            rua_tool::ToolNamespace::Mcp,
        )
        .unwrap();
    let mut toolkit = react_agent::AgentToolkit::new();
    rua_agent::tool_disclosure::populate_toolkit_from_registry(&mut toolkit, &registry);
    let meta = toolkit.disclosure_for("phone_status").unwrap();
    assert_eq!(meta.source_kind, ToolSourceKind::Unknown);
    assert!(meta.product_capability.is_none());
}
