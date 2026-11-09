use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_text_search::{regex_pattern, TextSearchQuery};

use crate::annotations::ExtendedToolAnnotations;
use crate::context::ToolExecutionContext;
use crate::error::ToolError;
use crate::schema::ToolSpec;

const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "dist",
    "build",
    ".next",
    ".nuxt",
    "vendor",
];

const DEFAULT_BINARY_EXTENSIONS: &[&str] = &[
    "7z", "a", "aac", "avi", "bin", "bmp", "class", "dll", "dmg", "doc", "docx", "dylib", "eot",
    "exe", "gif", "gz", "ico", "jar", "jpeg", "jpg", "lockb", "mov", "mp3", "mp4", "o", "ogg",
    "otf", "pdf", "png", "ppt", "pptx", "pyc", "rar", "so", "tar", "ttf", "webm", "webp", "woff",
    "woff2", "xls", "xlsx", "zip",
];

pub const MAX_REGEX_LENGTH: usize = 500;

const MAX_REGEX_REPETITION_DEPTH: usize = 3;

const DEFAULT_MAX_RESULTS: usize = 200;

const DEFAULT_CONTEXT_LINES: usize = 0;

const MAX_CONTEXT_LINES: usize = 5;

const MAX_OUTPUT_CHARS: usize = 50_000;

