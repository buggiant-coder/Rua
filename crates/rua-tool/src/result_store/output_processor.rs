use agent_types::{ContentPart, ToolResult};
use std::path::Path;

use crate::executor::RawCommandCapture;
use crate::raw_capture::RawCaptureBuffer;

use super::{
    compress_output_tokens, estimate_text_tokens, format_truncation_warning, recall_key,
    recall_meta, truncation_kind, ToolCallArchive, TruncationKind,
};

pub struct ToolResultOutputProcessor {
    logger: Option<ToolCallArchive>,
    execution_output_max_tokens: usize,
    raw_capture_max_bytes: usize,
    token_estimate_divisor: f64,
}

impl ToolResultOutputProcessor {
    pub fn from_tool_context(ctx: &rua_core::tool::ToolContext) -> Self {
        Self {
            logger: super::archive_dir_for_context(ctx)
                .ok()
                .map(ToolCallArchive::new),
            execution_output_max_tokens: ctx.execution_output_max_tokens.max(1),
            raw_capture_max_bytes: ctx.raw_capture_max_bytes.max(1),
            token_estimate_divisor: ctx.token_estimate_divisor,
        }
    }

    pub fn new(
        log_dir: impl AsRef<Path>,
        execution_output_max_tokens: usize,
        token_estimate_divisor: f64,
    ) -> Self {
        Self::new_with_limits(
            log_dir,
            execution_output_max_tokens,
            rua_core::tool::DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
            token_estimate_divisor,
        )
    }

    pub fn new_with_limits(
        log_dir: impl AsRef<Path>,
        execution_output_max_tokens: usize,
        raw_capture_max_bytes: usize,
        token_estimate_divisor: f64,
    ) -> Self {
        Self {
            logger: Some(ToolCallArchive::new(log_dir)),
            execution_output_max_tokens: execution_output_max_tokens.max(1),
            raw_capture_max_bytes: raw_capture_max_bytes.max(1),
            token_estimate_divisor,
        }
    }

    pub async fn process(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
        result: &mut ToolResult,
    ) -> bool {
        let original_text = result.text_content();
        let raw_capture = raw_command_capture(result);
        let original_bytes = raw_capture
            .as_ref()
            .map(|capture| {
                capture
                    .stdout_total_bytes
                    .saturating_add(capture.stderr_total_bytes) as usize
            })
            .unwrap_or(original_text.len());
        let original_tokens =
            ((original_bytes as f64) / self.token_estimate_divisor.max(1.0)).ceil() as u64;
        let raw_output_available_for_recall = self
            .ensure_raw_output_available(tool_use_id, tool_name, tool_input, result)
            .await;
        let raw_capture_exceeded = raw_capture.as_ref().is_some_and(|capture| {
            capture.stdout_omitted_bytes > 0 || capture.stderr_omitted_bytes > 0
        }) || original_text.len() > self.raw_capture_max_bytes;
        if tool_name != "recall_tool_result"
            && raw_capture.is_none()
            && original_text.len() > self.raw_capture_max_bytes
        {
            let mut capture = RawCaptureBuffer::new(self.raw_capture_max_bytes);
            capture.push(original_text.as_bytes());
            replace_text_content(
                result,
                String::from_utf8_lossy(&capture.to_bytes_with_marker()).into_owned(),
            );
        }
        let compacted = self.compact_tool_result_in_place(
            tool_use_id,
            tool_name,
            raw_output_available_for_recall,
            result,
        );
        let compacted = if !compacted && raw_capture_exceeded && tool_name != "recall_tool_result" {
            self.decorate_raw_capture_omission(
                tool_use_id,
                tool_name,
                raw_output_available_for_recall,
                original_bytes,
                original_tokens,
                result,
            )
        } else {
            compacted
        };
        let visible_tokens =
            estimate_text_tokens(&result.text_content(), self.token_estimate_divisor) as u64;
        let archive_key = raw_output_available_for_recall.then(|| recall_key(tool_use_id));
        result.metadata.insert(
            "tool_output".to_string(),
            serde_json::json!({
                "original_bytes": original_bytes,
                "original_tokens": original_tokens,
                "visible_tokens": visible_tokens,
                "raw_capture_max_bytes": self.raw_capture_max_bytes,
                "raw_capture_exceeded": raw_capture_exceeded,
                "archive_key": archive_key,
            }),
        );
        tracing::debug!(
            target: "diag.harness",
            tool_use_id,
            tool_name,
            original_bytes,
            original_tokens,
            visible_tokens,
            raw_capture_exceeded,
            archive_key = ?archive_key,
            "tool output bounded"
        );
        compacted
    }

