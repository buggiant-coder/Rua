use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const IGNORED_ARTIFACTS: &[&str] = &[
    "__pycache__",
    "__MACOSX",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
];

pub fn is_ignored_skill_path(relative: &Path) -> bool {
    relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|seg| IGNORED_ARTIFACTS.contains(&seg))
}

pub fn build_signature(skill_dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(skill_dir, &mut files)?;

    files.sort_by(|a, b| {
        let ra = a.strip_prefix(skill_dir).unwrap_or(a);
        let rb = b.strip_prefix(skill_dir).unwrap_or(b);
        ra.cmp(rb)
    });

    let mut digest = Sha256::new();
    for path in &files {
        let rel = match path.strip_prefix(skill_dir) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        if is_ignored_skill_path(&rel) {
            continue;
        }
        let rel_str = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/");
        digest.update(rel_str.as_bytes());

        let bytes = std::fs::read(path)?;
        digest.update(&bytes);
    }

    Ok(hex_encode(&digest.finalize()))
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_files(&path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
