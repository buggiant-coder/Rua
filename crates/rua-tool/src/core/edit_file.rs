use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::Result;
use tokio::fs;

use crate::annotations::ExtendedToolAnnotations;
use crate::context::ToolExecutionContext;
use crate::error::ToolError;
use crate::fs::{
    atomic_write, detect_newline_style, normalize_newlines, NewlineStyle, WorkspaceFs,
};
use crate::schema::ToolSpec;

pub struct EditFileTool;

impl EditFileTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Apply a Codex-style file patch. Supports adding new files and updating existing files. Deleting, moving, or renaming files is not supported. All hunks are verified before any file is written. The patch must start with `*** Begin Patch` and end with `*** End Patch`. Every Update File line must start with a space for unchanged context, `+` for an addition, or `-` for a deletion; the first change block may optionally start with `@@`. Use small, unique context from the current file. If expected lines are not found, re-read the target region and retry with a smaller patch.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Patch text using Codex apply_patch syntax. This field is a JSON string: to place a literal backslash-n escape in source code, encode it as `\\\\n` so it does not become an unprefixed patch line. Every unchanged, added, or deleted line in an Update File block must begin with ` `, `+`, or `-` respectively. Example: `*** Begin Patch\\n*** Update File: path\\n@@\\n-old\\n+new\\n*** End Patch`"
                    }
                },
                "required": ["patch"]
            }),
            annotations: ExtendedToolAnnotations::destructive_file(),
            namespace: "builtin".into(),
        }
    }

    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        let patch = args["patch"]
            .as_str()
            .ok_or_else(|| ToolError::SchemaValidation {
                tool: "edit".into(),
                message: "missing or invalid 'patch' field".into(),
            })?;

        let hunks = match parse_patch(patch) {
            Ok(hunks) => hunks,
            Err(message) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("edit patch verification failed: {message}"),
                ));
            }
        };

        let prepared = match prepare_patch_hunks(&hunks, ctx).await {
            Ok(prepared) => prepared,
            Err(PatchApplyError::UserVisible(message)) => {
                return Ok(ToolResult::error(&ctx.trace_id, message));
            }
            Err(PatchApplyError::Tool(error)) => return Err(error),
            Err(PatchApplyError::Commit(_)) => unreachable!("preflight cannot commit files"),
        };

        let summary = match commit_patch_hunks(&prepared).await {
            Ok(summary) => summary,
            Err(PatchApplyError::UserVisible(message)) => {
                return Ok(ToolResult::error(&ctx.trace_id, message));
            }
            Err(PatchApplyError::Tool(error)) => return Err(error),
            Err(PatchApplyError::Commit(failure)) => {
                let mut result = ToolResult::error(&ctx.trace_id, failure.model_message());
                result.metadata.insert(
                    "file_edit_committed_paths".to_string(),
                    serde_json::json!(failure.committed),
                );
                return Ok(result);
            }
        };

        Ok(ToolResult::text(&ctx.trace_id, summary))
    }
}

#[derive(Debug, Clone)]
enum PatchHunk {
    Add {
        path: String,
        content: String,
    },
    Update {
        path: String,
        chunks: Vec<UpdateChunk>,
    },
}

#[derive(Debug, Clone, Default)]
struct UpdateChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
}

#[derive(Debug)]
enum PatchApplyError {
    UserVisible(String),
    Tool(ToolError),
    Commit(PatchCommitFailure),
}

impl From<ToolError> for PatchApplyError {
    fn from(value: ToolError) -> Self {
        Self::Tool(value)
    }
}

#[derive(Debug)]
struct PatchCommitFailure {
    message: String,
    committed: Vec<String>,
}

impl PatchCommitFailure {
    fn model_message(&self) -> String {
        if self.committed.is_empty() {
            return format!(
                "edit commit failed before any file was changed: {}",
                self.message
            );
        }

        format!(
            "edit commit failed after partially applying the patch: {}\n\
             Already updated files:\n{}\n\
             Do not retry the full patch without re-reading these files.",
            self.message,
            self.committed.join("\n")
        )
    }
}

#[derive(Debug)]
enum PreparedPatchHunk {
    Add {
        path: String,
        resolved: PathBuf,
        content: String,
    },
    Update {
        path: String,
        resolved: PathBuf,
        expected_content: String,
        new_content: String,
    },
}

