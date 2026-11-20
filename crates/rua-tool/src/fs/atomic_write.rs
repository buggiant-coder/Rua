use std::path::Path;

use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::error::ToolError;

pub async fn atomic_write(
    path: &Path,
    content: &[u8],
    create_parents: bool,
) -> Result<(), ToolError> {
    let parent = path.parent().ok_or_else(|| ToolError::AtomicWriteFailed {
        path: path.to_path_buf(),
        message: "path has no parent directory".into(),
    })?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    if create_parents {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::AtomicWriteFailed {
                path: path.to_path_buf(),
                message: format!("failed to create parent directories: {e}"),
            })?;
    }

    let temp_name = format!(".rua-tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);

    let write_result = async {
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(content).await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(e) = write_result {
        let _ = fs::remove_file(&temp_path).await;
        return Err(ToolError::AtomicWriteFailed {
            path: path.to_path_buf(),
            message: format!("failed to write temp file: {e}"),
        });
    }

    let target = path.to_path_buf();
    let parent = parent.to_path_buf();
    // Dropping the caller's future must not interrupt publication between rename and fsync.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::rename(&temp_path, &target) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(ToolError::AtomicWriteFailed {
                path: target,
                message: format!("failed to rename temp file: {e}"),
            });
        }
        rua_core::fs::sync_directory(&parent).map_err(|e| ToolError::WriteOutcomeUnconfirmed {
            path: target,
            message: format!("file was renamed but syncing its parent directory failed: {e}"),
        })
    })
    .await
    .map_err(|e| ToolError::WriteOutcomeUnconfirmed {
        path: path.to_path_buf(),
        message: format!("could not confirm file publication: {e}"),
    })?
}

pub fn detect_newline_style(content: &str) -> NewlineStyle {
    if content.contains("\r\n") {
        NewlineStyle::CrLf
    } else {
        NewlineStyle::Lf
    }
}

pub fn normalize_newlines(content: &str, style: NewlineStyle) -> String {
    match style {
        NewlineStyle::Lf => content.replace("\r\n", "\n"),
        NewlineStyle::CrLf => {
            let without_crlf = content.replace("\r\n", "\n");
            without_crlf.replace('\n', "\r\n")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewlineStyle {
    Lf,
    CrLf,
}
