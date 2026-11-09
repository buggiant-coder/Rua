use std::io::Write;
use std::path::Path;

use rua_core::{Result, RuaError};

pub fn download(lib_dir: &Path) -> Result<Vec<u8>> {
    if !lib_dir.is_dir() {
        return Err(RuaError::NotFound {
            resource: "prompt_library".into(),
            id: "directory".into(),
        });
    }
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut entries: Vec<_> = walkdir::WalkDir::new(lib_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in &entries {
        let path = entry.path();
        let arcname = path
            .strip_prefix(lib_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if arcname.is_empty() {
            continue;
        }
        if path.is_file() {
            zip.start_file(&arcname, options)
                .map_err(|e| RuaError::Io {
                    message: format!("Failed to write zip entry {arcname}: {e}"),
                })?;
            let data = std::fs::read(path).map_err(|e| RuaError::Io {
                message: format!("Failed to read {}: {e}", path.display()),
            })?;
            zip.write_all(&data).map_err(|e| RuaError::Io {
                message: format!("Failed to write zip data: {e}"),
            })?;
        } else if path.is_dir() {
            let dir_name = format!("{arcname}/");
            zip.add_directory(&dir_name, options)
                .map_err(|e| RuaError::Io {
                    message: format!("Failed to add zip directory {dir_name}: {e}"),
                })?;
        }
    }

    let cursor = zip.finish().map_err(|e| RuaError::Io {
        message: format!("Failed to finish zip: {e}"),
    })?;
    Ok(cursor.into_inner())
}

pub(crate) struct PreparedUpload {
    pub files: Vec<(String, Vec<u8>)>,
    pub message: &'static str,
}

pub(crate) fn prepare_upload(data: &[u8]) -> Result<PreparedUpload> {
    let reader = std::io::Cursor::new(data);
    let mut files = Vec::new();
    let message;
    if let Ok(mut archive) = zip::ZipArchive::new(reader) {
        let mut top = std::collections::BTreeSet::new();
        let mut root_file = false;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| RuaError::Validation {
                message: e.to_string(),
            })?;
            let name = entry.name().to_owned();
            if name
                .trim_end_matches('/')
                .split('/')
                .any(|part| part.is_empty() || part == ".")
                || name.contains("..")
                || name.contains('\\')
                || Path::new(&name).is_absolute()
                || Path::new(&name)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(RuaError::Validation {
                    message: format!("Zip contains unsafe path: {name}"),
                });
            }
            if let Some(first) = name.split('/').next() {
                top.insert(first.to_string());
            }
            if entry.is_dir() {
                continue;
            }
            root_file |= !name.contains('/');
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            files.push((name, bytes));
        }
        if top.len() == 1 && !root_file {
            for (name, _) in &mut files {
                *name = name
                    .split_once('/')
                    .expect("wrapper directory")
                    .1
                    .to_string();
            }
        }
        message = "Extracted zip";
    } else {
        files.push(("uploaded.md".into(), data.to_vec()));
        message = "Uploaded file";
    }
    for (name, bytes) in &files {
        if name.to_ascii_lowercase().ends_with(".md") {
            std::str::from_utf8(bytes).map_err(|e| RuaError::Validation {
                message: format!("Invalid Markdown {name}: {e}"),
            })?;
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(PreparedUpload { files, message })
}
