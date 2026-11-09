pub fn strip_anyof_null(schema: &mut serde_json::Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    if let Some(any_of) = obj.remove("anyOf") {
        if let Some(branches) = any_of.as_array() {
            let non_null: Vec<&serde_json::Value> = branches
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) != Some("null"))
                .collect();
            if non_null.len() == 1 {
                if let Some(branch_obj) = non_null[0].as_object() {
                    for (k, v) in branch_obj {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            } else if !non_null.is_empty() {
                let mut cleaned: Vec<serde_json::Value> = non_null.into_iter().cloned().collect();
                for b in &mut cleaned {
                    strip_anyof_null(b);
                }
                obj.insert("anyOf".into(), serde_json::Value::Array(cleaned));
            }
        }
    }

    if let Some(props) = obj.get_mut("properties") {
        if let Some(props_obj) = props.as_object_mut() {
            for v in props_obj.values_mut() {
                strip_anyof_null(v);
            }
        }
    }

    if let Some(items) = obj.get_mut("items") {
        strip_anyof_null(items);
    }
}

pub(crate) fn is_valid_base64(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let stripped = s.trim_end_matches('=');
    if !stripped
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    {
        return false;
    }
    let pad = s.len() - stripped.len();
    pad <= 2 && s.len().is_multiple_of(4)
}