fn result_limit_notice(tool: &str, max_results: usize, unit: &str, next_step: &str) -> String {
    format!(
        "[result_limit tool=\"{tool}\"] max_results={max_results} reached; returned the first \
         {max_results} {unit}. To see more, {next_step}.\n\n"
    )
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    is_regex: Option<bool>,
    #[serde(default)]
    case_sensitive: Option<bool>,
    #[serde(default)]
    context_lines: Option<usize>,
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct GrepSearchTool;

impl GrepSearchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "grep".to_string(),
            description:
                "Search for a pattern in files. Respects .gitignore rules. Defaults to searching from the workspace directory."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Pattern to search for (literal string or regex)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search in. Absolute paths are supported; relative paths resolve from the workspace directory."
                    },
                    "is_regex": {
                        "type": "boolean",
                        "description": "Whether the pattern is a regex (default: false)"
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Whether the search is case sensitive (default: true)"
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Number of lines before and after each match to include",
                        "default": 0,
                        "minimum": 0,
                        "maximum": MAX_CONTEXT_LINES
                    },
                    "include": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to include"
                    },
                    "exclude": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to exclude"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of matching results to return (default: 200). If this cap is reached, the output includes a result_limit notice; increase max_results or narrow the query if needed."
                    }
                },
                "required": ["pattern"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }

    #[cfg(feature = "search")]
    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        let args: GrepArgs =
            serde_json::from_value(args).map_err(|e| ToolError::SchemaValidation {
                tool: "grep".into(),
                message: e.to_string(),
            })?;

        let is_regex = args.is_regex.unwrap_or(false);
        let case_sensitive = args.case_sensitive.unwrap_or(true);
        let context_lines = args
            .context_lines
            .unwrap_or(DEFAULT_CONTEXT_LINES)
            .min(MAX_CONTEXT_LINES);
        let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        let resolved_include = args.include.clone().filter(|patterns| !patterns.is_empty());
        let resolved_exclude = args.exclude.clone().filter(|patterns| !patterns.is_empty());

        if is_regex {
            validate_regex_complexity(&args.pattern)?;
        }

        let search_root = if let Some(ref path) = args.path {
            match ctx.workspace_fs.resolve(path) {
                Ok(p) => p,
                Err(_) => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        format!("Error: The path {path} does not exist."),
                    ));
                }
            }
        } else {
            ctx.workspace_fs.root().to_path_buf()
        };

        let pattern_str = regex_pattern(&TextSearchQuery {
            pattern: args.pattern.clone(),
            is_regex,
            case_sensitive,
        });

        let matcher = match grep_regex::RegexMatcher::new(&pattern_str) {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: Invalid regex pattern — {e}"),
                ));
            }
        };

        let mut walk_builder = ignore::WalkBuilder::new(&search_root);
        walk_builder
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b));

        let mut types_builder = ignore::types::TypesBuilder::new();
        types_builder.add_defaults();
        if let Some(ref includes) = resolved_include {
            for inc in includes {
                let _ = types_builder.add("_include", inc);
            }
            types_builder.select("_include");
        }

        if let Ok(types) = types_builder.build() {
            walk_builder.types(types);
        }

        let mut output_lines: Vec<String> = Vec::new();
        let mut total_match_count: usize = 0;
        let mut total_chars: usize = 0;
        let mut output_size_truncated = false;
        let mut result_limit_reached = false;
        let mut cancelled = false;
        let mut search_errors: Vec<String> = Vec::new();
        let mut search_error_count = 0usize;

        let walker = walk_builder.build();

        for entry in walker {
            if ctx.is_cancelled() {
                cancelled = true;
                break;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(error) => {
                    search_error_count += 1;
                    if search_errors.len() < 5 {
                        search_errors.push(error.to_string());
                    }
                    continue;
                }
            };

            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            if is_default_excluded(path) {
                continue;
            }

            if is_known_binary_file(path) {
                continue;
            }

            if let Some(ref excludes) = resolved_exclude {
                if is_excluded_by_globs(path, excludes) {
                    continue;
                }
            }

            let display_path = display_path_for(path, &search_root, ctx.workspace_fs.root());

            if let Some(ref includes) = resolved_include {
                if !is_matched_by_globs(&display_path, includes) {
                    continue;
                }
            }

            let (file_lines, file_match_count) =
                match search_file(&matcher, path, display_path.as_ref(), context_lines) {
                    Ok(result) => result,
                    Err(error) => {
                        search_error_count += 1;
                        if search_errors.len() < 5 {
                            search_errors.push(format!("{display_path}: {error}"));
                        }
                        continue;
                    }
                };

            for line in file_lines {
                let line_len = line.len() + 1;
                if total_chars + line_len > MAX_OUTPUT_CHARS {
                    output_size_truncated = true;
                    break;
                }
                output_lines.push(line);
                total_chars += line_len;
            }

            total_match_count += file_match_count;
            if total_match_count >= max_results {
                result_limit_reached = true;
                break;
            }

            if output_size_truncated {
                break;
            }
        }

        if cancelled {
            let mut body = "Search cancelled before completion.".to_string();
            if !output_lines.is_empty() {
                body.push_str("\n\n[partial results]\n");
                body.push_str(&output_lines.join("\n"));
            }
            return Ok(ToolResult::error(&ctx.trace_id, body));
        }

        if search_error_count > 0 {
            let mut body =
                format!("Search incomplete: failed to inspect {search_error_count} path(s).");
            for error in &search_errors {
                body.push_str("\n- ");
                body.push_str(error);
            }
            if search_error_count > search_errors.len() {
                body.push_str(&format!(
                    "\n- ... and {} more error(s)",
                    search_error_count - search_errors.len()
                ));
            }
            if !output_lines.is_empty() {
                body.push_str("\n\n[partial results]\n");
                body.push_str(&output_lines.join("\n"));
            }
            return Ok(ToolResult::error(&ctx.trace_id, body));
        }

        let truncation_meta = if output_size_truncated {
            let shown = output_lines.len() as u64;
            let hint = format!(
                "Truncation reason: output size limit (~{}KB). \
                 Re-run with a more specific pattern or a narrower path to fit the cap.",
                MAX_OUTPUT_CHARS / 1000
            );
            Some(crate::result_store::hard_cap_meta(
                "grep", None, shown, hint,
            ))
        } else {
            None
        };
        let output = if let Some(ref meta) = truncation_meta {
            let mut warning = crate::result_store::format_truncation_warning(meta);
            warning.push_str(&output_lines.join("\n"));
            warning
        } else if result_limit_reached {
            let mut notice = result_limit_notice(
                "grep",
                max_results,
                "matching results",
                "increase max_results or use a more specific pattern/path",
            );
            if !output_lines.is_empty() {
                notice.push_str(&output_lines.join("\n"));
            }
            notice
        } else if output_lines.is_empty() {
            format!("No matches found for pattern: {}", args.pattern)
        } else {
            output_lines.join("\n")
        };

        let mut result = ToolResult::text(&ctx.trace_id, output);
        result.truncation = truncation_meta;
        Ok(result)
    }
}

