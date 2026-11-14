use std::sync::Arc;

use agent_types::{ToolCall, ToolDefinition};
use react_agent::toolkit::{
    AgentToolkit, DisclosureClass, MockToolExecutor, ToolDisclosureMeta, ToolSourceKind,
};
use rua_core::tool::ToolContext;

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} test tool"),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        }),
        strict: Some(true),
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

#[tokio::test]
async fn toolkit_preserves_registration_execution_export_and_internal_disclosure_metadata() {
    let mut toolkit = AgentToolkit::new();
    let mut definition = tool_definition("read");
    definition
        .metadata
        .insert("provider_hint".to_string(), serde_json::json!("preserved"));

    let disclosure = ToolDisclosureMeta {
        class: DisclosureClass::Direct,
        source_kind: ToolSourceKind::Builtin,
        source_name: Some("core".to_string()),
        category: Some("core".to_string()),
        schema_tokens: Some(42),
        original_name: None,
        product_capability: None,
    };

    toolkit.register_with_disclosure(
        definition.clone(),
        Arc::new(MockToolExecutor::new("read ok")),
        disclosure.clone(),
    );

    assert!(toolkit.has_tool("read"));
    assert!(!toolkit.has_tool("file_read"));
    assert!(!toolkit.has_tool("FILE_READ"));
    assert_eq!(toolkit.len(), 1);
    assert_eq!(toolkit.get_definition("read"), Some(&definition));
    assert_eq!(toolkit.get_definition("file_read"), None);
    assert_eq!(toolkit.canonical_name("read"), Some("read"));
    assert_eq!(toolkit.canonical_name("file_read"), None);
    assert_eq!(toolkit.disclosure_for("read"), Some(&disclosure));
    assert_eq!(toolkit.disclosure_for("file_read"), None);

    let exported = toolkit.export_definitions();
    assert_eq!(exported, vec![definition.clone()]);
    assert_eq!(
        exported[0].metadata.get("provider_hint"),
        Some(&serde_json::json!("preserved"))
    );
    assert!(!exported[0].metadata.contains_key("disclosure_class"));
    assert!(!exported[0].metadata.contains_key("source_kind"));

    let executor = toolkit
        .get_executor("read")
        .expect("canonical name resolves to executor");
    let result = executor
        .execute(
            &ToolCall::new("call-1", "read", serde_json::json!({ "path": "README.md" })),
            &ToolContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.call_id, "call-1");
    assert_eq!(result.name.as_deref(), Some("read"));
}
