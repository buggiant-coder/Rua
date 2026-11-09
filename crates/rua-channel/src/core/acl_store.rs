use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ACCESS_CONTROL_FILE: &str = "access_control.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UserInfo {
    #[serde(default)]
    pub remark: String,

    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingEntry {
    pub user_id: String,

    pub channel: String,

    #[serde(default)]
    pub timestamp: f64,

    #[serde(default)]
    pub first_message: String,

    #[serde(default)]
    pub remark: String,

    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChannelAclData {
    #[serde(default)]
    pub whitelist: HashMap<String, UserInfo>,

    #[serde(default)]
    pub blacklist: HashMap<String, UserInfo>,

    #[serde(default)]
    pub pending: Vec<PendingEntry>,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    data: HashMap<String, ChannelAclData>,
    last_mtime: Option<SystemTime>,
}

impl Inner {
    fn load(&mut self) {
        let meta = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return,
        };
        self.last_mtime = meta.modified().ok();
        match fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<HashMap<String, ChannelAclData>>(&text) {
                Ok(data) => self.data = data,
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %self.path.display(),
                    "failed to parse access_control.json"
                ),
            },
            Err(e) => tracing::warn!(
                error = %e,
                path = %self.path.display(),
                "failed to read access_control.json"
            ),
        }
    }

    fn reload_if_stale(&mut self) {
        let meta = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return,
        };
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => return,
        };
        let stale = match self.last_mtime {
            Some(prev) => mtime > prev,
            None => true,
        };
        if stale {
            self.load();
        }
    }

    fn save(&mut self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                tracing::warn!(error = %e, path = %parent.display(), "failed to create acl dir");
                return;
            }
        }
        let payload = match serde_json::to_string_pretty(&self.data) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize access control data");
                return;
            }
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Err(e) = fs::write(&tmp, payload.as_bytes()) {
            tracing::warn!(error = %e, path = %tmp.display(), "failed to write acl tmp file");
            return;
        }
        if let Err(e) = fs::rename(&tmp, &self.path) {
            tracing::warn!(error = %e, path = %self.path.display(), "failed to rename acl tmp file");
            return;
        }
        if let Ok(meta) = fs::metadata(&self.path) {
            self.last_mtime = meta.modified().ok();
        }
    }
}

#[derive(Debug)]
pub struct AccessControlStore {
    inner: Mutex<Inner>,
}

impl AccessControlStore {
    pub fn new(path: PathBuf) -> Self {
        let mut inner = Inner {
            path,
            data: HashMap::new(),
            last_mtime: None,
        };
        inner.load();
        Self {
            inner: Mutex::new(inner),
        }
    }

    pub fn for_workspace(workspace_dir: &Path) -> Self {
        Self::new(workspace_dir.join(ACCESS_CONTROL_FILE))
    }

    pub fn is_whitelisted(&self, channel: &str, user_id: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        guard
            .data
            .get(channel)
            .map(|acl| acl.whitelist.contains_key(user_id))
            .unwrap_or(false)
    }

    pub fn is_blacklisted(&self, channel: &str, user_id: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        guard
            .data
            .get(channel)
            .map(|acl| acl.blacklist.contains_key(user_id))
            .unwrap_or(false)
    }

    pub fn get_acl(&self, channel: &str) -> ChannelAclData {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        guard.data.get(channel).cloned().unwrap_or_default()
    }

    pub fn get_all_acls(&self) -> HashMap<String, ChannelAclData> {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        guard.data.clone()
    }

    pub fn add_to_whitelist(&self, channel: &str, user_id: &str, remark: &str, username: &str) {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        {
            let acl = guard.data.entry(channel.to_string()).or_default();
            let existing = acl.whitelist.get(user_id);
            let existing_blacklist = acl.blacklist.get(user_id);
            let remark = if remark.is_empty() {
                existing
                    .or(existing_blacklist)
                    .map(|info| info.remark.clone())
                    .unwrap_or_default()
            } else {
                remark.to_string()
            };
            let username = if username.is_empty() {
                existing
                    .or(existing_blacklist)
                    .map(|info| info.username.clone())
                    .unwrap_or_default()
            } else {
                username.to_string()
            };
            acl.whitelist
                .insert(user_id.to_string(), UserInfo { remark, username });
            acl.blacklist.remove(user_id);
            acl.pending
                .retain(|p| !(p.user_id == user_id && p.channel == channel));
        }
        guard.save();
    }

