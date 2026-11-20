use async_trait::async_trait;
use serde::Deserialize;
#[cfg(feature = "search")]
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

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

#[cfg(feature = "search")]
fn filter_search_entries(
    walker: &mut ignore::WalkBuilder,
    fs: &crate::fs::WorkspaceFs,
) -> Arc<AtomicUsize> {
    let omitted = Arc::new(AtomicUsize::new(0));
    let skipped = omitted.clone();
    let fs = fs.clone();
    walker.filter_entry(move |entry| {
        let path = entry.path();
        if is_default_excluded(path) {
            return false;
        }
        // Tool arguments use UTF-8. A lossy filename can identify a different file.
        if path.to_str().is_none() || fs.check_secret_dir(path).is_err() {
            skipped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    });
    omitted
}

#[cfg(feature = "search")]
fn search_result(trace_id: &str, output: String, omitted: usize, details: &[String]) -> ToolResult {
    if omitted == 0 {
        return ToolResult::text(trace_id, output);
    }
    let mut body = format!("Search incomplete: failed to inspect {omitted} path(s).");
    for detail in details {
        body.push_str("\n- ");
        body.push_str(detail);
    }
    if !output.is_empty() {
        body.push_str("\n\n[partial results]\n");
        body.push_str(&output);
    }
    let mut result = ToolResult::error(trace_id, body);
    result
        .metadata
        .insert("omitted_entries".into(), omitted.into());
    result
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
    #[serde(default)]
    offset: usize,
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
                    "device_id": crate::device_file::device_id_parameter(),
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
                    "offset": {"type":"integer","minimum":0,"description":"Skip this many matching entries in the same query. Use the returned next_offset to continue."},
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
        let ctx = ctx.clone();
        tokio::task::spawn_blocking(move || Self::search(args, &ctx))
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?
    }

    #[cfg(feature = "search")]
    fn search(
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
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, 2000);
        let resolved_include = args.include.clone().filter(|patterns| !patterns.is_empty());
        let resolved_exclude = args.exclude.clone().filter(|patterns| !patterns.is_empty());

        if is_regex {
            validate_regex_complexity(&args.pattern)?;
        }

        let search_root = if let Some(ref path) = args.path {
            match ctx.workspace_fs.resolve(path) {
                Ok(p) => p,
                Err(ToolError::FileNotFound { .. }) => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        format!("Error: The path {path} does not exist."),
                    ));
                }
                Err(error) => return Err(error),
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

        let omitted = filter_search_entries(&mut walk_builder, &ctx.workspace_fs);
        let walker = walk_builder.build();
        let mut remaining_skip = args.offset;
        let single_file_label = search_root.is_file().then(|| {
            search_root
                .strip_prefix(ctx.workspace_fs.root())
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|_| {
                    ctx.workspace_fs
                        .public_path(std::path::Path::new(args.path.as_deref().unwrap_or("")))
                })
        });

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

            let display_path = single_file_label
                .as_deref()
                .unwrap_or_else(|| path.strip_prefix(&search_root).expect("walked search root"))
                .to_string_lossy();

            if let Some(ref includes) = resolved_include {
                if !is_matched_by_globs(&display_path, includes) {
                    continue;
                }
            }

            let page = match search_file(
                &matcher,
                path,
                &display_path,
                context_lines,
                (
                    max_results.saturating_sub(total_match_count),
                    MAX_OUTPUT_CHARS.saturating_sub(total_chars),
                    remaining_skip,
                ),
                ctx,
            ) {
                Ok(page) => page,
                Err(error) => {
                    search_error_count += 1;
                    if search_errors.len() < 5 {
                        search_errors.push(format!("{display_path}: {error}"));
                    }
                    continue;
                }
            };
            remaining_skip = page.skip;
            let file_match_count = page.matches;
            total_chars += page.bytes;
            output_lines.extend(page.lines);
            output_size_truncated |= page.byte_limited;
            if ctx.is_cancelled() {
                cancelled = true;
                break;
            }
            total_match_count += file_match_count;
            if page.match_limited {
                result_limit_reached = true;
                break;
            }

            if output_size_truncated {
                break;
            }
        }

        if cancelled || ctx.is_cancelled() {
            let mut body = "Search cancelled before completion.".to_string();
            if !output_lines.is_empty() {
                body.push_str("\n\n[partial results]\n");
                body.push_str(&output_lines.join("\n"));
            }
            return Ok(ToolResult::error(&ctx.trace_id, body));
        }

        search_error_count += omitted.load(Ordering::Relaxed);
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
        } else if output_lines.is_empty() && search_error_count == 0 {
            format!("No matches found for pattern: {}", args.pattern)
        } else {
            output_lines.join("\n")
        };

        let mut result = search_result(&ctx.trace_id, output, search_error_count, &search_errors);
        result.truncation = truncation_meta;
        if result_limit_reached || output_size_truncated {
            result.metadata.insert("more_results".into(), true.into());
            if total_match_count > 0 {
                let next = args.offset.saturating_add(total_match_count);
                result.metadata.insert("next_offset".into(), next.into());
                result.content.push(agent_types::ContentPart::Text {
                    text: format!("\nContinue the same grep query with offset={next}."),
                });
            }
        }
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
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        crate::device_file::call("grep", args, ctx).await
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
    #[serde(default)]
    offset: usize,
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
                    "device_id": crate::device_file::device_id_parameter(),
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
                    "offset": {"type":"integer","minimum":0,"description":"Skip this many matching entries in the same query. Use the returned next_offset to continue."},
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
        let ctx = ctx.clone();
        tokio::task::spawn_blocking(move || Self::search(args, &ctx))
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?
    }

    #[cfg(feature = "search")]
    fn search(
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        let args: GlobArgs =
            serde_json::from_value(args).map_err(|e| ToolError::SchemaValidation {
                tool: "glob".into(),
                message: e.to_string(),
            })?;

        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, 2000);
        let include_hidden = args.include_hidden.unwrap_or(false);

        let search_root = if let Some(ref path) = args.path {
            match ctx.workspace_fs.resolve(path) {
                Ok(p) => p,
                Err(ToolError::FileNotFound { .. }) => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        format!("Error: The path {path} does not exist."),
                    ));
                }
                Err(error) => return Err(error),
            }
        } else {
            ctx.workspace_fs.root().to_path_buf()
        };

        if std::path::Path::new(&args.pattern).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Use the path argument for the search root and a pattern inside it.",
            ));
        }
        let pattern = glob::Pattern::new(&args.pattern).map_err(|e| ToolError::Search {
            message: format!("invalid glob pattern '{}': {e}", args.pattern),
        })?;
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: !include_hidden,
        };
        let mut results = Vec::new();
        let mut truncated = false;
        let mut cancelled = false;
        let mut bytes = 0;
        let mut byte_limited = false;
        let mut failed = 0;
        let mut remaining_skip = args.offset;
        let mut builder = ignore::WalkBuilder::new(&search_root);
        builder
            .standard_filters(false)
            .follow_links(false)
            .hidden(!include_hidden)
            .sort_by_file_path(|left, right| left.cmp(right));
        if !args.pattern.contains("**") {
            builder.max_depth(Some(
                std::path::Path::new(&args.pattern).components().count(),
            ));
        }
        let omitted = filter_search_entries(&mut builder, &ctx.workspace_fs);
        for entry in builder.build() {
            if ctx.is_cancelled() {
                cancelled = true;
                break;
            }
            let entry = match entry {
                Ok(entry) if entry.depth() == 0 => continue,
                Ok(entry) => entry,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(&search_root) else {
                continue;
            };
            if !pattern.matches_path_with(relative, options) {
                continue;
            }
            if ctx.workspace_fs.resolve_host_path(path).is_err() {
                failed += 1;
                continue;
            }
            if remaining_skip > 0 {
                remaining_skip -= 1;
                continue;
            }
            let mut name = relative.to_string_lossy().into_owned();
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                name.push('/');
            }
            byte_limited = bytes + name.len() + 1 > MAX_OUTPUT_CHARS;
            if results.len() >= max_results || byte_limited {
                truncated = true;
                break;
            }
            bytes += name.len() + 1;
            results.push(name);
        }
        if ctx.is_cancelled() {
            cancelled = true;
        }
        failed += omitted.load(Ordering::Relaxed);
        results.sort();

        if cancelled {
            let mut body = "Search cancelled before completion.".to_string();
            if !results.is_empty() {
                body.push_str("\n\n[partial results]\n");
                body.push_str(&results.join("\n"));
            }
            return Ok(ToolResult::error(&ctx.trace_id, body));
        }

        let output = if byte_limited {
            format!(
                "Output size limit ({MAX_OUTPUT_CHARS} bytes) reached.\n{}",
                results.join("\n")
            )
        } else if truncated {
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
        } else if results.is_empty() && failed == 0 {
            format!("No files matched pattern: {}", args.pattern)
        } else {
            results.join("\n")
        };

        let mut result = search_result(&ctx.trace_id, output, failed, &[]);
        if truncated {
            result.metadata.insert("more_results".into(), true.into());
            let next = args.offset.saturating_add(results.len());
            result.metadata.insert("next_offset".into(), next.into());
            result.content.push(agent_types::ContentPart::Text {
                text: format!("\nContinue the same glob query with offset={next}."),
            });
        }
        Ok(result)
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
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        crate::device_file::call("glob", args, ctx).await
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

