use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};

use crate::annotations::ExtendedToolAnnotations;
use crate::context::{ToolExecutionContext, DEFAULT_READ_LINES};
use crate::error::ToolError;
use crate::schema::ToolSpec;

mod opened;
pub use opened::OpenedRegularFile;

pub(crate) enum ReadSource {
    Path,
    Media(rua_core::ResolvedChatMedia),
    Opened(OpenedRegularFile),
}

fn file_read_state() -> &'static Mutex<HashMap<PathBuf, SystemTime>> {
    static STATE: OnceLock<Mutex<HashMap<PathBuf, SystemTime>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn record_file_mtime(path: &std::path::Path, mtime: SystemTime) {
    if let Ok(canonical) = path.canonicalize() {
        if let Ok(mut state) = file_read_state().lock() {
            state.insert(canonical, mtime);
        }
    }
}

pub(crate) fn update_file_mtime(path: &std::path::Path, new_mtime: SystemTime) {
    if let Ok(canonical) = path.canonicalize() {
        if let Ok(mut state) = file_read_state().lock() {
            state.insert(canonical, new_mtime);
        }
    }
}

pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: format!(
                "Read a line window from a text file or list one directory page sorted by filename. Each call returns at most `limit` lines \
                 (default {DEFAULT_READ_LINES}) with line numbers formatted as `line | text`. Explicit \
                 `start_line` and `limit` windows return the requested lines. If the tool applies its \
                 default or runtime safety limit, follow the returned continuation to read the next \
                 line window or byte window of a very long line."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": crate::device_file::device_id_parameter(),
                    "byte_offset": {"type":"integer","minimum":0,"description":"Continue a bounded text preview at the returned byte offset."},
                    "after": {"type":"string","description":"For a directory, resume after the exact name from the previous page."},
                    "file_path": {
                        "type": "string",
                        "description": "Path to the text file to read"
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "First line to read (1-based, inclusive). Defaults to 1. Use the continuation hint's start_line to keep reading."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": format!("Maximum number of lines to return. Defaults to {DEFAULT_READ_LINES}; very large requests are capped by a runtime safety limit. Use smaller windows for large files.")
                    }
                },
                "required": ["file_path"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }

    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        self.read_from(args, ctx, ReadSource::Path, None).await
    }

    pub(crate) async fn read_from(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
        source: ReadSource,
        device_location: Option<(&str, &Path)>,
    ) -> std::result::Result<ToolResult, ToolError> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| ToolError::SchemaValidation {
                tool: "read".into(),
                message: "missing or invalid 'file_path' field".into(),
            })?;
        let already_open = matches!(source, ReadSource::Opened(_));
        let bounded_page = matches!(&source, ReadSource::Opened(file) if file.bounded_page);
        if ctx.is_cancelled() {
            return Ok(ToolResult::error(&ctx.trace_id, "Read cancelled"));
        }

        let start_line = match args.get("start_line") {
            Some(value) if !value.is_null() => match value.as_u64() {
                Some(n) => Some(n as usize),
                None => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        "Error: start_line must be an integer.",
                    ));
                }
            },
            _ => None,
        };
        let limit = match args.get("limit") {
            Some(value) if !value.is_null() => match value.as_u64() {
                Some(n) => Some(n as usize),
                None => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        "Error: limit must be an integer.",
                    ));
                }
            },
            _ => None,
        };
        if matches!(start_line, Some(0)) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: start_line must be greater than or equal to 1.",
            ));
        }
        if matches!(limit, Some(0)) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: limit must be greater than or equal to 1.",
            ));
        }
        if args.get("end_line").is_some() {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: read uses start_line and limit; end_line is not supported.",
            ));
        }

        let resolved = match &source {
            ReadSource::Path => ctx.workspace_fs.resolve(file_path),
            ReadSource::Media(media) => ctx.workspace_fs.resolve_host_path(&media.path),
            ReadSource::Opened(file) => Ok(file.backing_path.clone()),
        };
        let resolved = match resolved {
            Ok(path) => path,
            Err(ToolError::FileNotFound { .. }) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: The file {file_path} does not exist."),
                ));
            }
            Err(e) => return Err(e),
        };

        let metadata = match &source {
            ReadSource::Opened(file) => file.file.metadata().await,
            _ => fs::metadata(&resolved).await,
        };
        let metadata = match metadata {
            Ok(m) => m,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: The file {file_path} does not exist."),
                ));
            }
            Err(error) => return Err(error.into()),
        };

        if metadata.is_dir() && !already_open {
            return directory_page(
                &args,
                &resolved,
                file_path,
                ctx,
                limit,
                device_location.is_none(),
            )
            .await;
        }
        if !metadata.is_file() {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                format!("The path {file_path} is not a regular file or directory."),
            ));
        }
        if args.get("after").is_some() {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "after is only supported for a directory",
            ));
        }
        let start = start_line.unwrap_or(1);
        let requested_limit = limit
            .unwrap_or(ctx.limits.default_read_lines)
            .min(ctx.limits.max_read_lines)
            .max(1);
        let tool_limited = limit.is_none() || limit.is_some_and(|n| n > ctx.limits.max_read_lines);
        let file = match source {
            ReadSource::Opened(file) => file.file,
            _ => fs::File::open(&resolved).await?,
        };
        let mut reader = tokio::io::BufReader::new(file);
        let budget = ctx.raw_capture_max_bytes.max(1);
        if reader.fill_buf().await?.contains(&0) {
            return Err(ToolError::BinaryFile {
                path: file_path.into(),
            });
        }
        if let Some(offset) = args.get("byte_offset") {
            let Some(offset) = offset
                .as_u64()
                .filter(|_| start_line.is_none() && limit.is_none())
            else {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    "Use a nonnegative byte_offset without start_line or limit.",
                ));
            };
            return text_bytes(
                &mut reader,
                offset..metadata.len(),
                None,
                file_path,
                ctx,
                device_location.is_none(),
                bounded_page,
            )
            .await;
        }
        let mut byte_position = 0u64;
        // Skip preceding lines without allocating them, including very long lines.
        let mut line_no = 1;
        while line_no < start {
            if ctx.is_cancelled() {
                return Ok(ToolResult::error(&ctx.trace_id, "Read cancelled"));
            }
            let chunk = reader.fill_buf().await?;
            if chunk.is_empty() {
                break;
            }
            let mut count = 0;
            for &byte in chunk {
                count += 1;
                if byte == 0 {
                    return Err(ToolError::BinaryFile {
                        path: file_path.into(),
                    });
                }
                if byte == b'\n' {
                    line_no += 1;
                    if line_no == start {
                        break;
                    }
                }
            }
            reader.consume(count);
            byte_position += count as u64;
        }
        if reader.fill_buf().await?.is_empty() && (start != 1 || metadata.len() != 0) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                format!("Error: start_line {start} exceeds file length."),
            ));
        }
        let mut output = String::new();
        let mut row = Vec::new();
        let mut count = 0;
        let mut byte_limited = false;
        let mut has_more = false;
        while count < requested_limit {
            if ctx.is_cancelled() {
                return Ok(ToolResult::error(&ctx.trace_id, "Read cancelled"));
            }
            row.clear();
            let remaining = budget - output.len();
            let n = (&mut reader)
                .take((remaining as u64).saturating_add(1))
                .read_until(b'\n', &mut row)
                .await?;
            if n == 0 {
                break;
            }
            if row.contains(&0) {
                return Err(ToolError::BinaryFile {
                    path: file_path.into(),
                });
            }
            let content = row.strip_suffix(b"\n").map_or(row.as_slice(), |line| {
                line.strip_suffix(b"\r").unwrap_or(line)
            });
            let line = format!("{line_no:>6} | {}\n", String::from_utf8_lossy(content));
            if n > remaining || line.len() > remaining {
                if count == 0 {
                    return text_bytes(
                        &mut reader,
                        byte_position..metadata.len(),
                        row.ends_with(b"\n").then_some(n),
                        file_path,
                        ctx,
                        device_location.is_none(),
                        bounded_page,
                    )
                    .await;
                }
                has_more = true;
                byte_limited = true;
                break;
            }
            if bounded_page && std::str::from_utf8(content).is_err() {
                return Err(ToolError::BinaryFile {
                    path: file_path.into(),
                });
            }
            output.push_str(&line);
            byte_position += n as u64;
            count += 1;
            line_no += 1;
        }
        has_more |= !reader.fill_buf().await?.is_empty();
        let mut result = ToolResult::text(&ctx.trace_id, &output);
        if has_more {
            let mut next = serde_json::json!({"file_path":file_path,"start_line":line_no,"limit":requested_limit});
            if let Some((device_id, public_path)) = device_location {
                next["device_id"] = device_id.into();
                next["file_path"] = serde_json::json!(public_path);
            }
            if (byte_limited || tool_limited) && !bounded_page {
                let hint = if device_location.is_some() {
                    format!("read({next})")
                } else {
                    format!("read(file_path={file_path:?}, start_line={line_no}, limit={requested_limit})")
                };
                let meta = crate::result_store::continuation_meta(
                    "read",
                    None,
                    Some(metadata.len()),
                    start as u64,
                    (line_no - 1) as u64,
                    hint,
                );
                let mut body = crate::result_store::format_truncation_warning(&meta);
                body.push_str(&output);
                result.content = vec![agent_types::ContentPart::Text { text: body }];
                result.truncation = Some(meta);
            }
            result.metadata.insert("continuation".into(), next);
        }
        if !already_open {
            if let Ok(mtime) = metadata.modified() {
                record_file_mtime(&resolved, mtime);
            }
        }

        Ok(result)
    }
}

