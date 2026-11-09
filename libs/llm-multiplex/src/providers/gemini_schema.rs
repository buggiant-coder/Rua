use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SchemaType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
}

impl SchemaType {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "boolean" => Some(Self::Boolean),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
        }
    }

    fn matches(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Boolean => value.is_boolean(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
        }
    }
}

pub fn lower_tool_parameters(schema: &Value) -> Value {
    let lowered = lower_schema_node(schema)
        .filter(|value| schema_type(value) == Some(SchemaType::Object))
        .unwrap_or_else(empty_object_schema);

    if validate_tool_parameters(&lowered).is_ok() {
        lowered
    } else {
        empty_object_schema()
    }
}

pub fn validate_tool_parameters(schema: &Value) -> Result<(), String> {
    validate_schema_node(schema, "$")?;
    if schema_type(schema) != Some(SchemaType::Object) {
        return Err("$: tool parameters must have type object".into());
    }
    Ok(())
}

fn lower_schema_node(schema: &Value) -> Option<Value> {
    let source = schema.as_object()?;
    let nullable = source_nullable(source);

    if let Some(schema_type) = declared_type(source) {
        return lower_typed_schema(source, schema_type, nullable);
    }
    if source.get("properties").is_some_and(Value::is_object) {
        return lower_typed_schema(source, SchemaType::Object, nullable);
    }
    if source.contains_key("items") || source.contains_key("prefixItems") {
        return lower_typed_schema(source, SchemaType::Array, nullable);
    }
    if source.get("const").is_some_and(Value::is_string)
        || source
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(Value::is_string))
    {
        return lower_typed_schema(source, SchemaType::String, nullable);
    }

    for keyword in ["anyOf", "oneOf", "allOf"] {
        let Some(variants) = source.get(keyword).and_then(Value::as_array) else {
            continue;
        };
        for variant in variants.iter().filter(|variant| !is_null_schema(variant)) {
            let Some(mut lowered) = lower_schema_node(variant) else {
                continue;
            };
            let lowered_type = schema_type(&lowered)?;
            if let Some(target) = lowered.as_object_mut() {
                apply_type_constraints(source, target, lowered_type);
                apply_common_fields(source, target, nullable);
                copy_matching_value(source, target, "default", lowered_type, nullable);
                copy_matching_value(source, target, "example", lowered_type, nullable);
            }
            return Some(lowered);
        }
    }

    None
}

fn lower_typed_schema(
    source: &Map<String, Value>,
    schema_type: SchemaType,
    nullable: bool,
) -> Option<Value> {
    let mut target = Map::new();
    target.insert("type".into(), Value::String(schema_type.as_str().into()));

    match schema_type {
        SchemaType::Object => lower_object_fields(source, &mut target),
        SchemaType::Array => lower_array_fields(source, &mut target)?,
        SchemaType::String => lower_string_fields(source, &mut target),
        SchemaType::Number | SchemaType::Integer => {
            copy_number_field(source, &mut target, "minimum");
            copy_number_field(source, &mut target, "maximum");
            copy_supported_format(source, &mut target, schema_type);
        }
        SchemaType::Boolean => {}
    }

    apply_common_fields(source, &mut target, nullable);
    copy_matching_value(source, &mut target, "default", schema_type, nullable);
    copy_matching_value(source, &mut target, "example", schema_type, nullable);
    Some(Value::Object(target))
}

fn lower_object_fields(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let mut properties = Map::new();
    if let Some(source_properties) = source.get("properties").and_then(Value::as_object) {
        for (name, property_schema) in source_properties {
            if let Some(lowered) = lower_schema_node(property_schema) {
                properties.insert(name.clone(), lowered);
            }
        }
    }

    if let Some(required) = source.get("required").and_then(Value::as_array) {
        let required: Vec<_> = required
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| properties.contains_key(*name))
            .map(|name| Value::String(name.to_string()))
            .collect();
        if !required.is_empty() {
            target.insert("required".into(), Value::Array(required));
        }
    }

    if let Some(ordering) = source.get("propertyOrdering").and_then(Value::as_array) {
        let ordering: Vec<_> = ordering
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| properties.contains_key(*name))
            .map(|name| Value::String(name.to_string()))
            .collect();
        if !ordering.is_empty() {
            target.insert("propertyOrdering".into(), Value::Array(ordering));
        }
    }

    target.insert("properties".into(), Value::Object(properties));
    copy_int64_field(source, target, "minProperties");
    copy_int64_field(source, target, "maxProperties");
}

