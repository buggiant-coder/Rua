use std::path::{Path, PathBuf};

use crate::error::SkillError;
use crate::signature::{build_signature, is_ignored_skill_path, IGNORED_ARTIFACTS};

pub fn copy_skill_dir(source: &Path, target: &Path) -> Result<(), SkillError> {
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::create_dir_all(target)?;
    copy_recursive(source, target)
}

fn copy_recursive(source: &Path, target: &Path) -> Result<(), SkillError> {
    let entries = match std::fs::read_dir(source) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };

    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if IGNORED_ARTIFACTS.contains(&name_str.as_ref()) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = target.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

pub fn build_signature_skip_artifacts(skill_dir: &Path) -> Result<String, SkillError> {
    Ok(build_signature(skill_dir)?)
}

pub(crate) fn collect_skill_files(root: &Path) -> Result<Vec<PathBuf>, SkillError> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), SkillError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if is_ignored_skill_path(&rel) {
            continue;
        }
        if file_type.is_dir() {
            walk(root, &path, out)?;
        } else if file_type.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}
