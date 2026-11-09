use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::{debug, warn};

#[derive(Debug, Clone, Default)]
pub struct WorkspaceContext {
    pub memory_text: String,
    pub kb_text: String,
}

impl WorkspaceContext {
    pub fn is_empty(&self) -> bool {
        self.memory_text.is_empty() && self.kb_text.is_empty()
    }

    pub fn to_injection_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.memory_text.is_empty() {
            parts.push(format!("# Memories\n\n{}", self.memory_text));
        }
        if !self.kb_text.is_empty() {
            parts.push(format!("# Knowledge Base\n\n{}", self.kb_text));
        }
        parts.join("\n\n")
    }
}

pub async fn load_workspace_context(workspace_dir: &Path) -> WorkspaceContext {
    let kb_text = load_dirs_md_merged(&[
        workspace_dir.join("KNOWLEDGE"),
        workspace_dir.join("knowledge"),
    ])
    .await;

    debug!(
        memory_len = 0usize,
        kb_len = kb_text.len(),
        "loaded workspace context"
    );

    WorkspaceContext {
        memory_text: String::new(),
        kb_text,
    }
}

pub async fn load_dirs_md_merged(dirs: &[PathBuf]) -> String {
    let mut seen_names = HashSet::new();
    let mut texts = Vec::new();

    for dir in dirs {
        let mut entries = match fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_lowercase(),
                None => continue,
            };
            if !seen_names.insert(file_name) {
                continue;
            }
            match fs::read_to_string(&path).await {
                Ok(content) if !content.trim().is_empty() => {
                    texts.push(content);
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read md file");
                }
            }
        }
    }

    texts.join("\n\n")
}
