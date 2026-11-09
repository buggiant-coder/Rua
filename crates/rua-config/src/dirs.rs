use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub rua_root: PathBuf,
    pub secret_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn new(rua_root: Option<&str>) -> Self {
        let rua_root = resolve_rua_root(rua_root);
        let secret_dir = derive_secret_dir(&rua_root);
        let cache_dir = resolve_cache_dir(&rua_root);
        Self {
            rua_root,
            secret_dir,
            cache_dir,
        }
    }
}

fn derive_secret_dir(wd: &Path) -> PathBuf {
    if let Ok(val) = std::env::var("RUA_SECRET_DIR") {
        let val = val.trim();
        if !val.is_empty() {
            return shellexpand_path(val);
        }
    }
    derive_sibling_dir(wd, "secret")
}

pub fn resolve_cache_dir(rua_root: &Path) -> PathBuf {
    if let Ok(val) = std::env::var("RUA_CACHE_DIR") {
        let val = val.trim();
        if !val.is_empty() {
            return shellexpand_path(val);
        }
    }
    derive_sibling_dir(rua_root, "cache")
}

fn derive_sibling_dir(rua_root: &Path, suffix: &str) -> PathBuf {
    let parent = rua_root.parent().unwrap_or(Path::new("/"));
    let name = rua_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rua");
    parent.join(format!("{name}.{suffix}"))
}

pub fn resolve_rua_root(cli_arg: Option<&str>) -> PathBuf {
    if let Some(arg) = cli_arg {
        let trimmed = arg.trim();
        if !trimmed.is_empty() {
            return shellexpand_path(trimmed);
        }
    }
    if let Ok(val) = std::env::var("RUA_ROOT") {
        let val = val.trim();
        if !val.is_empty() {
            return shellexpand_path(val);
        }
    }
    home_dir().join(".rua")
}

pub fn rua_root() -> PathBuf {
    resolve_rua_root(None)
}

pub fn resolve_secret_dir() -> PathBuf {
    derive_secret_dir(&rua_root())
}

pub fn home_dir() -> PathBuf {
    env_home_dir()
        .or_else(windows_profile_dir)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn shellexpand_path(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        home_dir().join(rest)
    } else if let Some(rest) = s.strip_prefix("~\\") {
        home_dir().join(rest)
    } else if s == "~" {
        home_dir()
    } else {
        PathBuf::from(s)
    }
}

fn env_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn windows_profile_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            if drive.is_empty() || path.is_empty() {
                return None;
            }
            let mut combined = drive;
            combined.push(path);
            Some(PathBuf::from(combined))
        })
}