    pub fn remove_from_whitelist(&self, channel: &str, user_id: &str) {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard
            .data
            .entry(channel.to_string())
            .or_default()
            .whitelist
            .remove(user_id);
        guard.save();
    }

    pub fn add_to_blacklist(&self, channel: &str, user_id: &str, remark: &str, username: &str) {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        {
            let acl = guard.data.entry(channel.to_string()).or_default();
            let existing = acl.blacklist.get(user_id);
            let existing_whitelist = acl.whitelist.get(user_id);
            let remark = if remark.is_empty() {
                existing
                    .or(existing_whitelist)
                    .map(|info| info.remark.clone())
                    .unwrap_or_default()
            } else {
                remark.to_string()
            };
            let username = if username.is_empty() {
                existing
                    .or(existing_whitelist)
                    .map(|info| info.username.clone())
                    .unwrap_or_default()
            } else {
                username.to_string()
            };
            acl.blacklist
                .insert(user_id.to_string(), UserInfo { remark, username });
            acl.whitelist.remove(user_id);
            acl.pending
                .retain(|p| !(p.user_id == user_id && p.channel == channel));
        }
        guard.save();
    }

    pub fn remove_from_blacklist(&self, channel: &str, user_id: &str) {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard
            .data
            .entry(channel.to_string())
            .or_default()
            .blacklist
            .remove(user_id);
        guard.save();
    }

    pub fn update_remark(&self, channel: &str, user_id: &str, remark: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let Some(acl) = guard.data.get_mut(channel) else {
            return false;
        };
        if let Some(info) = acl.whitelist.get_mut(user_id) {
            info.remark = remark.to_string();
            guard.save();
            return true;
        }
        if let Some(info) = acl.blacklist.get_mut(user_id) {
            info.remark = remark.to_string();
            guard.save();
            return true;
        }
        false
    }

    pub fn update_username(&self, channel: &str, user_id: &str, username: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let Some(acl) = guard.data.get_mut(channel) else {
            return false;
        };
        let mut found = false;
        if let Some(info) = acl.whitelist.get_mut(user_id) {
            info.username = username.to_string();
            found = true;
        }
        if let Some(info) = acl.blacklist.get_mut(user_id) {
            info.username = username.to_string();
            found = true;
        }
        for entry in &mut acl.pending {
            if entry.user_id == user_id && entry.channel == channel {
                entry.username = username.to_string();
                found = true;
            }
        }
        if found {
            guard.save();
        }
        found
    }

    pub fn record_pending(
        &self,
        channel: &str,
        user_id: &str,
        first_message: &str,
        username: &str,
    ) {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        {
            let acl = guard.data.entry(channel.to_string()).or_default();
            if acl
                .pending
                .iter()
                .any(|p| p.user_id == user_id && p.channel == channel)
            {
                return;
            }
            let truncated: String = first_message.chars().take(200).collect();
            acl.pending.push(PendingEntry {
                user_id: user_id.to_string(),
                channel: channel.to_string(),
                timestamp: now_unix(),
                first_message: truncated,
                remark: String::new(),
                username: username.to_string(),
            });
        }
        guard.save();
    }

    pub fn get_all_pending(&self) -> Vec<PendingEntry> {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        let mut entries: Vec<PendingEntry> = guard
            .data
            .values()
            .flat_map(|acl| acl.pending.iter().cloned())
            .collect();
        entries.sort_by(|a, b| b.timestamp.total_cmp(&a.timestamp));
        entries
    }

    pub fn update_pending_remark(&self, channel: &str, user_id: &str, remark: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let Some(acl) = guard.data.get_mut(channel) else {
            return false;
        };
        for entry in &mut acl.pending {
            if entry.user_id == user_id && entry.channel == channel {
                entry.remark = remark.to_string();
                guard.save();
                return true;
            }
        }
        false
    }

