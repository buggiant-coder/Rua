use std::path::{Path, PathBuf};

use rua_config::types::security::{
    FileGuardConfig, SecurityConfig, SkillScannerConfig, SkillScannerWhitelistEntry,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("json error on {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("{resource} not found: {id}")]
    NotFound { resource: String, id: String },
}

#[derive(Debug, Clone)]
pub struct SecurityConfigStore {
    data_dir: PathBuf,
}

impl SecurityConfigStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn agent_security_path(&self, agent_id: &str) -> PathBuf {
        self.data_dir
            .join("workspaces")
            .join(agent_id)
            .join("security.json")
    }

    pub fn load(&self, agent_id: &str) -> Result<SecurityConfig, StoreError> {
        let path = self.agent_security_path(agent_id);
        match read_json::<SecurityConfig>(&path)? {
            Some(cfg) => Ok(cfg),
            None => Ok(Self::builtin_template()),
        }
    }

    pub fn save(&self, agent_id: &str, config: &SecurityConfig) -> Result<(), StoreError> {
        let path = self.agent_security_path(agent_id);
        let value = serde_json::to_value(config).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        rua_config::atomic_write_json(&path, &value)
            .map_err(|source| StoreError::Io { path, source })
    }

    pub fn delete(&self, agent_id: &str) -> Result<(), StoreError> {
        let path = self.agent_security_path(agent_id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    pub fn builtin_template() -> SecurityConfig {
        SecurityConfig::default()
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    fn blocked_history_path(&self) -> PathBuf {
        self.data_dir.join("skill_scanner_blocked.json")
    }

    pub(crate) fn load_security_section(&self) -> Result<SecurityConfig, StoreError> {
        let path = self.config_path();
        let raw: Option<serde_json::Value> = read_json(&path)?;
        let root_val = raw.unwrap_or_else(|| serde_json::json!({}));
        let sec_val = root_val
            .get("security")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        serde_json::from_value(sec_val).map_err(|source| StoreError::Json { path, source })
    }

    pub(crate) fn save_security_section(&self, sec: &SecurityConfig) -> Result<(), StoreError> {
        let path = self.config_path();
        let mut root_val: serde_json::Value =
            read_json(&path)?.unwrap_or_else(|| serde_json::json!({}));
        let sec_val = serde_json::to_value(sec).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        root_val
            .as_object_mut()
            .unwrap()
            .insert("security".to_string(), sec_val);
        rua_config::atomic_write_json(&path, &root_val)
            .map_err(|source| StoreError::Io { path, source })
    }

    pub fn get_security_config(&self) -> Result<SecurityConfig, StoreError> {
        self.load_security_section()
    }

    pub fn get_allow_no_auth_hosts(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.load_security_section()?.allow_no_auth_hosts)
    }

    pub fn update_allow_no_auth_hosts(
        &self,
        hosts: Vec<String>,
    ) -> Result<Vec<String>, StoreError> {
        let mut sec = self.load_security_section()?;
        sec.allow_no_auth_hosts = hosts;
        self.save_security_section(&sec)?;
        Ok(sec.allow_no_auth_hosts)
    }

    pub fn get_file_guard(&self) -> Result<FileGuardConfig, StoreError> {
        Ok(self.load_security_section()?.file_guard)
    }

    pub fn update_file_guard(
        &self,
        new_cfg: FileGuardConfig,
    ) -> Result<FileGuardConfig, StoreError> {
        let mut sec = self.load_security_section()?;
        sec.file_guard = new_cfg;
        self.save_security_section(&sec)?;
        Ok(sec.file_guard)
    }

    pub fn get_skill_scanner(&self) -> Result<SkillScannerConfig, StoreError> {
        Ok(self.load_security_section()?.skill_scanner)
    }

    pub fn update_skill_scanner(
        &self,
        new_cfg: SkillScannerConfig,
    ) -> Result<SkillScannerConfig, StoreError> {
        let mut sec = self.load_security_section()?;
        sec.skill_scanner = new_cfg;
        self.save_security_section(&sec)?;
        Ok(sec.skill_scanner)
    }

    pub fn add_whitelist(
        &self,
        entry: SkillScannerWhitelistEntry,
    ) -> Result<SkillScannerWhitelistEntry, StoreError> {
        let mut sec = self.load_security_section()?;
        sec.skill_scanner.whitelist.push(entry.clone());
        self.save_security_section(&sec)?;
        Ok(entry)
    }

    pub fn remove_whitelist(&self, skill_name: &str) -> Result<(), StoreError> {
        let mut sec = self.load_security_section()?;
        let before = sec.skill_scanner.whitelist.len();
        sec.skill_scanner
            .whitelist
            .retain(|w| w.skill_name != skill_name);
        if sec.skill_scanner.whitelist.len() == before {
            return Err(StoreError::NotFound {
                resource: "whitelist_entry".into(),
                id: skill_name.into(),
            });
        }
        self.save_security_section(&sec)?;
        Ok(())
    }

    pub fn get_blocked_history(&self) -> Result<Vec<serde_json::Value>, StoreError> {
        let path = self.blocked_history_path();
        match read_json::<Vec<serde_json::Value>>(&path)? {
            Some(entries) => Ok(entries),
            None => Ok(Vec::new()),
        }
    }

    fn save_blocked_history(&self, entries: &[serde_json::Value]) -> Result<(), StoreError> {
        let path = self.blocked_history_path();
        if entries.is_empty() {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
            return Ok(());
        }
        let value = serde_json::to_value(entries).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        rua_config::atomic_write_json(&path, &value)
            .map_err(|source| StoreError::Io { path, source })
    }

    pub fn append_blocked_entries(&self, entries: &[serde_json::Value]) -> Result<(), StoreError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut history = self.get_blocked_history()?;
        history.extend_from_slice(entries);
        self.save_blocked_history(&history)
    }

    pub fn clear_blocked_history(&self) -> Result<(), StoreError> {
        self.save_blocked_history(&[])
    }

    pub fn delete_blocked_history_entry(&self, index: usize) -> Result<(), StoreError> {
        let mut history = self.get_blocked_history()?;
        if index >= history.len() {
            return Err(StoreError::NotFound {
                resource: "blocked_history_entry".into(),
                id: index.to_string(),
            });
        }
        history.remove(index);
        self.save_blocked_history(&history)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(|source| StoreError::Json {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StoreError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
