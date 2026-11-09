use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use crate::migration::atomic::atomic_write_json;
use crate::secrets::{is_secret_field, mask_env_value};

pub struct ChannelConfigStore {
    rua_root: PathBuf,
}

impl ChannelConfigStore {
    pub fn new(rua_root: PathBuf) -> Self {
        Self { rua_root }
    }

    fn config_path(&self) -> PathBuf {
        self.rua_root.join("config.json")
    }

    fn load_root(&self) -> std::io::Result<Map<String, Value>> {
        load_json_object(&self.config_path())
    }

    fn save_root(&self, root: Map<String, Value>) -> std::io::Result<()> {
        atomic_write_json(&self.config_path(), &Value::Object(root))
    }

    fn channels_map(root: &Map<String, Value>) -> Map<String, Value> {
        match root.get("channels") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        }
    }

    pub fn list_channels(&self) -> std::io::Result<Map<String, Value>> {
        let root = self.load_root()?;
        Ok(Self::channels_map(&root))
    }

    pub fn list_channels_masked(&self) -> std::io::Result<Map<String, Value>> {
        let map = self.list_channels()?;
        Ok(map.into_iter().map(|(k, v)| (k, mask_secrets(v))).collect())
    }

    pub fn get_channel(&self, channel_name: &str) -> std::io::Result<Option<Value>> {
        let root = self.load_root()?;
        Ok(Self::channels_map(&root).get(channel_name).cloned())
    }

    pub fn get_channel_masked(&self, channel_name: &str) -> std::io::Result<Option<Value>> {
        Ok(self.get_channel(channel_name)?.map(mask_secrets))
    }

    pub fn set_channel(&self, channel_name: &str, config: Value) -> std::io::Result<()> {
        let mut root = self.load_root()?;
        let mut channels = Self::channels_map(&root);
        channels.insert(channel_name.to_string(), config);
        root.insert("channels".to_string(), Value::Object(channels));
        self.save_root(root)
    }

    pub fn delete_channel(&self, channel_name: &str) -> std::io::Result<bool> {
        let mut root = self.load_root()?;
        let mut channels = Self::channels_map(&root);
        let removed = channels.remove(channel_name).is_some();
        root.insert("channels".to_string(), Value::Object(channels));
        self.save_root(root)?;
        Ok(removed)
    }

    pub fn replace_all(&self, channels: Map<String, Value>) -> std::io::Result<()> {
        let mut root = self.load_root()?;
        root.insert("channels".to_string(), Value::Object(channels));
        self.save_root(root)
    }
}

fn mask_secrets(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                let masked = match v {
                    Value::String(s) if is_secret_field(&k) && !s.is_empty() => {
                        Value::String(mask_env_value(&s))
                    }
                    other => mask_secrets(other),
                };
                out.insert(k, masked);
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(mask_secrets).collect()),
        other => other,
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
