use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::migration::atomic::atomic_write_json;
use crate::types::templates::SubagentTemplatesFile;

pub struct SubagentTemplateStore {
    rua_root: PathBuf,
}

impl SubagentTemplateStore {
    pub fn new(rua_root: PathBuf) -> Self {
        Self { rua_root }
    }

    fn templates_path(&self) -> PathBuf {
        self.rua_root.join("subagent_templates.json")
    }

    pub fn load_file(&self) -> std::io::Result<SubagentTemplatesFile> {
        let path = self.templates_path();
        match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Ok(SubagentTemplatesFile::default()),
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(SubagentTemplatesFile::default())
            }
            Err(e) => Err(e),
        }
    }

    fn save_file(&self, file: &SubagentTemplatesFile) -> std::io::Result<()> {
        let value = serde_json::to_value(file).map_err(std::io::Error::other)?;
        atomic_write_json(&self.templates_path(), &value)
    }

    pub fn list(&self) -> std::io::Result<Vec<Value>> {
        Ok(self.load_file()?.templates)
    }

    pub fn get(&self, template_id: &str) -> std::io::Result<Option<Value>> {
        Ok(self
            .load_file()?
            .templates
            .into_iter()
            .find(|t| extract_id(t) == Some(template_id)))
    }

    pub fn create(&self, template: Value) -> std::io::Result<bool> {
        let id = extract_id(&template)
            .map(|s| s.to_string())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "subagent template missing required `id` field",
                )
            })?;
        let mut file = self.load_file()?;
        if file
            .templates
            .iter()
            .any(|t| extract_id(t) == Some(id.as_str()))
        {
            return Ok(false);
        }
        file.templates.push(template);
        self.save_file(&file)?;
        Ok(true)
    }

    pub fn update(&self, template_id: &str, template: Value) -> std::io::Result<bool> {
        let mut file = self.load_file()?;
        let mut found = false;
        for entry in file.templates.iter_mut() {
            if extract_id(entry) == Some(template_id) {
                *entry = ensure_id(template, template_id);
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
        self.save_file(&file)?;
        Ok(true)
    }

    pub fn delete(&self, template_id: &str) -> std::io::Result<bool> {
        let mut file = self.load_file()?;
        let len_before = file.templates.len();
        file.templates
            .retain(|t| extract_id(t) != Some(template_id));
        if file.templates.len() == len_before {
            return Ok(false);
        }
        self.save_file(&file)?;
        Ok(true)
    }
}

fn extract_id(value: &Value) -> Option<&str> {
    value.as_object()?.get("id")?.as_str()
}

fn ensure_id(mut value: Value, id: &str) -> Value {
    match value {
        Value::Object(ref mut map) => {
            map.insert("id".to_string(), Value::String(id.to_string()));
            value
        }
        _ => {
            let mut map = Map::new();
            map.insert("id".to_string(), Value::String(id.to_string()));
            Value::Object(map)
        }
    }
}