fn parse_patch(patch: &str) -> std::result::Result<Vec<PatchHunk>, String> {
    let lines: Vec<&str> = patch
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err("The first line of the patch must be '*** Begin Patch'.".to_string());
    }

    let mut idx = 1;
    let mut hunks = Vec::new();
    let mut saw_end = false;
    while idx < lines.len() {
        let line = lines[idx];
        if line == "*** End Patch" {
            saw_end = true;
            idx += 1;
            break;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let (hunk, next_idx) = parse_add_file(path, &lines, idx + 1)?;
            hunks.push(hunk);
            idx = next_idx;
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            validate_patch_path(path)?;
            return Err(format!("Deleting files is not supported by edit: {path}."));
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let (hunk, next_idx) = parse_update_file(path, &lines, idx + 1)?;
            hunks.push(hunk);
            idx = next_idx;
        } else {
            return Err(format!("Unexpected line in patch: '{line}'."));
        }
    }

    if !saw_end {
        return Err("The last line of the patch must be '*** End Patch'.".to_string());
    }
    if lines[idx..].iter().any(|line| !line.trim().is_empty()) {
        return Err("Unexpected content after '*** End Patch'.".to_string());
    }
    if hunks.is_empty() {
        return Err("No files were modified.".to_string());
    }

    Ok(hunks)
}

fn parse_add_file(
    path: &str,
    lines: &[&str],
    mut idx: usize,
) -> std::result::Result<(PatchHunk, usize), String> {
    validate_patch_path(path)?;

    let mut content = String::new();
    let mut saw_line = false;
    while idx < lines.len() && !is_patch_boundary(lines[idx]) {
        let line = lines[idx];
        let Some(rest) = line.strip_prefix('+') else {
            return Err(format!(
                "Unexpected line in add hunk for {path:?}: every content line must start with '+'."
            ));
        };
        content.push_str(rest);
        content.push('\n');
        saw_line = true;
        idx += 1;
    }

    if !saw_line {
        return Err(format!(
            "Add hunk for {path:?} must contain at least one content line."
        ));
    }

    Ok((
        PatchHunk::Add {
            path: path.to_string(),
            content,
        },
        idx,
    ))
}

