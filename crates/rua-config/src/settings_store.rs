use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use crate::migration::atomic::atomic_write_json;

pub struct SettingsStore {
    rua_root: PathBuf,
}

impl SettingsStore {
    pub fn new(rua_root: PathBuf) -> Self {
        Self { rua_root }
    }

    fn settings_path(&self) -> PathBuf {
        self.rua_root.join("settings.json")
    }

    pub fn load(&self) -> std::io::Result<Map<String, Value>> {
        load_json_object(&self.settings_path())
    }

    pub fn get(&self, key: &str) -> std::io::Result<Option<Value>> {
        Ok(self.load()?.remove(key))
    }

    pub fn set(&self, key: &str, value: Value) -> std::io::Result<()> {
        let mut data = self.load()?;
        data.insert(key.to_string(), value);
        atomic_write_json(&self.settings_path(), &Value::Object(data))
    }

    pub fn delete(&self, key: &str) -> std::io::Result<bool> {
        let mut data = self.load()?;
        let removed = data.remove(key).is_some();
        atomic_write_json(&self.settings_path(), &Value::Object(data))?;
        Ok(removed)
    }

    pub fn replace_all(&self, data: Map<String, Value>) -> std::io::Result<()> {
        atomic_write_json(&self.settings_path(), &Value::Object(data))
    }
}

fn load_json_object(path: &Path) -> std::io::Result<Map<String, Value>> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(Map::new()),
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
            match value {
                Value::Object(map) => Ok(map),
                _ => Err(std::io::Error::other(format!(
                    "{} is not a JSON object",
                    path.display()
                ))),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}
