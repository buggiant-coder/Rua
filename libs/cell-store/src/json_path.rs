use serde_json::{Map, Value};

use crate::error::CellStoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    Field(String),
    Index(usize),
}

pub(crate) fn parse(path: &str) -> Result<Vec<Segment>, CellStoreError> {
    if !path.starts_with('$') {
        return Err(CellStoreError::InvalidJsonPath(path.to_string()));
    }
    let rest = &path[1..];
    let mut segments = Vec::new();
    let mut chars = rest.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '.' => {
                chars.next();
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '.' || c == '[' {
                        break;
                    }
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        name.push(c);
                        chars.next();
                    } else {
                        return Err(CellStoreError::InvalidJsonPath(path.to_string()));
                    }
                }
                if name.is_empty() {
                    return Err(CellStoreError::InvalidJsonPath(path.to_string()));
                }
                segments.push(Segment::Field(name));
            }
            '[' => {
                chars.next();
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ']' {
                        break;
                    }
                    if c.is_ascii_digit() {
                        num.push(c);
                        chars.next();
                    } else {
                        return Err(CellStoreError::InvalidJsonPath(path.to_string()));
                    }
                }
                if chars.next() != Some(']') {
                    return Err(CellStoreError::InvalidJsonPath(path.to_string()));
                }
                if num.is_empty() {
                    return Err(CellStoreError::InvalidJsonPath(path.to_string()));
                }
                let idx: usize = num
                    .parse()
                    .map_err(|_| CellStoreError::InvalidJsonPath(path.to_string()))?;
                segments.push(Segment::Index(idx));
            }
            _ => return Err(CellStoreError::InvalidJsonPath(path.to_string())),
        }
    }
    Ok(segments)
}

pub(crate) fn get<'a>(root: &'a Value, segments: &[Segment]) -> Option<&'a Value> {
    let mut current = root;
    for seg in segments {
        current = match seg {
            Segment::Field(name) => current.as_object()?.get(name)?,
            Segment::Index(i) => current.as_array()?.get(*i)?,
        };
    }
    Some(current)
}

pub(crate) fn set(
    root: &mut Value,
    segments: &[Segment],
    new: Value,
    raw_path: &str,
) -> Result<(), CellStoreError> {
    if segments.is_empty() {
        *root = new;
        return Ok(());
    }
    if root.is_null() {
        *root = default_for(&segments[0]);
    }
    let mut current = root;
    for (i, seg) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        match seg {
            Segment::Field(name) => {
                let current_kind = kind(current);
                let obj = current.as_object_mut().ok_or_else(|| {
                    CellStoreError::InvalidJsonPath(format!(
                        "{raw_path}: expected object at segment {i}, got {current_kind}"
                    ))
                })?;
                if is_last {
                    obj.insert(name.clone(), new);
                    return Ok(());
                }
                let next_default = default_for(&segments[i + 1]);
                let entry = obj.entry(name.clone()).or_insert(next_default);
                if entry.is_null() {
                    *entry = default_for(&segments[i + 1]);
                }
                current = entry;
            }
            Segment::Index(idx) => {
                let current_kind = kind(current);
                let arr = current.as_array_mut().ok_or_else(|| {
                    CellStoreError::InvalidJsonPath(format!(
                        "{raw_path}: expected array at segment {i}, got {current_kind}"
                    ))
                })?;
                while arr.len() <= *idx {
                    arr.push(Value::Null);
                }
                if is_last {
                    arr[*idx] = new;
                    return Ok(());
                }
                if arr[*idx].is_null() {
                    arr[*idx] = default_for(&segments[i + 1]);
                }
                current = &mut arr[*idx];
            }
        }
    }
    Ok(())
}

pub(crate) fn remove(root: &mut Value, segments: &[Segment]) -> bool {
    if segments.is_empty() {
        *root = Value::Null;
        return true;
    }
    let mut current = root;
    for seg in &segments[..segments.len() - 1] {
        current = match seg {
            Segment::Field(name) => match current.as_object_mut().and_then(|m| m.get_mut(name)) {
                Some(v) => v,
                None => return false,
            },
            Segment::Index(i) => match current.as_array_mut().and_then(|a| a.get_mut(*i)) {
                Some(v) => v,
                None => return false,
            },
        };
    }
    match segments.last().unwrap() {
        Segment::Field(name) => current
            .as_object_mut()
            .and_then(|m| m.remove(name))
            .is_some(),
        Segment::Index(i) => {
            if let Some(arr) = current.as_array_mut() {
                if *i < arr.len() {
                    arr.remove(*i);
                    return true;
                }
            }
            false
        }
    }
}

fn default_for(seg: &Segment) -> Value {
    match seg {
        Segment::Field(_) => Value::Object(Map::new()),
        Segment::Index(_) => Value::Array(Vec::new()),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
