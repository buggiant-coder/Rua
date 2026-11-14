use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
use rua_tool::{AgentToolsCells, ToolConfigStore};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

fn setup(tools: Value) -> (TempDir, ToolConfigStore) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces/default/agent.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rua_config::atomic_write_json(&path, &json!({"id":"default","tools":tools})).unwrap();
    let cells = Arc::new(AgentToolsCells::new(
        Arc::new(cell_store::FileSystemCellStore::new()),
        tmp.path().join("workspaces"),
    ));
    (tmp, ToolConfigStore::new(cells))
}

#[tokio::test]
async fn missing_policy_is_invalid_and_reads_do_not_repair_configuration() {
    let (tmp, store) = setup(json!({"builtin_tools":{}}));
    let path = tmp.path().join("workspaces/default/agent.json");
    let bytes = std::fs::read(&path).unwrap();
    assert!(store.list_tools(None).await.is_err());
    assert!(store.selection_policy(None).await.is_err());
    assert!(store.set_enabled(None, "bash", true).await.is_err());
    assert!(store
        .preview_selection_policy(None, ToolSelectionPolicy::default_enabled())
        .await
        .is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn policy_edit_preserves_settings_and_checks_stale_revision() {
    let (tmp, store) = setup(json!({
        "selection_policy":ToolSelectionPolicy::default_enabled(),"future":"keep",
        "builtin_tools":{"edit":{"name":"edit","config":{"api_key":"private-secret"}}}
    }));
    let preview = store
        .preview_selection_policy(
            None,
            ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("read")]),
        )
        .await
        .unwrap();
    assert_eq!(preview.current, ToolSelectionPolicy::default_enabled());
    assert!(!serde_json::to_string(&preview)
        .unwrap()
        .contains("private-secret"));
    store
        .set_selection_policy(None, preview.expected.clone(), preview.candidate.clone())
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_selection_policy(None, preview.expected, preview.candidate)
            .await,
        Err(rua_tool::ToolConfigError::Conflict(_))
    ));
    let body: Value = serde_json::from_slice(
        &std::fs::read(tmp.path().join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["tools"]["future"], "keep");
    assert_eq!(
        body["tools"]["builtin_tools"]["edit"]["config"]["api_key"],
        "private-secret"
    );
}