impl Default for GrepSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GrepSearchTool {
    fn name(&self) -> &str {
        "grep"
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
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        #[cfg(feature = "search")]
        {
            let tool_result_store_dir = crate::result_store::archive_dir_for_context(&_ctx)?;
            let cancel = _ctx.cancel.unwrap_or_else(|| {
                let (_, rx) = tokio::sync::watch::channel(false);
                rx
            });
            let workspace_fs =
                Arc::new(crate::fs::WorkspaceFs::new(&_ctx.working_dir).map_err(|e| {
                    rua_core::RuaError::Tool {
                        tool: "grep".into(),
                        message: e.to_string(),
                    }
                })?);
            let exec_ctx = ToolExecutionContext {
                workspace_fs,
                tool_result_store_dir,
                working_dir: _ctx.working_dir,
                session_id: _ctx.session_id,
                user_id: String::new(),
                agent_id: _ctx.agent_id,
                channel: "console".into(),
                trace_id: uuid::Uuid::new_v4().to_string(),
                cancel,
                security_context: crate::context::SecurityContext::default(),
                executor: Arc::new(crate::executor::DisabledCommandExecutor::default()),
                task_session: None,
                limits: crate::context::ToolExecutionLimits::default(),

                execution_output_max_tokens:
                    rua_core::tool::DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
                raw_capture_max_bytes: rua_core::tool::DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
                recall_slice_max_tokens: rua_core::tool::DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,

                old_max_bytes: rua_core::tool::DEFAULT_TOOL_OLD_MAX_BYTES,

                token_estimate_divisor: rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
            };
            self.execute_with_context(args, &exec_ctx)
                .await
                .map_err(rua_core::RuaError::from)
        }
        #[cfg(not(feature = "search"))]
        {
            let _ = args;
            Err(rua_core::RuaError::Tool {
                tool: "grep".into(),
                message: "search feature not enabled".into(),
            })
        }
    }
}

#[derive(Debug, Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include_hidden: Option<bool>,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct GlobSearchTool;