    pub fn approve_pending(&self, channel: &str, user_id: &str, remark: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let acl = guard.data.entry(channel.to_string()).or_default();
        let mut effective_remark = remark.to_string();
        let mut username = String::new();
        for entry in &acl.pending {
            if entry.user_id == user_id && entry.channel == channel {
                if effective_remark.is_empty() {
                    effective_remark = entry.remark.clone();
                }
                username = entry.username.clone();
                break;
            }
        }
        acl.pending
            .retain(|p| !(p.user_id == user_id && p.channel == channel));
        acl.whitelist.insert(
            user_id.to_string(),
            UserInfo {
                remark: effective_remark,
                username,
            },
        );
        acl.blacklist.remove(user_id);
        guard.save();
        true
    }

    pub fn deny_pending(&self, channel: &str, user_id: &str, remark: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let acl = guard.data.entry(channel.to_string()).or_default();
        let mut effective_remark = remark.to_string();
        let mut username = String::new();
        for entry in &acl.pending {
            if entry.user_id == user_id && entry.channel == channel {
                if effective_remark.is_empty() {
                    effective_remark = entry.remark.clone();
                }
                username = entry.username.clone();
                break;
            }
        }
        acl.pending
            .retain(|p| !(p.user_id == user_id && p.channel == channel));
        acl.blacklist.insert(
            user_id.to_string(),
            UserInfo {
                remark: effective_remark,
                username,
            },
        );
        acl.whitelist.remove(user_id);
        guard.save();
        true
    }

    pub fn dismiss_pending(&self, channel: &str, user_id: &str) -> bool {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        let acl = guard.data.entry(channel.to_string()).or_default();
        let before = acl.pending.len();
        acl.pending
            .retain(|p| !(p.user_id == user_id && p.channel == channel));
        let removed = acl.pending.len() < before;
        if removed {
            guard.save();
        }
        removed
    }

    pub fn pending(&self, channel: &str) -> Vec<PendingEntry> {
        let mut guard = self.inner.lock().expect("acl store mutex poisoned");
        guard.reload_if_stale();
        guard
            .data
            .get(channel)
            .map(|acl| acl.pending.clone())
            .unwrap_or_default()
    }
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

const SUPPORTED_LANGS: [&str; 6] = ["zh", "en", "ja", "ru", "pt-BR", "id"];

fn normalize_lang(lang: &str) -> &str {
    if SUPPORTED_LANGS.contains(&lang) {
        lang
    } else if lang.starts_with("zh") {
        "zh"
    } else {
        "en"
    }
}

fn blocked_text(lang: &str) -> &'static str {
    match lang {
        "zh" => "您已被禁止访问此智能体。",
        "ja" => "このエージェントへのアクセスがブロックされています。",
        "ru" => "Вам заблокирован доступ к этому агенту.",
        "pt-BR" => "Você foi bloqueado deste agente.",
        "id" => "Anda telah diblokir dari agen ini.",
        _ => "You have been blocked from this agent.",
    }
}

fn pending_text(lang: &str) -> &'static str {
    match lang {
        "zh" => "您目前没有访问此智能体的权限，需要审批。\nID: {sender_id}",
        "ja" => "このエージェントへのアクセス権がありません。承認が必要です。\nID: {sender_id}",
        "ru" => "У вас нет доступа к этому агенту. Требуется одобрение.\nID: {sender_id}",
        "pt-BR" => "Você não tem acesso a este agente. Aprovação necessária.\nID: {sender_id}",
        "id" => "Anda tidak memiliki akses ke agen ini. Persetujuan diperlukan.\nID: {sender_id}",
        _ => "You do not have access to this agent. Approval required.\nID: {sender_id}",
    }
}

pub fn acl_message(key: &str, lang: &str, sender_id: &str) -> String {
    let lang = normalize_lang(lang);
    match key {
        "blocked" => blocked_text(lang).to_string(),
        "pending" => pending_text(lang).replace("{sender_id}", sender_id),
        _ => String::new(),
    }
}