#[tokio::test]
async fn default_enabled_toggle_persists_an_absent_catalog_tool() {
    let (tmp, store) = setup(
        json!({"selection_policy":ToolSelectionPolicy::default_enabled(),"builtin_tools":{}}),
    );
    assert!(store
        .list_tools(None)
        .await
        .unwrap()
        .iter()
        .all(|tool| tool.enabled));
    assert!(
        !store
            .set_enabled(None, "bash", false)
            .await
            .unwrap()
            .enabled
    );
    let policy = store.selection_policy(None).await.unwrap();
    assert!(!policy.selects(&ToolIdentity::builtin("bash"), None));
    assert!(store.set_enabled(None, "bash", true).await.unwrap().enabled);
    let body: Value = serde_json::from_slice(
        &std::fs::read(tmp.path().join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    assert!(body["tools"]["builtin_tools"]["bash"]
        .get("enabled")
        .is_none());
}

#[tokio::test]
async fn restricted_toggle_retains_exact_selection_and_unknown_fields() {
    let (_tmp, store) = setup(
        json!({"selection_policy":{"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"read","future":1}],"future":true},"builtin_tools":{}}),
    );
    assert_eq!(
        store
            .list_tools(None)
            .await
            .unwrap()
            .iter()
            .filter(|tool| tool.enabled)
            .count(),
        1
    );
    assert!(store.set_enabled(None, "bash", true).await.unwrap().enabled);
    let policy = store.selection_policy(None).await.unwrap();
    assert!(!policy.selects(&ToolIdentity::builtin("new_product"), None));
    let value = serde_json::to_value(policy).unwrap();
    assert_eq!(value["future"], true);
    assert_eq!(value["allowed_tools"][0]["future"], 1);
}

#[tokio::test]
async fn invalid_selection_policy_does_not_fall_back_to_default_enabled() {
    let (_tmp, store) =
        setup(json!({"selection_policy":{"mode":"default_enabled"},"builtin_tools":{}}));
    assert!(store.selection_policy(None).await.is_err());
    assert!(store.list_tools(None).await.is_err());
}

#[tokio::test]
async fn policy_edits_retain_off_device_identities() {
    let policy = ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("off_device")]);
    let (_tmp, store) = setup(json!({"selection_policy":policy,"builtin_tools":{}}));
    let preview = store
        .preview_selection_policy(None, policy.clone())
        .await
        .unwrap();
    assert_eq!(preview.candidate, policy);
    assert!(preview
        .candidate
        .selects(&ToolIdentity::builtin("off_device"), None));
    assert!(!preview
        .candidate
        .selects(&ToolIdentity::builtin("new_product"), None));
}

#[test]
fn product_capability_membership_requires_trusted_exact_registration() {
    use rua_tool::registry::builtin_product_capability;
    let mut spec = rua_tool::builtin::phone_use::PhoneStatusTool::spec();
    let catalog = rua_tool::tool_catalog_from_specs([&spec]);
    assert_eq!(catalog["phone_status"]["capability"], "phone_use");
    spec.namespace = "external".into();
    let catalog = rua_tool::tool_catalog_from_specs([&spec]);
    assert!(catalog["phone_status"].get("capability").is_none());
    assert_eq!(builtin_product_capability("phone_new_untrusted"), None);
}

#[tokio::test]
async fn capability_metadata_comes_from_catalog_not_saved_behavior() {
    let (_tmp, store) = setup(json!({
        "selection_policy":{"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":["phone_use"]},
        "builtin_tools":{"read":{"name":"read","capability":"phone_use"}}
    }));
    let read = store
        .list_tools(None)
        .await
        .unwrap()
        .into_iter()
        .find(|tool| tool.name == "read")
        .unwrap();
    assert!(read.enabled);
    assert!(!read.extra.contains_key("capability"));
}

#[tokio::test]
async fn typed_toggle_changes_only_selection_policy() {
    let (tmp, store) = setup(json!({
        "selection_policy":ToolSelectionPolicy::default_enabled(),
        "builtin_tools":{"read":{"name":"read","config":{"keep":true}}},
        "external_sources":{"enrollment":"keep"}
    }));
    let identity = ToolIdentity::mcp("external", "read");
    let policy = store
        .set_identity_enabled(None, identity.clone(), false)
        .await
        .unwrap();
    assert!(!policy.selects(&identity, None));
    assert!(policy.selects(&ToolIdentity::builtin("read"), None));
    let body: Value = serde_json::from_slice(
        &std::fs::read(tmp.path().join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(body["tools"]["external_sources"]["enrollment"], "keep");
    assert_eq!(
        body["tools"]["builtin_tools"]["read"]["config"]["keep"],
        true
    );
    assert!(store
        .set_identity_enabled(None, identity.clone(), true)
        .await
        .unwrap()
        .selects(&identity, None));
}

#[tokio::test]
async fn typed_toggle_requires_valid_identity_and_explicit_policy() {
    let (tmp, store) = setup(json!({"builtin_tools":{}}));
    let path = tmp.path().join("workspaces/default/agent.json");
    let bytes = std::fs::read(&path).unwrap();
    assert!(store
        .set_identity_enabled(None, ToolIdentity::mcp("external", "read"), true)
        .await
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(store
        .set_identity_enabled(None, ToolIdentity::mcp("", "read"), true)
        .await
        .is_err());
}

#[tokio::test]
async fn source_changes_after_preview_reject_policy_edit() {
    let (tmp, store) = setup(
        json!({"selection_policy":ToolSelectionPolicy::default_enabled(),"builtin_tools":{}}),
    );
    let preview = store
        .preview_selection_policy(None, ToolSelectionPolicy::restricted(vec![]))
        .await
        .unwrap();
    let path = tmp.path().join("workspaces/default/agent.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["mcp"] = json!({"clients":{"external":{"enabled":false}}});
    rua_config::atomic_write_json(&path, &document).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(
        store
            .set_selection_policy(None, preview.expected, preview.candidate)
            .await,
        Err(rua_tool::ToolConfigError::Conflict(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        store.selection_policy(None).await.unwrap(),
        ToolSelectionPolicy::default_enabled()
    );
}

#[tokio::test]
async fn capability_disabled_catalog_explains_why_individual_tools_are_unavailable() {
    let (_tmp, store) = setup(
        json!({"selection_policy":{"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":["phone_use"]},"builtin_tools":{}}),
    );
    let spec = rua_tool::builtin::phone_use::PhoneStatusTool::spec();
    let store = store.with_catalog(rua_tool::tool_catalog_from_specs([&spec]));
    let tools = store.list_tools(None).await.unwrap();
    assert!(!tools[0].enabled);
    assert_eq!(tools[0].extra["capability_enabled"], false);
}

#[tokio::test]
async fn selection_policy_apply_does_not_create_missing_agent_configuration() {
    let (tmp, store) = setup(json!({"builtin_tools":{}}));
    assert!(matches!(
        store
            .set_selection_policy(
                Some("missing"),
                None,
                ToolSelectionPolicy::default_enabled()
            )
            .await,
        Err(rua_tool::ToolConfigError::NotFound(_))
    ));
    assert!(!tmp.path().join("workspaces/missing").exists());
}