impl GlobSearchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern. Defaults to searching from the workspace directory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match files (e.g., '**/*.rs', 'src/**/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in. Absolute paths are supported; relative paths resolve from the workspace directory."
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "Whether to include hidden files (default: false)"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of paths to return (default: 200). If this cap is reached, the output includes a result_limit notice; increase max_results or narrow the pattern/path if needed."
                    }
                },
                "required": ["pattern"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }

    #[cfg(feature = "search")]
    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        let args: GlobArgs =
            serde_json::from_value(args).map_err(|e| ToolError::SchemaValidation {
                tool: "glob".into(),
                message: e.to_string(),
            })?;

        let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        let include_hidden = args.include_hidden.unwrap_or(false);

        let search_root = if let Some(ref path) = args.path {
            match ctx.workspace_fs.resolve(path) {
                Ok(p) => p,
                Err(_) => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        format!("Error: The path {path} does not exist."),
                    ));
                }
            }
        } else {
            ctx.workspace_fs.root().to_path_buf()
        };

        let full_pattern = search_root.join(&args.pattern);
        let pattern_str = full_pattern.to_string_lossy().to_string();

        let glob_opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: !include_hidden,
        };

        let mut results: Vec<String> = Vec::new();
        let mut truncated = false;
        let mut cancelled = false;

        match glob::glob_with(&pattern_str, glob_opts) {
            Ok(paths) => {
                for entry in paths {
                    if ctx.is_cancelled() {
                        cancelled = true;
                        break;
                    }
                    match entry {
                        Ok(path) => {
                            if is_default_excluded(&path) {
                                continue;
                            }

                            let rel = path
                                .strip_prefix(&search_root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .to_string();

                            if path.is_dir() {
                                results.push(format!("{rel}/"));
                            } else {
                                results.push(rel);
                            }
                            if results.len() >= max_results {
                                truncated = true;
                                break;
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
            Err(e) => {
                return Err(ToolError::Search {
                    message: format!("invalid glob pattern '{}': {e}", args.pattern),
                });
            }
        }

        results.sort();

        if cancelled {
            let mut body = "Search cancelled before completion.".to_string();
            if !results.is_empty() {
                body.push_str("\n\n[partial results]\n");
                body.push_str(&results.join("\n"));
            }
            return Ok(ToolResult::error(&ctx.trace_id, body));
        }

        let output = if truncated {
            let mut notice = result_limit_notice(
                "glob",
                max_results,
                "paths",
                "increase max_results or use a more specific pattern/path",
            );
            if !results.is_empty() {
                notice.push_str(&results.join("\n"));
            }
            notice
        } else if results.is_empty() {
            format!("No files matched pattern: {}", args.pattern)
        } else {
            results.join("\n")
        };

        Ok(ToolResult::text(&ctx.trace_id, output))
    }
}

impl Default for GlobSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GlobSearchTool {
    fn name(&self) -> &str {
        "glob"
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
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        #[cfg(feature = "search")]
        {
            let tool_result_store_dir = crate::result_store::archive_dir_for_context(&_ctx)?;
            let cancel = _ctx.cancel.unwrap_or_else(|| {
                let (_, rx) = tokio::sync::watch::channel(false);
                rx
            });
            let workspace_fs =
                Arc::new(crate::fs::WorkspaceFs::new(&_ctx.working_dir).map_err(|e| {
                    rua_core::RuaError::Tool {
                        tool: "glob".into(),
                        message: e.to_string(),
                    }
                })?);
            let exec_ctx = ToolExecutionContext {
                workspace_fs,
                tool_result_store_dir,
                working_dir: _ctx.working_dir,
                session_id: _ctx.session_id,
                user_id: String::new(),
                agent_id: _ctx.agent_id,
                channel: "console".into(),
                trace_id: uuid::Uuid::new_v4().to_string(),
                cancel,
                security_context: crate::context::SecurityContext::default(),
                executor: Arc::new(crate::executor::DisabledCommandExecutor::default()),
                task_session: None,
                limits: crate::context::ToolExecutionLimits::default(),

                execution_output_max_tokens:
                    rua_core::tool::DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
                raw_capture_max_bytes: rua_core::tool::DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
                recall_slice_max_tokens: rua_core::tool::DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,

                old_max_bytes: rua_core::tool::DEFAULT_TOOL_OLD_MAX_BYTES,

                token_estimate_divisor: rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
            };
            self.execute_with_context(args, &exec_ctx)
                .await
                .map_err(rua_core::RuaError::from)
        }
        #[cfg(not(feature = "search"))]
        {
            let _ = args;
            Err(rua_core::RuaError::Tool {
                tool: "glob".into(),
                message: "search feature not enabled".into(),
            })
        }
    }
}

pub fn validate_regex_complexity(pattern: &str) -> Result<(), ToolError> {
    if pattern.len() > MAX_REGEX_LENGTH {
        return Err(ToolError::RegexTooComplex);
    }

    let mut depth = 0u32;
    let mut max_depth = 0u32;
    let mut in_quantifier = false;

    for ch in pattern.chars() {
        match ch {
            '(' => {
                depth += 1;
                if in_quantifier {
                    max_depth = max_depth.max(depth);
                }
            }
            ')' => {
                depth = depth.saturating_sub(1);
            }
            '+' | '*' => {
                in_quantifier = true;
                if depth > 0 {
                    max_depth = max_depth.max(depth);
                }
            }
            _ => {
                in_quantifier = false;
            }
        }
    }

    if max_depth as usize > MAX_REGEX_REPETITION_DEPTH {
        return Err(ToolError::RegexTooComplex);
    }

    Ok(())
}

pub fn is_default_excluded(path: &std::path::Path) -> bool {
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            for excluded in DEFAULT_EXCLUDED_DIRS {
                if name_str == *excluded {
                    return true;
                }
            }
        }
    }
    false
}

fn is_known_binary_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            DEFAULT_BINARY_EXTENSIONS.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

fn is_excluded_by_globs(path: &std::path::Path, excludes: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    is_matched_by_globs(&path_str, excludes)
}

fn is_matched_by_globs(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| glob_matches(path, pattern))
}

fn glob_matches(path: &str, pattern: &str) -> bool {
    #[cfg(feature = "search")]
    {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(path))
            .unwrap_or(false)
    }
    #[cfg(not(feature = "search"))]
    {
        let _ = (path, pattern);
        false
    }
}