#[cfg(feature = "search")]
struct SearchPage<'a> {
    path: &'a str,
    context: bool,
    remaining_matches: usize,
    remaining_bytes: usize,
    skip: usize,
    match_limited: bool,
    cancel: &'a tokio::sync::watch::Receiver<bool>,
    lines: Vec<String>,
    matches: usize,
    bytes: usize,
    byte_limited: bool,
}

#[cfg(feature = "search")]
impl SearchPage<'_> {
    fn push(&mut self, number: u64, bytes: &[u8], matched: bool) -> std::io::Result<bool> {
        if *self.cancel.borrow() {
            return Ok(false);
        }
        if self.skip > 0 {
            self.skip -= usize::from(matched);
            return Ok(true);
        }
        if matched && self.matches == self.remaining_matches {
            self.match_limited = true;
            return Ok(false);
        }
        if !matched && self.remaining_matches == 0 {
            return Ok(true);
        }
        let line = std::str::from_utf8(bytes)
            .map_err(std::io::Error::other)?
            .trim_end_matches(['\r', '\n']);
        let mark = if !self.context {
            ""
        } else if matched {
            "> "
        } else {
            "  "
        };
        let prefix = format!("{}:{number}:{mark}", self.path);
        let size = prefix.len() + line.len() + 1;
        if size > self.remaining_bytes.saturating_sub(self.bytes) {
            self.byte_limited = true;
            return Ok(false);
        }
        self.bytes += size;
        self.matches += usize::from(matched);
        self.lines.push(prefix + line);
        Ok(true)
    }
}