async fn text_bytes(
    reader: &mut (impl tokio::io::AsyncBufRead + tokio::io::AsyncSeek + Unpin),
    range: std::ops::Range<u64>,
    line_bytes: Option<usize>,
    path: &str,
    ctx: &ToolExecutionContext,
    emit_hint: bool,
    strict_utf8: bool,
) -> std::result::Result<ToolResult, ToolError> {
    let (offset, length) = (range.start, range.end);
    if offset > length || (offset == length && length > 0) {
        return Ok(ToolResult::error(
            &ctx.trace_id,
            format!("Byte offset {offset} exceeds the text file range of {length} bytes."),
        ));
    }
    reader.seek(std::io::SeekFrom::Start(offset)).await?;
    if length == 0 && !reader.fill_buf().await?.is_empty() {
        return Ok(ToolResult::error(
            &ctx.trace_id,
            "The file reports zero bytes but contains data; its byte range is unavailable.",
        ));
    }
    let mut bytes = Vec::new();
    let budget = ctx.raw_capture_max_bytes.max(1);
    // Three bytes of lookahead distinguish a split scalar from invalid input.
    let available = (length - offset).min(line_bytes.map_or(u64::MAX, |n| n as u64));
    reader
        .take((budget as u64).saturating_add(3).min(available))
        .read_to_end(&mut bytes)
        .await?;
    if bytes.contains(&0) {
        return Err(ToolError::BinaryFile { path: path.into() });
    }
    if bytes.is_empty() && length != 0 {
        return Ok(ToolResult::error(
            &ctx.trace_id,
            "The text file changed while reading this range.",
        ));
    }
    let mut shown = String::new();
    let mut consumed = 0;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        let mut take = valid.len().min(budget - shown.len());
        while !valid.is_char_boundary(take) {
            take -= 1;
        }
        shown.push_str(&valid[..take]);
        consumed += take;
        if take < valid.len() {
            break;
        }
        if !chunk.invalid().is_empty() {
            if strict_utf8 && shown.len() < budget {
                return Err(ToolError::BinaryFile { path: path.into() });
            }
            if budget - shown.len() < char::REPLACEMENT_CHARACTER.len_utf8() {
                break;
            }
            shown.push(char::REPLACEMENT_CHARACTER);
            consumed += chunk.invalid().len();
        }
    }
    if consumed == 0 && !bytes.is_empty() {
        return Ok(ToolResult::error(
            &ctx.trace_id,
            "The configured read byte budget is too small for the next UTF-8 character.",
        ));
    }
    let end = offset + consumed as u64;
    let mut result = ToolResult::text(&ctx.trace_id, shown);
    result.metadata.insert(
        "byte_range".into(),
        serde_json::json!({"start":offset,"end_exclusive":end,"total_bytes":length}),
    );
    if end < length {
        let next = serde_json::json!({"file_path":path,"byte_offset":end});
        let mut text = format!("\nText bytes {offset}..{end} of {length}.");
        if emit_hint {
            text.push_str(&format!(" Continue with read({next})."));
        }
        result.content.push(agent_types::ContentPart::Text { text });
        result.metadata.insert("continuation".into(), next);
    }
    Ok(result)
}

