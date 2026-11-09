use agent_types::{ChatRequest, ModelDialogueItem, Role, ToolAnnotations, ToolDefinition};
use llm_multiplex::providers::gemini::{
    lower_gemini_tool_parameters, validate_gemini_tool_parameters, GeminiConfig, GeminiProvider,
};
use serde_json::{json, Value};

fn gemini_provider() -> GeminiProvider {
    GeminiProvider::new(GeminiConfig {
        provider_id: "gemini".into(),
        provider_name: "Gemini".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        api_key: None,
        headers: serde_json::Map::new(),
        models: Vec::new(),
        timeout_secs: 30,
        generate_kwargs: serde_json::Map::new(),
    })
    .expect("gemini provider")
}

#[test]
fn gemini_schema_lowering_emits_only_supported_typed_nodes() {
    let schema = json!({
        "type": "object",
        "required": true,
        "additionalProperties": {"type": "string"},
        "properties": {
            "query": {
                "anyOf": [{"type": "string"}, {"type": "null"}],
                "description": "optional query",
                "minLength": 2
            },
            "limit": {
                "oneOf": [{"type": "integer"}, {"type": "null"}],
                "default": 10
            },
            "options": {
                "type": "object",
                "additionalProperties": true,
                "maxProperties": 3,
                "properties": {
                    "enabled": true,
                    "disabled": false
                }
            }
        }
    });

    let sanitized = lower_gemini_tool_parameters(&schema);
    let encoded = serde_json::to_string(&sanitized).expect("json encodes");
    validate_gemini_tool_parameters(&sanitized).expect("valid Gemini tool parameters");

    assert!(!encoded.contains("additionalProperties"));
    assert_eq!(sanitized["required"], Value::Null);
    assert_eq!(sanitized["properties"]["query"]["type"], "string");
    assert_eq!(sanitized["properties"]["query"]["nullable"], true);
    assert_eq!(
        sanitized["properties"]["query"]["description"],
        "optional query"
    );
    assert_eq!(sanitized["properties"]["query"]["minLength"], "2");
    assert_eq!(sanitized["properties"]["limit"]["type"], "integer");
    assert_eq!(sanitized["properties"]["limit"]["nullable"], true);
    assert_eq!(sanitized["properties"]["limit"]["default"], 10);
    assert_eq!(sanitized["properties"]["options"]["properties"], json!({}));
    assert_eq!(sanitized["properties"]["options"]["maxProperties"], "3");
}

#[test]
fn cross_field_constraints_lower_to_valid_gemini_parameters() {
    let schema = json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "text": {"type": "string"},
            "close_stdin": {
                "type": "boolean",
                "const": true
            },
            "transport": {
                "type": "string",
                "const": "stdin"
            }
        },
        "required": ["task_id"],
        "oneOf": [
            {
                "required": ["text"],
                "not": {"required": ["close_stdin"]}
            },
            {
                "required": ["close_stdin"],
                "not": {"required": ["text"]}
            }
        ],
        "additionalProperties": false
    });

    let sanitized = lower_gemini_tool_parameters(&schema);
    let encoded = serde_json::to_string(&sanitized).expect("json encodes");
    validate_gemini_tool_parameters(&sanitized).expect("valid Gemini tool parameters");

    assert!(!encoded.contains("\"const\""));
    assert!(!encoded.contains("\"oneOf\""));
    assert!(!encoded.contains("\"not\""));
    assert!(!encoded.contains("additionalProperties"));
    assert_eq!(sanitized["properties"]["close_stdin"]["type"], "boolean");
    assert_eq!(
        sanitized["properties"]["close_stdin"]["enum"],
        Value::Null,
        "Gemini only accepts enum values for string schemas"
    );
    assert_eq!(
        sanitized["properties"]["transport"]["enum"],
        json!(["stdin"])
    );
    assert!(
        sanitized.get("anyOf").is_none(),
        "Gemini rejects union variants that do not declare a schema type"
    );

    let tool = ToolDefinition {
        name: "constraint_tool".into(),
        description: "Exercise unsupported JSON Schema constraints".into(),
        parameters: schema,
        strict: None,
        annotations: ToolAnnotations::default(),
        metadata: serde_json::Map::new(),
    };
    let request = ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "send input")])
        .with_tools(vec![tool]);
    let body = gemini_provider().build_request_body(&request, "gemini-test");
    let parameters = &body["tools"][0]["functionDeclarations"][0]["parameters"];

    assert_eq!(parameters["type"], "object");
    assert!(parameters.get("anyOf").is_none());
    validate_gemini_tool_parameters(parameters).expect("valid Gemini tool parameters");
}

#[test]
fn gemini_request_body_drops_non_string_tool_enums() {
    let tool = ToolDefinition {
        name: "set_flags".into(),
        description: "Set boolean flags".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "enabled": {
                    "type": "boolean",
                    "enum": [true, false]
                },
                "mode": {
                    "type": "string",
                    "enum": ["safe", "fast"]
                }
            }
        }),
        strict: None,
        annotations: ToolAnnotations::default(),
        metadata: serde_json::Map::new(),
    };
    let request = ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "set flags")])
        .with_tools(vec![tool]);

    let body = gemini_provider().build_request_body(&request, "gemini-test");
    let properties = &body["tools"][0]["functionDeclarations"][0]["parameters"]["properties"];

    assert_eq!(properties["enabled"]["type"], "boolean");
    assert_eq!(properties["enabled"]["enum"], Value::Null);
    assert_eq!(properties["mode"]["enum"], json!(["safe", "fast"]));
    validate_gemini_tool_parameters(&body["tools"][0]["functionDeclarations"][0]["parameters"])
        .expect("valid Gemini tool parameters");
}

