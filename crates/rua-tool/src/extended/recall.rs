use async_trait::async_trait;
use serde::Deserialize;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::result_store::{
    archive_dir_for_context, ToolCallArchive, ToolResultBlob, ToolResultStore,
};
use crate::schema::ToolSpec;

#[derive(Debug, Deserialize)]
struct RecallArgs {
    recall_key: String,

    #[serde(default)]
    start_line: Option<usize>,

    #[serde(default)]
    end_line: Option<usize>,

    #[serde(default)]
    max_tokens: Option<usize>,

    #[serde(default)]
    max_bytes: Option<usize>,
}

pub struct RecallToolResultTool;

impl RecallToolResultTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "recall_tool_result".to_string(),
            description: "Retrieve the input and output of a previous tool call that was archived \
                 because its output was too long to keep inline. The original snipped \
                 marker tells you the total line count, byte size, and approximate \
                 token cost — use those numbers to decide whether to fetch the whole \
                 archive or slice it. To slice, pass `start_line` / `end_line` \
                 (1-indexed, inclusive). Use `max_tokens` to cap the model-visible \
                 slice; omitted lines are returned with continuation metadata."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "recall_key": {
                        "type": "string",
                        "description": "The recall key from the snipped tool result hint."
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "1-indexed line to start from. Defaults to 1.",
                        "minimum": 1
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "1-indexed inclusive line to end at. Defaults to the last line.",
                        "minimum": 1
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum model-visible tokens for this slice. Defaults to the current Agent Recall slice budget.",
                        "minimum": 1
                    }
                },
                "required": ["recall_key"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for RecallToolResultTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RecallToolResultTool {
    fn name(&self) -> &str {
        "recall_tool_result"
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
        let args: RecallArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "recall_tool_result".into(),
                message: e.to_string(),
            })?;

        let key = args.recall_key.trim().to_string();

        if key.is_empty() {
            return Ok(ToolResult::error(
                "recall_tool_result",
                "Error: recall_key is empty.",
            ));
        }

        let contextual_store = ToolCallArchive::new(archive_dir_for_context(&ctx)?);
        let blob = match read_recall_blob(&contextual_store, &key).await {
            Ok(blob) => blob,
            Err(_) => {
                return Ok(ToolResult::error(
                    "recall_tool_result",
                    format!(
                        "Tool call log not found for key '{key}'. It may have been cleaned up."
                    ),
                ));
            }
        };

        let divisor = ctx.token_estimate_divisor.max(1.0);
        let max_tokens = args
            .max_tokens
            .or_else(|| {
                args.max_bytes
                    .map(|bytes| ((bytes as f64) / divisor).ceil() as usize)
            })
            .unwrap_or(ctx.recall_slice_max_tokens)
            .max(1);
        let (text, truncation) = render_slice(RecallSliceRequest {
            recall_key: &key,
            tool_name: &blob.tool_name,
            input: blob.input.as_ref(),
            content: &blob.content,
            start_line: args.start_line,
            end_line: args.end_line,
            max_tokens,
            token_divisor: divisor,
        });
        let mut result = ToolResult::text("recall_tool_result", text);
        result.truncation = truncation;
        Ok(result)
    }
}

struct RecallSliceRequest<'a> {
    recall_key: &'a str,
    tool_name: &'a str,
    input: Option<&'a serde_json::Value>,
    content: &'a str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_tokens: usize,
    token_divisor: f64,
}

fn render_slice(request: RecallSliceRequest<'_>) -> (String, Option<agent_types::TruncationMeta>) {
    let mut max_bytes = ((request.max_tokens as f64) * request.token_divisor).floor() as usize;
    max_bytes = max_bytes.max(1);
    loop {
        let rendered = render_slice_with_byte_budget(&request, max_bytes);
        let visible_tokens =
            crate::result_store::estimate_text_tokens(&rendered.0, request.token_divisor);
        if visible_tokens <= request.max_tokens || max_bytes == 1 {
            return rendered;
        }
        let excess_tokens = visible_tokens.saturating_sub(request.max_tokens).max(1);
        let reduction = ((excess_tokens as f64) * request.token_divisor).ceil() as usize + 64;
        max_bytes = max_bytes.saturating_sub(reduction).max(1);
    }
}