fn lower_array_fields(source: &Map<String, Value>, target: &mut Map<String, Value>) -> Option<()> {
    let item_source = source.get("items").or_else(|| {
        source
            .get("prefixItems")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
    })?;
    target.insert("items".into(), lower_schema_node(item_source)?);
    copy_int64_field(source, target, "minItems");
    copy_int64_field(source, target, "maxItems");
    Some(())
}

fn lower_string_fields(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let enum_values: Vec<_> = source
        .get("enum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|value| Value::String(value.to_string()))
        .collect();
    let enum_values = if enum_values.is_empty() {
        source
            .get("const")
            .and_then(Value::as_str)
            .map(|value| vec![Value::String(value.to_string())])
            .unwrap_or_default()
    } else {
        enum_values
    };
    if !enum_values.is_empty() {
        target.insert("enum".into(), Value::Array(enum_values));
        target.insert("format".into(), Value::String("enum".into()));
    } else {
        copy_supported_format(source, target, SchemaType::String);
    }

    copy_string_field(source, target, "pattern");
    copy_int64_field(source, target, "minLength");
    copy_int64_field(source, target, "maxLength");
}

fn apply_type_constraints(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    schema_type: SchemaType,
) {
    match schema_type {
        SchemaType::String => lower_string_fields(source, target),
        SchemaType::Number | SchemaType::Integer => {
            copy_number_field(source, target, "minimum");
            copy_number_field(source, target, "maximum");
            copy_supported_format(source, target, schema_type);
        }
        SchemaType::Array => {
            copy_int64_field(source, target, "minItems");
            copy_int64_field(source, target, "maxItems");
        }
        SchemaType::Object => {
            copy_int64_field(source, target, "minProperties");
            copy_int64_field(source, target, "maxProperties");
        }
        SchemaType::Boolean => {}
    }
}

fn apply_common_fields(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    nullable: bool,
) {
    copy_string_field(source, target, "title");
    copy_string_field(source, target, "description");
    if nullable || source.get("nullable").and_then(Value::as_bool) == Some(true) {
        target.insert("nullable".into(), Value::Bool(true));
    }
}

fn copy_supported_format(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    schema_type: SchemaType,
) {
    let Some(format) = source.get("format").and_then(Value::as_str) else {
        return;
    };
    let supported = match schema_type {
        SchemaType::String => matches!(format, "byte" | "date" | "date-time" | "duration" | "time"),
        SchemaType::Number => matches!(format, "float" | "double"),
        SchemaType::Integer => matches!(format, "int32" | "int64"),
        _ => false,
    };
    if supported {
        target.insert("format".into(), Value::String(format.to_string()));
    }
}

fn copy_string_field(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_str) {
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn copy_number_field(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).filter(|value| value.is_number()) {
        target.insert(key.to_string(), value.clone());
    }
}

fn copy_int64_field(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let encoded = source.get(key).and_then(|value| {
        value
            .as_u64()
            .map(|number| number.to_string())
            .or_else(|| {
                value
                    .as_i64()
                    .filter(|number| *number >= 0)
                    .map(|number| number.to_string())
            })
            .or_else(|| {
                value
                    .as_str()
                    .filter(|number| number.parse::<u64>().is_ok())
                    .map(str::to_string)
            })
    });
    if let Some(encoded) = encoded {
        target.insert(key.to_string(), Value::String(encoded));
    }
}

fn copy_matching_value(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    schema_type: SchemaType,
    nullable: bool,
) {
    if let Some(value) = source
        .get(key)
        .filter(|value| schema_type.matches(value) || nullable && value.is_null())
    {
        target.insert(key.to_string(), value.clone());
    }
}

fn declared_type(source: &Map<String, Value>) -> Option<SchemaType> {
    match source.get("type") {
        Some(Value::String(name)) => SchemaType::from_name(name),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find_map(SchemaType::from_name),
        _ => None,
    }
}

fn source_nullable(source: &Map<String, Value>) -> bool {
    if source.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if source
        .get("type")
        .and_then(Value::as_array)
        .is_some_and(|types| types.iter().any(|value| value.as_str() == Some("null")))
    {
        return true;
    }
    ["anyOf", "oneOf"]
        .iter()
        .filter_map(|keyword| source.get(*keyword).and_then(Value::as_array))
        .flatten()
        .any(is_null_schema)
}

fn is_null_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

fn schema_type(schema: &Value) -> Option<SchemaType> {
    schema
        .get("type")
        .and_then(Value::as_str)
        .and_then(SchemaType::from_name)
}

fn empty_object_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

fn validate_schema_node(schema: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;
    let schema_type =
        schema_type(schema).ok_or_else(|| format!("{path}: schema type is required"))?;

    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "type"
                | "title"
                | "description"
                | "nullable"
                | "default"
                | "example"
                | "format"
                | "enum"
                | "pattern"
                | "minLength"
                | "maxLength"
                | "minimum"
                | "maximum"
                | "items"
                | "minItems"
                | "maxItems"
                | "properties"
                | "required"
                | "propertyOrdering"
                | "minProperties"
                | "maxProperties"
        ) {
            return Err(format!("{path}: unsupported field {key}"));
        }
    }

    for key in ["title", "description", "format", "pattern"] {
        if object.get(key).is_some_and(|value| !value.is_string()) {
            return Err(format!("{path}.{key}: expected string"));
        }
    }
    if object
        .get("nullable")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(format!("{path}.nullable: expected boolean"));
    }
    for key in ["minimum", "maximum"] {
        if object.get(key).is_some_and(|value| !value.is_number()) {
            return Err(format!("{path}.{key}: expected number"));
        }
    }
    for key in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
    ] {
        if let Some(value) = object.get(key) {
            let valid = value
                .as_str()
                .is_some_and(|number| number.parse::<u64>().is_ok());
            if !valid {
                return Err(format!("{path}.{key}: expected int64 string"));
            }
        }
    }

    if let Some(values) = object.get("enum") {
        if schema_type != SchemaType::String {
            return Err(format!("{path}.enum: only string schemas support enum"));
        }
        let values = values
            .as_array()
            .ok_or_else(|| format!("{path}.enum: expected array"))?;
        if values.is_empty() || values.iter().any(|value| !value.is_string()) {
            return Err(format!("{path}.enum: expected non-empty string array"));
        }
    }

    match schema_type {
        SchemaType::Object => validate_object_schema(object, path)?,
        SchemaType::Array => {
            let items = object
                .get("items")
                .ok_or_else(|| format!("{path}.items: array schema requires items"))?;
            validate_schema_node(items, &format!("{path}.items"))?;
        }
        _ => {}
    }

    for key in ["default", "example"] {
        if let Some(value) = object.get(key) {
            let nullable = object.get("nullable").and_then(Value::as_bool) == Some(true);
            if !(schema_type.matches(value) || nullable && value.is_null()) {
                return Err(format!("{path}.{key}: value does not match schema type"));
            }
        }
    }

    Ok(())
}

fn validate_object_schema(object: &Map<String, Value>, path: &str) -> Result<(), String> {
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{path}.properties: object schema requires properties"))?;
    for (name, property_schema) in properties {
        validate_schema_node(property_schema, &format!("{path}.properties.{name}"))?;
    }

    for key in ["required", "propertyOrdering"] {
        let Some(values) = object.get(key) else {
            continue;
        };
        let values = values
            .as_array()
            .ok_or_else(|| format!("{path}.{key}: expected array"))?;
        for value in values {
            let name = value
                .as_str()
                .ok_or_else(|| format!("{path}.{key}: expected string entries"))?;
            if !properties.contains_key(name) {
                return Err(format!("{path}.{key}: unknown property {name}"));
            }
        }
    }

    Ok(())
}