#[test]
fn gemini_schema_lowering_omits_unrepresentable_nested_schemas() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": false,
                    "additionalProperties": {"type": "string"},
                    "properties": {
                        "name": true,
                        "deleted": false
                    }
                },
                "prefixItems": [
                    {"type": "string", "additionalProperties": true},
                    false
                ]
            },
            "guarded": {
                "not": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": {
                        "reason": true
                    }
                }
            },
            "dependent": {
                "dependencies": {
                    "mode": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "legacy": false
                }
            }
        }
    });

    let sanitized = lower_gemini_tool_parameters(&schema);
    let encoded = serde_json::to_string(&sanitized).expect("json encodes");
    validate_gemini_tool_parameters(&sanitized).expect("valid Gemini tool parameters");

    assert!(
        !encoded.contains("additionalProperties"),
        "Gemini rejects additionalProperties anywhere in tool parameter schemas"
    );
    assert_eq!(
        sanitized["properties"]["items"]["items"]["required"],
        Value::Null,
        "malformed boolean required must be removed even when nested under items"
    );
    assert_eq!(
        sanitized["properties"]["items"]["items"]["properties"],
        json!({})
    );
    assert!(sanitized["properties"]["items"]
        .get("prefixItems")
        .is_none());
    assert!(sanitized["properties"].get("guarded").is_none());
    assert!(sanitized["properties"].get("dependent").is_none());
}

#[test]
fn gemini_schema_lowering_selects_a_typed_nullable_union_variant() {
    let schema = json!({
        "type": "object",
        "properties": {
            "selector": {
                "description": "string or integer selector",
                "default": "latest",
                "oneOf": [
                    {"type": "string", "additionalProperties": true},
                    {"type": "integer"},
                    {"type": "null"}
                ]
            }
        }
    });

    let sanitized = lower_gemini_tool_parameters(&schema);
    let selector = &sanitized["properties"]["selector"];
    let encoded = serde_json::to_string(selector).expect("json encodes");
    validate_gemini_tool_parameters(&sanitized).expect("valid Gemini tool parameters");

    assert!(!encoded.contains("additionalProperties"));
    assert_eq!(selector["type"], "string");
    assert_eq!(selector["nullable"], true);
    assert_eq!(selector["description"], "string or integer selector");
    assert_eq!(selector["default"], "latest");
    assert!(selector.get("oneOf").is_none());
    assert!(selector.get("anyOf").is_none());
}

#[test]
fn gemini_schema_lowering_selects_the_first_supported_declared_type() {
    let schema = json!({
        "type": "object",
        "properties": {
            "optional_name": {
                "type": ["string", "null"],
                "description": "optional name"
            },
            "arbitrary_value": {
                "type": ["string", "number", "boolean", "object", "array", "null"]
            }
        }
    });

    let sanitized = lower_gemini_tool_parameters(&schema);
    let optional_name = &sanitized["properties"]["optional_name"];
    let arbitrary_value = &sanitized["properties"]["arbitrary_value"];
    validate_gemini_tool_parameters(&sanitized).expect("valid Gemini tool parameters");

    assert_eq!(optional_name["type"], "string");
    assert_eq!(optional_name["nullable"], true);
    assert_eq!(optional_name["description"], "optional name");
    assert_eq!(arbitrary_value["type"], "string");
    assert_eq!(arbitrary_value["nullable"], true);
}

#[test]
fn gemini_schema_validator_rejects_invalid_provider_shapes() {
    let untyped = json!({"description": "missing type"});
    let boolean_enum = json!({"type": "boolean", "enum": [true]});

    assert!(validate_gemini_tool_parameters(&untyped).is_err());
    assert!(validate_gemini_tool_parameters(&boolean_enum).is_err());
}

#[test]
fn gemini_request_body_uses_lowered_tool_parameters_without_mutating_request() {
    let tool = ToolDefinition {
        name: "search".into(),
        description: "Search with optional filters".into(),
        parameters: json!({
            "type": "object",
            "$defs": {
                "Filter": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "properties": {
                        "owner": {
                            "anyOf": [{"type": "string"}, {"type": "null"}]
                        }
                    }
                }
            },
            "properties": {
                "filter": {"$ref": "#/$defs/Filter"}
            }
        }),
        strict: None,
        annotations: ToolAnnotations::default(),
        metadata: serde_json::Map::new(),
    };
    let request = ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "search docs")])
        .with_tools(vec![tool]);
    let original_tools = request.tools.clone();

    let body = gemini_provider().build_request_body(&request, "gemini-test");
    let parameters = &body["tools"][0]["functionDeclarations"][0]["parameters"];
    let encoded = serde_json::to_string(parameters).expect("json encodes");

    assert!(!encoded.contains("$defs"));
    assert!(!encoded.contains("$ref"));
    assert!(!encoded.contains("additionalProperties"));
    assert_eq!(
        parameters["properties"]["filter"]["properties"]["owner"]["type"],
        "string"
    );
    assert_eq!(
        parameters["properties"]["filter"]["properties"]["owner"]["nullable"],
        true
    );
    validate_gemini_tool_parameters(parameters).expect("valid Gemini tool parameters");
    assert_eq!(request.tools, original_tools);
}