fn parse_update_file(
    path: &str,
    lines: &[&str],
    mut idx: usize,
) -> std::result::Result<(PatchHunk, usize), String> {
    validate_patch_path(path)?;

    if idx < lines.len() {
        if let Some(dest) = lines[idx].strip_prefix("*** Move to: ") {
            validate_patch_path(dest)?;
            return Err(format!(
                "Moving or renaming files is not supported by edit: {path} -> {dest}."
            ));
        }
    }

    let mut chunks = Vec::new();
    while idx < lines.len() && !is_patch_boundary(lines[idx]) {
        let line = lines[idx];
        let first_chunk_without_header = chunks.is_empty() && is_update_line(line);
        if !line.starts_with("@@") && !first_chunk_without_header {
            return Err(format!(
                "Unexpected line found in update hunk for {path:?}: '{line}'. Every update line must start with ' ', '+', or '-'. If a source string contains a literal `\\n`, encode it as `\\\\n` in the JSON `patch` argument."
            ));
        }

        let mut chunk = UpdateChunk {
            change_context: if first_chunk_without_header || line == "@@" {
                None
            } else {
                line.strip_prefix("@@ ")
                    .map(|value| value.to_string())
                    .or_else(|| Some(line.trim_start_matches("@@").trim().to_string()))
                    .filter(|value| !value.is_empty())
            },
            ..UpdateChunk::default()
        };
        if !first_chunk_without_header {
            idx += 1;
        }

        while idx < lines.len() && !is_patch_boundary(lines[idx]) && !lines[idx].starts_with("@@") {
            let line = lines[idx];
            if line == "*** End of File" {
                chunk.is_end_of_file = true;
                idx += 1;
                continue;
            }

            if let Some(rest) = line.strip_prefix(' ') {
                chunk.old_lines.push(rest.to_string());
                chunk.new_lines.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix('-') {
                chunk.old_lines.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix('+') {
                chunk.new_lines.push(rest.to_string());
            } else {
                return Err(format!(
                    "Unexpected line found in update hunk: '{line}'. Every update line must start with ' ', '+', or '-'. If a source string contains a literal `\\n`, encode it as `\\\\n` in the JSON `patch` argument."
                ));
            }
            idx += 1;
        }

        if chunk.old_lines.is_empty() && chunk.new_lines.is_empty() {
            return Err(format!(
                "Update block for {path:?} does not change any lines."
            ));
        }
        chunks.push(chunk);
    }

    if chunks.is_empty() {
        return Err(format!(
            "Update hunk for {path:?} must contain a change block."
        ));
    }

    Ok((
        PatchHunk::Update {
            path: path.to_string(),
            chunks,
        },
        idx,
    ))
}

fn is_update_line(line: &str) -> bool {
    line.starts_with([' ', '+', '-'])
}

fn is_patch_boundary(line: &str) -> bool {
    line == "*** End Patch"
        || line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
}

fn validate_patch_path(path: &str) -> std::result::Result<(), String> {
    if path.trim().is_empty() {
        return Err("Patch file paths cannot be empty.".to_string());
    }
    Ok(())
}

async fn prepare_patch_hunks(
    hunks: &[PatchHunk],
    ctx: &ToolExecutionContext,
) -> std::result::Result<Vec<PreparedPatchHunk>, PatchApplyError> {
    let mut prepared = Vec::with_capacity(hunks.len());
    let mut virtual_contents = HashMap::<PathBuf, String>::new();

    for hunk in hunks {
        match hunk {
            PatchHunk::Add { path, content } => {
                check_write_size(path, content, ctx)?;
                let resolved = ctx.workspace_fs.resolve_for_create(path)?;
                if resolved.exists() || virtual_contents.contains_key(&resolved) {
                    return Err(PatchApplyError::UserVisible(format!(
                        "Error: The file {path} already exists."
                    )));
                }
                virtual_contents.insert(resolved.clone(), content.clone());
                prepared.push(PreparedPatchHunk::Add {
                    path: path.clone(),
                    resolved,
                    content: content.clone(),
                });
            }
            PatchHunk::Update { path, chunks } => {
                let create_path = ctx.workspace_fs.resolve_for_create(path)?;
                let (resolved, original) = if let Some(content) = virtual_contents.get(&create_path)
                {
                    (create_path, content.clone())
                } else {
                    let resolved = resolve_existing_file(path, ctx).await?;
                    let original = if let Some(content) = virtual_contents.get(&resolved) {
                        content.clone()
                    } else {
                        read_text_file(path, &resolved).await?.0
                    };
                    (resolved, original)
                };
                let newline_style = detect_newline_style(&original);
                let mut new_content = apply_update_chunks(path, &original, chunks)
                    .map_err(PatchApplyError::UserVisible)?;
                new_content = normalize_newlines(&new_content, newline_style);
                check_write_size(path, &new_content, ctx)?;
                virtual_contents.insert(resolved.clone(), new_content.clone());
                prepared.push(PreparedPatchHunk::Update {
                    path: path.clone(),
                    resolved,
                    expected_content: original,
                    new_content,
                });
            }
        }
    }

    Ok(prepared)
}

async fn commit_patch_hunks(
    hunks: &[PreparedPatchHunk],
) -> std::result::Result<String, PatchApplyError> {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut committed = Vec::new();

    for hunk in hunks {
        match hunk {
            PreparedPatchHunk::Add {
                path,
                resolved,
                content,
            } => {
                match fs::symlink_metadata(resolved).await {
                    Ok(_) => {
                        return Err(commit_failure(
                            format!("the file {path} was created after patch verification"),
                            committed,
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(commit_failure(
                            format!("failed to verify add target {path}: {error}"),
                            committed,
                        ));
                    }
                }
                if let Err(error) = atomic_write(resolved, content.as_bytes(), true).await {
                    return Err(commit_failure(error.to_string(), committed));
                }
                update_recorded_mtime(resolved).await;
                added.push(path.clone());
                committed.push(format!("A {path}"));
            }
            PreparedPatchHunk::Update {
                path,
                resolved,
                expected_content,
                new_content,
            } => {
                let current_content = match read_text_file(path, resolved).await {
                    Ok((content, _)) => content,
                    Err(PatchApplyError::UserVisible(message)) => {
                        return Err(commit_failure(message, committed));
                    }
                    Err(PatchApplyError::Tool(error)) => {
                        return Err(commit_failure(error.to_string(), committed));
                    }
                    Err(PatchApplyError::Commit(_)) => {
                        unreachable!("reading a file cannot commit changes")
                    }
                };
                if current_content != *expected_content {
                    return Err(commit_failure(
                        format!("the file {path} changed after patch verification"),
                        committed,
                    ));
                }
                if let Err(error) = atomic_write(resolved, new_content.as_bytes(), false).await {
                    return Err(commit_failure(error.to_string(), committed));
                }
                update_recorded_mtime(resolved).await;
                modified.push(path.clone());
                committed.push(format!("M {path}"));
            }
        }
    }

    Ok(format_patch_summary(&added, &modified))
}

fn commit_failure(message: String, committed: Vec<String>) -> PatchApplyError {
    PatchApplyError::Commit(PatchCommitFailure { message, committed })
}

async fn resolve_existing_file(
    path: &str,
    ctx: &ToolExecutionContext,
) -> std::result::Result<PathBuf, PatchApplyError> {
    let resolved = match ctx.workspace_fs.resolve(path) {
        Ok(path) => path,
        Err(ToolError::FileNotFound { .. }) => {
            return Err(PatchApplyError::UserVisible(format!(
                "Error: The file {path} does not exist."
            )));
        }
        Err(error) => return Err(PatchApplyError::Tool(error)),
    };

    let metadata = fs::metadata(&resolved).await.map_err(ToolError::Io)?;
    if !metadata.is_file() {
        return Err(PatchApplyError::UserVisible(format!(
            "Error: The path {path} is not a file."
        )));
    }
    Ok(resolved)
}

async fn read_text_file(
    display_path: &str,
    path: &Path,
) -> std::result::Result<(String, NewlineStyle), PatchApplyError> {
    let raw = fs::read(path).await.map_err(ToolError::Io)?;
    let check_len = raw.len().min(8192);
    if raw[..check_len].contains(&0) {
        return Err(PatchApplyError::Tool(ToolError::BinaryFile {
            path: PathBuf::from(display_path),
        }));
    }

    let content = String::from_utf8_lossy(&raw).into_owned();
    let newline_style = detect_newline_style(&content);
    Ok((content, newline_style))
}

fn check_write_size(
    path: &str,
    content: &str,
    ctx: &ToolExecutionContext,
) -> std::result::Result<(), PatchApplyError> {
    let size = content.len() as u64;
    if size > ctx.limits.max_edit_file_bytes {
        return Err(PatchApplyError::Tool(ToolError::FileTooLarge {
            path: PathBuf::from(path),
            size,
            max: ctx.limits.max_edit_file_bytes,
        }));
    }
    Ok(())
}

async fn update_recorded_mtime(path: &Path) {
    if let Ok(meta) = fs::metadata(path).await {
        if let Ok(new_mtime) = meta.modified() {
            super::read_file::update_file_mtime(path, new_mtime);
        }
    }
}

fn apply_update_chunks(
    path: &str,
    original: &str,
    chunks: &[UpdateChunk],
) -> std::result::Result<String, String> {
    let had_trailing_newline = original.ends_with('\n');
    let mut lines: Vec<String> = original.split('\n').map(str::to_string).collect();
    if had_trailing_newline {
        lines.pop();
    }

    let mut cursor = 0usize;
    for chunk in chunks {
        let mut anchored_by_context = false;
        if let Some(context) = &chunk.change_context {
            let idx = match find_sequence(&lines, std::slice::from_ref(context), cursor, false) {
                MatchResult::Found(idx) => idx,
                MatchResult::NotFound => {
                    return Err(format!("Failed to find context '{context}' in {path}."));
                }
                MatchResult::Ambiguous => {
                    return Err(format!(
                        "Context '{context}' matched multiple locations in {path}; include more surrounding context."
                    ));
                }
            };
            cursor = idx + 1;
            anchored_by_context = true;
        }

        if chunk.old_lines.is_empty() {
            let insertion_idx = if anchored_by_context {
                cursor.min(lines.len())
            } else {
                lines.len()
            };
            lines.splice(insertion_idx..insertion_idx, chunk.new_lines.clone());
            cursor = insertion_idx + chunk.new_lines.len();
            continue;
        }

        let start_idx = match find_sequence(&lines, &chunk.old_lines, cursor, chunk.is_end_of_file)
        {
            MatchResult::Found(idx) => idx,
            MatchResult::NotFound => {
                return Err(format!(
                    "Failed to find expected lines in {path}:\n{}\nThe file may have changed or been formatted. Re-read the target region and retry with a smaller, unique context.",
                    chunk.old_lines.join("\n")
                ));
            }
            MatchResult::Ambiguous => {
                return Err(format!(
                    "Expected lines matched multiple locations in {path}; include more unchanged surrounding lines:\n{}",
                    chunk.old_lines.join("\n")
                ));
            }
        };

        let old_len = chunk.old_lines.len();
        lines.splice(
            start_idx..start_idx + old_len,
            chunk.new_lines.iter().cloned(),
        );
        cursor = start_idx + chunk.new_lines.len();
    }

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchResult {
    Found(usize),
    NotFound,
    Ambiguous,
}

fn find_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    end_of_file: bool,
) -> MatchResult {
    if pattern.is_empty() || pattern.len() > lines.len() {
        return MatchResult::NotFound;
    }

    if let Some(idx) = matching_indexes(lines, pattern, start, end_of_file, MatchMode::Exact).next()
    {
        return MatchResult::Found(idx);
    }

    for mode in [MatchMode::TrimEnd, MatchMode::Trim, MatchMode::Normalized] {
        let mut matches = matching_indexes(lines, pattern, start, end_of_file, mode);
        let Some(idx) = matches.next() else {
            continue;
        };
        if matches.next().is_some() {
            return MatchResult::Ambiguous;
        }
        return MatchResult::Found(idx);
    }

    MatchResult::NotFound
}

#[derive(Debug, Clone, Copy)]
enum MatchMode {
    Exact,
    TrimEnd,
    Trim,
    Normalized,
}

fn matching_indexes<'a>(
    lines: &'a [String],
    pattern: &'a [String],
    start: usize,
    end_of_file: bool,
    mode: MatchMode,
) -> impl Iterator<Item = usize> + 'a {
    let max_start = lines.len().saturating_sub(pattern.len());
    (start.min(lines.len())..=max_start).filter(move |&idx| {
        (!end_of_file || idx + pattern.len() == lines.len())
            && lines[idx..idx + pattern.len()]
                .iter()
                .zip(pattern)
                .all(|(line, expected)| lines_match(line, expected, mode))
    })
}

fn lines_match(actual: &str, expected: &str, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Exact => actual == expected,
        MatchMode::TrimEnd => actual.trim_end() == expected.trim_end(),
        MatchMode::Trim => actual.trim() == expected.trim(),
        MatchMode::Normalized => normalize_match_text(actual) == normalize_match_text(expected),
    }
}

fn normalize_match_text(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{00a0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200a}' | '\u{202f}' | '\u{205f}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn format_patch_summary(added: &[String], modified: &[String]) -> String {
    let mut out = String::from("Success. Updated the following files:\n");
    for path in added {
        out.push_str("A ");
        out.push_str(path);
        out.push('\n');
    }
    for path in modified {
        out.push_str("M ");
        out.push_str(path);
        out.push('\n');
    }
    out
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let tool_result_store_dir = crate::result_store::archive_dir_for_context(&ctx)?;
        let cancel = ctx.cancel.unwrap_or_else(|| {
            let (_, rx) = tokio::sync::watch::channel(false);
            rx
        });

        let workspace_fs =
            Arc::new(
                WorkspaceFs::new(&ctx.working_dir).map_err(|e| rua_core::RuaError::Tool {
                    tool: "edit".into(),
                    message: e.to_string(),
                })?,
            );

        let exec_ctx = ToolExecutionContext {
            workspace_fs,
            tool_result_store_dir,
            working_dir: ctx.working_dir,
            session_id: ctx.session_id,
            user_id: String::new(),
            agent_id: ctx.agent_id,
            channel: "console".into(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            cancel,
            security_context: crate::context::SecurityContext::default(),
            executor: Arc::new(crate::executor::DisabledCommandExecutor::default()),
            task_session: None,
            limits: crate::context::ToolExecutionLimits::default(),

            execution_output_max_tokens: rua_core::tool::DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
            raw_capture_max_bytes: rua_core::tool::DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
            recall_slice_max_tokens: rua_core::tool::DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,

            old_max_bytes: rua_core::tool::DEFAULT_TOOL_OLD_MAX_BYTES,

            token_estimate_divisor: rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
        };

        self.execute_with_context(args, &exec_ctx)
            .await
            .map_err(rua_core::RuaError::from)
    }
}