#[cfg(feature = "search")]
impl grep_searcher::Sink for SearchPage<'_> {
    type Error = std::io::Error;
    fn matched(
        &mut self,
        _: &grep_searcher::Searcher,
        hit: &grep_searcher::SinkMatch<'_>,
    ) -> std::io::Result<bool> {
        self.push(
            hit.line_number().expect("line numbers enabled"),
            hit.bytes(),
            true,
        )
    }
    fn context(
        &mut self,
        _: &grep_searcher::Searcher,
        line: &grep_searcher::SinkContext<'_>,
    ) -> std::io::Result<bool> {
        self.push(
            line.line_number().expect("line numbers enabled"),
            line.bytes(),
            false,
        )
    }
    fn context_break(&mut self, _: &grep_searcher::Searcher) -> std::io::Result<bool> {
        if self.lines.is_empty() {
            return Ok(true);
        }
        if self.bytes + 4 > self.remaining_bytes {
            self.byte_limited = true;
            return Ok(false);
        }
        self.lines.push("---".into());
        self.bytes += 4;
        Ok(true)
    }
}

#[cfg(feature = "search")]
struct SearchReader<'a> {
    input: std::fs::File,
    ctx: &'a ToolExecutionContext,
}

#[cfg(feature = "search")]
impl std::io::Read for SearchReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.ctx.is_cancelled() {
            return Err(std::io::Error::other("Search cancelled"));
        }
        self.input.read(buf)
    }
}

#[cfg(feature = "search")]
fn search_file<'a>(
    matcher: &grep_regex::RegexMatcher,
    path: &std::path::Path,
    display_path: &'a str,
    context_lines: usize,
    (remaining_matches, remaining_bytes, skip): (usize, usize, usize),
    ctx: &'a ToolExecutionContext,
) -> std::io::Result<SearchPage<'a>> {
    let mut page = SearchPage {
        path: display_path,
        context: context_lines > 0,
        remaining_matches,
        remaining_bytes,
        skip,
        match_limited: false,
        cancel: &ctx.cancel,
        lines: Vec::new(),
        matches: 0,
        bytes: 0,
        byte_limited: false,
    };
    grep_searcher::SearcherBuilder::new()
        .before_context(context_lines)
        .after_context(context_lines)
        .line_number(true)
        .heap_limit(Some(1024 * 1024))
        .binary_detection(grep_searcher::BinaryDetection::quit(0))
        .build()
        .search_reader(
            matcher,
            SearchReader {
                input: std::fs::File::open(path)?,
                ctx,
            },
            &mut page,
        )?;
    Ok(page)
}