fn render_slice_with_byte_budget(
    request: &RecallSliceRequest<'_>,
    max_bytes: usize,
) -> (String, Option<agent_types::TruncationMeta>) {
    let &RecallSliceRequest {
        recall_key,
        tool_name,
        input,
        content,
        start_line,
        end_line,
        max_tokens,
        token_divisor,
    } = request;
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let total_bytes = content.len();

    if total_lines == 0 {
        let input_pretty = format_input(input);
        return (
            format!(
                "Tool: {tool_name}\nInput: {input_pretty}\n\
                 Output (empty archive, 0/0 bytes):\n",
            ),
            None,
        );
    }

    let start_idx = start_line.map(|n| n.saturating_sub(1)).unwrap_or(0);
    let start_idx = start_idx.min(total_lines.saturating_sub(1));
    let end_idx_inclusive = end_line.unwrap_or(total_lines).min(total_lines);
    let end_idx_inclusive = end_idx_inclusive.max(start_idx + 1);

    let mut slice = String::new();
    let mut bytes_returned = 0usize;
    let mut last_line_returned = start_idx;
    for (offset, line) in lines[start_idx..end_idx_inclusive].iter().enumerate() {
        let line_size = line.len() + 1;
        if bytes_returned + line_size > max_bytes && bytes_returned > 0 {
            break;
        }
        slice.push_str(line);
        slice.push('\n');
        bytes_returned += line_size;
        last_line_returned = start_idx + offset + 1;
    }

    let truncation_meta = if last_line_returned < end_idx_inclusive {
        let next_line = last_line_returned + 1;
        let hint = format!(
            "recall_tool_result(recall_key=\"{recall_key}\", start_line={next_line}, max_tokens={max_tokens})"
        );
        let mut meta = crate::result_store::recall_meta(
            "recall_tool_result",
            Some(total_lines as u64),
            total_bytes as u64,
            crate::result_store::estimate_text_tokens(content, token_divisor) as u64,
            recall_key,
        );
        meta.continuation_hint = Some(hint);
        meta.shown_start = Some((start_idx + 1) as u64);
        meta.shown_end = Some(last_line_returned as u64);
        Some(meta)
    } else {
        None
    };

    let input_pretty = format_input(input);
    let header = format!(
        "Tool: {tool_name}\nInput: {input_pretty}\n\
         Output (lines {}-{} of {}, {}/{} bytes returned):\n",
        start_idx + 1,
        last_line_returned,
        total_lines,
        bytes_returned,
        total_bytes,
    );

    let mut text = String::with_capacity(header.len() + slice.len() + 256);
    if let Some(ref meta) = truncation_meta {
        text.push_str(&crate::result_store::format_truncation_warning(meta));
    }
    text.push_str(&header);
    text.push_str(&slice);
    (text, truncation_meta)
}

fn format_input(input: Option<&serde_json::Value>) -> String {
    match input {
        Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
        None => "{}".into(),
    }
}

fn unsuffixed_recall_key(key: &str) -> Option<&str> {
    let (prefix, suffix) = key.split_once(':')?;
    if suffix.is_empty() {
        return None;
    }
    if prefix.len() == 16 && prefix.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(prefix)
    } else {
        None
    }
}

async fn read_recall_blob(
    store: &dyn ToolResultStore,
    key: &str,
) -> Result<ToolResultBlob, crate::error::ToolError> {
    match store.read(key).await {
        Ok(blob) => Ok(blob),
        Err(error) => match unsuffixed_recall_key(key) {
            Some(fallback_key) => store.read(fallback_key).await,
            None => Err(error),
        },
    }
}
