use std::path::Path;

use agent_types::ContentPart;
use rua_core::tool::{
    ReadPageByteRange, ReadPageContinuation, SharedReadPage, ToolContext,
    MAX_SHARED_READ_TEXT_BYTES,
};
use rua_core::{Result, RuaError};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{OpenedRegularFile, ReadFileTool};

impl OpenedRegularFile {
    /// Compare this handle with a path selected independently by the local authorization service.
    pub async fn matches_approved_path(&self, approved_public_path: &Path) -> Result<bool> {
        #[cfg(unix)]
        {
            use rua_core::runtime_layout::{
                RuntimeLayout, MOBILE_SHARED_STORAGE_ANDROID, MOBILE_SHARED_STORAGE_PRIMARY,
            };
            use std::os::unix::fs::MetadataExt;

            if self.public_path.as_os_str() != approved_public_path.as_os_str()
                || ["/proc", "/sys", "/dev"]
                    .iter()
                    .any(|root| self.backing_path.starts_with(root))
            {
                return Ok(false);
            }
            let file = self.file.try_clone().await?.into_std().await;
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || super::opened_path(&file)?.as_os_str() != self.backing_path.as_os_str()
            {
                return Ok(false);
            }
            if self.resolved_public_path.as_os_str() == approved_public_path.as_os_str() {
                return Ok(true);
            }
            let Some(layout) = RuntimeLayout::from_current_process(None).linux_compat else {
                return Ok(false);
            };
            // Android registers an alternate private home only after matching the root's dev/ino.
            if let Ok(suffix) = self.backing_path.strip_prefix(&layout.host_home) {
                if self.resolved_public_path.as_os_str() == self.backing_path.as_os_str()
                    && layout.binds.iter().any(|bind| {
                        bind.host == layout.host_home
                            && bind.guest != bind.host
                            && bind.guest.join(suffix).as_os_str()
                                == approved_public_path.as_os_str()
                    })
                {
                    let selected = tokio::fs::metadata(approved_public_path).await?;
                    return Ok(selected.dev() == metadata.dev() && selected.ino() == metadata.ino());
                }
            }
            // Android exposes the same shared storage through two registered public roots.
            let alias = Path::new(MOBILE_SHARED_STORAGE_PRIMARY);
            let storage = Path::new(MOBILE_SHARED_STORAGE_ANDROID);
            let Ok(suffix) = self.backing_path.strip_prefix(storage) else {
                return Ok(false);
            };
            Ok(layout
                .binds
                .iter()
                .any(|bind| bind.guest == alias && bind.host == storage)
                && self.resolved_public_path.as_os_str() == alias.join(suffix).as_os_str()
                && approved_public_path.as_os_str() == self.backing_path.as_os_str())
        }
        #[cfg(not(unix))]
        {
            let _ = approved_public_path;
            Ok(false)
        }
    }
}

impl ReadFileTool {
    /// Project the normal reader's text window for a file explicitly authorized on this Device.
    /// Only the complete bounded UTF-8 input from this handle is eligible for this projection.
    pub async fn read_opened_page(
        &self,
        mut file: OpenedRegularFile,
        args: Value,
        mut ctx: ToolContext,
    ) -> Result<SharedReadPage> {
        let invalid = || RuaError::Validation {
            message: "A verified bounded text page is unavailable".into(),
        };
        file.validate_read_call(&args, &ctx)?;
        let approved_path = Path::new(args["file_path"].as_str().ok_or_else(invalid)?);
        if ctx.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
            || !(file.matches_approved_path(approved_path).await?)
            || file.file.metadata().await?.len() > MAX_SHARED_READ_TEXT_BYTES as u64
        {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        (&mut file.file)
            .take(MAX_SHARED_READ_TEXT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        if bytes.len() > MAX_SHARED_READ_TEXT_BYTES
            || bytes.contains(&0)
            || args["byte_offset"].as_u64().is_some_and(|offset| {
                usize::try_from(offset).map_or(true, |offset| !text.is_char_boundary(offset))
            })
            || !(file.matches_approved_path(approved_path).await?)
        {
            return Err(invalid());
        }
        file.bounded_text = Some(bytes);
        ctx.raw_capture_max_bytes = ctx
            .raw_capture_max_bytes
            .clamp(1, MAX_SHARED_READ_TEXT_BYTES);
        let result = self
            .read_opened_file(file, args, ctx)
            .await
            .map_err(|_| invalid())?;
        if result.is_error
            || result.attachments.is_some()
            || result
                .content
                .iter()
                .any(|part| !matches!(part, ContentPart::Text { .. }))
            || result.truncation.is_some()
        {
            return Err(invalid());
        }
        let Some(ContentPart::Text { text }) = result.content.into_iter().next() else {
            return Err(invalid());
        };
        let continuation = result
            .metadata
            .get("continuation")
            .map(|next| -> Result<_> {
                if let Some(byte_offset) = next.get("byte_offset") {
                    Ok(ReadPageContinuation::Bytes {
                        byte_offset: byte_offset.as_u64().ok_or_else(invalid)?,
                    })
                } else {
                    Ok(ReadPageContinuation::Lines {
                        start_line: next["start_line"].as_u64().ok_or_else(invalid)?,
                        limit: next["limit"].as_u64().ok_or_else(invalid)?,
                    })
                }
            })
            .transpose()?;
        let byte_range = result
            .metadata
            .get("byte_range")
            .map(|range| {
                serde_json::from_value::<ReadPageByteRange>(range.clone()).map_err(|_| invalid())
            })
            .transpose()?;
        let page = SharedReadPage {
            text,
            continuation,
            byte_range,
        };
        if !page.is_valid() {
            return Err(invalid());
        }
        Ok(page)
    }
}