    async fn ensure_raw_output_available(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
        result: &ToolResult,
    ) -> bool {
        if tool_use_id.trim().is_empty() || tool_name == "recall_tool_result" {
            remove_raw_capture_files(raw_command_capture(result)).await;
            return false;
        }

        let Some(logger) = &self.logger else {
            remove_raw_capture_files(raw_command_capture(result)).await;
            return false;
        };

        if has_recall_archive(result) {
            return true;
        }

        if logger.has_entry(tool_use_id).await {
            remove_raw_capture_files(raw_command_capture(result)).await;
            return true;
        }

        let saved = match raw_command_capture(result)
            .filter(|capture| capture.stdout_path.is_some() || capture.stderr_path.is_some())
        {
            Some(capture) => {
                let saved = logger
                    .save_captured_call(tool_use_id, tool_name, tool_input, &capture)
                    .await;
                if !capture.live_output_owned {
                    remove_raw_capture_files(Some(capture)).await;
                }
                saved
            }
            None => {
                logger
                    .save_call(tool_use_id, tool_name, tool_input, &result.text_content())
                    .await
            }
        };
        match saved {
            Ok(()) => true,
            Err(err) => {
                tracing::debug!("Failed to save tool call log: {err}");
                remove_raw_capture_files(raw_command_capture(result)).await;
                false
            }
        }
    }

    fn compact_tool_result_in_place(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        raw_output_available_for_recall: bool,
        result: &mut ToolResult,
    ) -> bool {
        if tool_name == "recall_tool_result" || has_continuation_or_recall_truncation(result) {
            return false;
        }

        let text = result.text_content();
        if estimate_text_tokens(&text, self.token_estimate_divisor)
            <= self.execution_output_max_tokens
        {
            return false;
        }

        let marker_tool_name = result.name.as_deref().unwrap_or(tool_name);
        let (compacted, truncation) = compress_output_tokens(
            &text,
            self.execution_output_max_tokens,
            marker_tool_name,
            raw_output_available_for_recall.then_some(tool_use_id),
            self.token_estimate_divisor,
        );

        let Some(truncation) = truncation else {
            return false;
        };

        let mut content = vec![ContentPart::Text { text: compacted }];
        content.extend(
            result
                .content
                .iter()
                .filter(|part| !part.is_text())
                .cloned(),
        );
        result.content = content;
        result.truncation = Some(truncation);
        true
    }

    fn decorate_raw_capture_omission(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        raw_output_available_for_recall: bool,
        original_bytes: usize,
        original_tokens: u64,
        result: &mut ToolResult,
    ) -> bool {
        let marker_tool_name = result.name.as_deref().unwrap_or(tool_name);
        let mut meta = recall_meta(
            marker_tool_name,
            Some(result.text_content().lines().count() as u64),
            original_bytes as u64,
            original_tokens,
            recall_key(tool_use_id),
        );
        if !raw_output_available_for_recall {
            meta.recall_key = None;
            meta.retry_hint = Some("Full raw output could not be archived.".to_string());
        }
        let mut text = format_truncation_warning(&meta);
        text.push_str(&result.text_content());
        replace_text_content(result, text);
        result.truncation = Some(meta);
        true
    }
}

fn has_recall_archive(result: &ToolResult) -> bool {
    result
        .truncation
        .as_ref()
        .and_then(|meta| meta.recall_key.as_ref())
        .is_some()
}

fn has_continuation_or_recall_truncation(result: &ToolResult) -> bool {
    result.truncation.as_ref().is_some_and(|meta| {
        matches!(
            truncation_kind(meta),
            TruncationKind::Continuation | TruncationKind::Recall
        )
    })
}

fn raw_command_capture(result: &ToolResult) -> Option<RawCommandCapture> {
    serde_json::from_value(result.metadata.get("raw_capture")?.clone()).ok()
}

async fn remove_raw_capture_files(capture: Option<RawCommandCapture>) {
    let Some(capture) = capture else {
        return;
    };
    if capture.live_output_owned {
        return;
    }
    for path in [capture.stdout_path, capture.stderr_path]
        .into_iter()
        .flatten()
    {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn replace_text_content(result: &mut ToolResult, text: String) {
    let mut content = vec![ContentPart::Text { text }];
    content.extend(
        result
            .content
            .iter()
            .filter(|part| !part.is_text())
            .cloned(),
    );
    result.content = content;
}
