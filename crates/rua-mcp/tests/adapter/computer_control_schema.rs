use mcp_manager::types::{McpServerId, McpToolInfo};
use rua_mcp::computer_control::interaction_tool;
use serde_json::{json, Value};

// The pinned Windows Cua 0.38 click schema omits `required`; the macOS
// schema contains an array. No native operation is needed to inspect tools/list.
fn native_tools() -> Vec<McpToolInfo> {
    [
        (
            "click",
            json!({"type":"object","additionalProperties":false,"properties":{
                "pid":{"type":"integer"},"window_id":{"type":"integer"},
                "scope":{"type":"string","enum":["window","desktop"]},
                "x":{"type":"number"},"y":{"type":"number"},
                "button":{"type":"string","enum":["left","right","middle"]}
            }}),
        ),
        (
            "type_text",
            json!({"type":"object","additionalProperties":false,"properties":{
                "pid":{"type":"integer"},"text":{"type":"string"}
            },"required":["pid","text"]}),
        ),
        (
            "press_key",
            json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
        ),
        (
            "scroll",
            json!({"type":"object","properties":{
                "direction":{"type":"string","enum":["up","down","left","right"]},
                "amount":{"type":"integer","minimum":1,"maximum":50}
            },"required":["direction"]}),
        ),
    ]
    .into_iter()
    .map(|(name, input_schema)| McpToolInfo {
        server_id: McpServerId::new("rua_computer"),
        name: name.into(),
        description: name.into(),
        raw: json!({"name":name,"inputSchema":input_schema}),
        input_schema,
    })
    .collect()
}

fn action_arguments(schema: &Value, name: &str) -> Value {
    schema["properties"]["actions"]["items"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["tool"]["enum"] == json!([name]))
        .unwrap()["properties"]["arguments"]
        .clone()
}

#[test]
fn windows_click_without_required_does_not_gain_a_null_schema_keyword() {
    let tools = native_tools();
    assert!(tools[0].input_schema.get("required").is_none());
    let interact = interaction_tool(&tools).unwrap();
    let click = action_arguments(&interact.input_schema, "click");
    assert!(
        click.get("required").is_none(),
        "a missing optional JSON Schema keyword must stay absent, not become null: {click}"
    );
}

#[test]
fn interaction_projection_preserves_native_constraints_and_hides_injected_target_fields() {
    let mut tools = native_tools();
    tools[0].input_schema["required"] = json!([]);
    let originals = tools.clone();
    let interact = interaction_tool(&tools).unwrap();
    let click = action_arguments(&interact.input_schema, "click");
    assert_eq!(click["required"], json!([]));
    assert_eq!(
        click["properties"]["button"]["enum"],
        json!(["left", "right", "middle"])
    );
    assert!(click["properties"].get("pid").is_none());
    assert!(click["properties"].get("window_id").is_none());
    assert!(click["properties"].get("scope").is_none());
    let text = action_arguments(&interact.input_schema, "type_text");
    assert_eq!(text["required"], json!(["text"]));
    let scroll = action_arguments(&interact.input_schema, "scroll");
    assert_eq!(scroll["properties"]["amount"]["maximum"], 50);
    assert_eq!(scroll["additionalProperties"], false);
    assert_eq!(interact.raw["inputSchema"], interact.input_schema);
    for (tool, original) in tools.iter().zip(&originals) {
        assert_eq!(tool.input_schema, original.input_schema);
    }
}