fn display_path_for(
    path: &std::path::Path,
    search_root: &std::path::Path,
    workspace_root: &std::path::Path,
) -> String {
    if search_root.is_file() {
        return path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
    }

    path.strip_prefix(search_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(feature = "search")]
fn search_file(
    matcher: &grep_regex::RegexMatcher,
    path: &std::path::Path,
    display_path: &str,
    context_lines: usize,
) -> std::io::Result<(Vec<String>, usize)> {
    use grep_searcher::{sinks::UTF8, Searcher};

    let mut match_lines: Vec<(u64, String)> = Vec::new();
    let mut searcher = Searcher::new();
    searcher.search_path(
        matcher,
        path,
        UTF8(|line_num, line| {
            match_lines.push((line_num, line.trim_end().to_string()));
            Ok(true)
        }),
    )?;

    let match_count = match_lines.len();
    if match_count == 0 {
        return Ok((vec![], 0));
    }

    if context_lines == 0 {
        let output: Vec<String> = match_lines
            .iter()
            .map(|(line_num, content)| format!("{display_path}:{line_num}:{content}"))
            .collect();
        return Ok((output, match_count));
    }

    let raw_bytes = std::fs::read(path)?;

    let content = String::from_utf8_lossy(&raw_bytes);
    let all_lines: Vec<&str> = content.lines().collect();
    let match_nums: std::collections::HashSet<u64> = match_lines.iter().map(|(n, _)| *n).collect();
    let match_line_nums: Vec<u64> = match_lines.iter().map(|(n, _)| *n).collect();

    let mut output_lines: Vec<String> = Vec::new();
    let mut last_output_line: Option<u64> = None;

    for &match_num in &match_line_nums {
        let idx = (match_num as usize).saturating_sub(1);
        let start_idx = idx.saturating_sub(context_lines);
        let end_idx = (idx + context_lines + 1).min(all_lines.len());

        if let Some(last) = last_output_line {
            if (start_idx as u64) + 1 > last + 1 {
                output_lines.push("---".to_string());
            }
        }

        for (j, line_content) in all_lines.iter().enumerate().take(end_idx).skip(start_idx) {
            let line_num = (j as u64) + 1;
            if let Some(last) = last_output_line {
                if line_num <= last {
                    continue;
                }
            }
            let is_match = match_nums.contains(&line_num);
            let prefix = if is_match { ">" } else { " " };
            output_lines.push(format!("{display_path}:{line_num}:{prefix} {line_content}"));
            last_output_line = Some(line_num);
        }
    }

    Ok((output_lines, match_count))
}
