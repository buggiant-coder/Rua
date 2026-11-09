use std::path::Path;

use sha2::{Digest, Sha256};

use rua_config::SkillScannerConfig;

pub fn compute_skill_content_hash(skill_dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    walk(skill_dir, &mut files)?;
    files.sort();

    let mut digest = Sha256::new();
    for path in &files {
        let bytes = std::fs::read(path)?;
        digest.update(&bytes);
    }
    Ok(hex_encode(&digest.finalize()))
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn is_skill_whitelisted(
    cfg: &SkillScannerConfig,
    skill_name: &str,
    skill_dir: Option<&Path>,
) -> bool {
    for entry in &cfg.whitelist {
        if entry.skill_name != skill_name {
            continue;
        }
        if entry.content_hash.is_empty() {
            return true;
        }
        if let Some(dir) = skill_dir {
            if let Ok(current) = compute_skill_content_hash(dir) {
                if current == entry.content_hash {
                    return true;
                }
            }
        } else {
            return true;
        }
    }
    false
}