async fn directory_page(
    args: &serde_json::Value,
    directory: &std::path::Path,
    path: &str,
    ctx: &ToolExecutionContext,
    limit: Option<usize>,
    emit_hint: bool,
) -> std::result::Result<ToolResult, ToolError> {
    if args.get("byte_offset").is_some()
        || args.get("start_line").is_some()
        || args.get("after").is_some_and(|value| !value.is_string())
    {
        return Ok(ToolResult::error(
            &ctx.trace_id,
            "Directory pages use after (a filename) and limit.",
        ));
    }
    let after = args.get("after").and_then(|value| value.as_str());
    let limit = limit.unwrap_or(100).min(200);
    let mut names = std::collections::BTreeSet::new();
    let mut entries = fs::read_dir(directory).await?;
    let mut omitted = 0;
    while let Some(entry) = entries.next_entry().await? {
        if ctx.is_cancelled() {
            return Ok(ToolResult::error(&ctx.trace_id, "Directory read cancelled"));
        }
        let Ok(name) = entry.file_name().into_string() else {
            omitted += 1;
            continue;
        };
        if after.is_some_and(|last| name.as_str() <= last) {
            continue;
        }
        if names.len() > limit && names.last().is_some_and(|last| &name >= last) {
            continue;
        }
        if ctx.workspace_fs.resolve_host_path(&entry.path()).is_err() {
            omitted += 1;
            continue;
        }
        names.insert(name);
        if names.len() > limit + 1 {
            names.pop_last();
        }
    }
    let has_more = names.len() > limit;
    if has_more {
        names.pop_last();
    }
    let mut listing = Vec::with_capacity(names.len());
    let mut entries = Vec::with_capacity(names.len());
    for name in &names {
        match fs::symlink_metadata(directory.join(name)).await {
            Ok(meta) => {
                let kind = if meta.is_dir() {
                    "directory"
                } else if meta.is_symlink() {
                    "symlink"
                } else if meta.is_file() {
                    "file"
                } else {
                    "other"
                };
                listing.push(if meta.is_dir() {
                    format!("{name}/")
                } else {
                    name.clone()
                });
                entries.push(serde_json::json!({"name":name,"kind":kind}));
            }
            Err(_) => omitted += 1,
        }
    }
    let mut result = ToolResult::text(&ctx.trace_id, listing.join("\n"));
    result
        .metadata
        .insert("directory_entries".into(), entries.into());
    if has_more {
        let next = serde_json::json!({"file_path":path,"limit":limit,"after":names.last()});
        if emit_hint {
            result.content.push(agent_types::ContentPart::Text {
                text: format!("\nContinue with read({next})."),
            });
        }
        result.metadata.insert("continuation".into(), next);
    }
    if omitted > 0 {
        result.content.push(agent_types::ContentPart::Text {text: format!("\nIncomplete listing: {omitted} inaccessible or unrepresentable entries were omitted.")});
        result
            .metadata
            .insert("omitted_entries".into(), serde_json::json!(omitted));
    }
    Ok(result)
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        crate::device_file::call("read", args, ctx).await
    }
}
