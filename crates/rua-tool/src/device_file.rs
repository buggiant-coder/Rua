use std::path::Path;
use std::sync::Arc;

use rua_core::tool::{ToolContext, ToolResult};
use rua_core::RuaError;
use serde_json::{json, Value};

use crate::context::{SecurityContext, ToolExecutionContext, ToolExecutionLimits};
use crate::core::read_file::{OpenedRegularFile, ReadSource};
use crate::error::ToolError;
use crate::fs::WorkspaceFs;

pub(crate) fn device_id_parameter() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Stable ID of the device that holds the requested path. Omit to use the current execution device. If a location is unavailable, report the missing scope, continue independent work, and defer edits that require a complete comparison.",
    })
}

// The model adapter displays RuaError through Display. Retain the original
// category (including unit cancellation) when reporting a file operation.
fn at_location(
    mut error: RuaError,
    device_id: Option<&str>,
    paths: impl serde::Serialize,
) -> RuaError {
    let detail = match &mut error {
        RuaError::Security { message }
        | RuaError::Tool { message, .. }
        | RuaError::Runtime { message }
        | RuaError::Io { message }
        | RuaError::Validation { message } => message,
        RuaError::Timeout { operation } => operation,
        RuaError::Conflict { detail, .. } => detail,
        _ => return error,
    };
    let location = json!({"device_id": device_id, "paths": paths, "coverage": "unavailable"});
    detail.push_str(&format!("\nLocation {location}."));
    error
}

pub(crate) async fn call(
    tool: &'static str,
    args: Value,
    ctx: ToolContext,
) -> rua_core::Result<ToolResult> {
    call_local(tool, args, ctx, None).await
}

pub(crate) async fn read_opened_file(
    args: Value,
    ctx: ToolContext,
    file: OpenedRegularFile,
) -> rua_core::Result<ToolResult> {
    call_local("read", args, ctx, Some(file)).await
}

async fn call_local(
    tool: &'static str,
    args: Value,
    ctx: ToolContext,
    opened: Option<OpenedRegularFile>,
) -> rua_core::Result<ToolResult> {
    let target = args.get("device_id");
    let call_id = ctx
        .metadata
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let path_key = if tool == "read" { "file_path" } else { "path" };
    let path = args.get(path_key).and_then(Value::as_str);
    let paths: Vec<_> = if tool == "edit" {
        args["patch"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                line.strip_prefix("*** Add File: ")
                    .or_else(|| line.strip_prefix("*** Update File: "))
            })
            .collect()
    } else {
        path.into_iter().collect()
    };
    if let Some(device) = target {
        let requested = device.as_str().filter(|id| !id.trim().is_empty());
        let execution = ctx
            .local_device_id
            .as_deref()
            .filter(|id| !id.trim().is_empty());
        let problem = match (requested, execution) {
            (None, _) => Some("device_id must be a nonempty string"),
            (_, None) => Some("the execution device ID is unknown"),
            (Some(target), Some(local)) if target != local => {
                Some("this file tool has no route to the requested device")
            }
            _ => None,
        };
        if let Some(problem) = problem {
            let mut location = json!({
                "device_id": requested, "execution_device_id": execution,
                "paths": paths, "path": paths.first(), "coverage": "unavailable", "dispatched": false,
            });
            let mut reason = problem.to_owned();
            if opened.is_none()
                && requested.is_some()
                && execution.is_some()
                && requested != execution
            {
                if let Some(access) = &ctx.device_read_access {
                    use rua_core::tool::DeviceReadCheck;
                    let (status, detail) = match access.check_operation(tool, &ctx, &args).await? {
                        DeviceReadCheck::Shared(page) => {
                            return Ok(shared_read_result(&call_id, &args, &ctx, location, page));
                        }
                        DeviceReadCheck::Executed(mut result) => {
                            result.call_id = call_id.clone();
                            result.name = Some(tool.into());
                            let actual = result.metadata.entry("file_location")
                                .or_insert_with(|| location.clone());
                            actual["device_id"] = json!(requested);
                            actual["execution_device_id"] = json!(execution);
                            actual["dispatched"] = json!(true);
                            return Ok(*result);
                        }
                        DeviceReadCheck::OutcomeUnconfirmed(detail) => {
                            location["outcome"] = json!("unconfirmed");
                            location["dispatched"] = json!(true);
                            let mut result = ToolResult::error(&call_id, format!(
                                "{tool} on the target device has an unconfirmed outcome. {detail} Check the target files before retrying. Location {location}."
                            ));
                            result.metadata.insert("file_location".into(), location);
                            return Ok(result);
                        }
                        DeviceReadCheck::FileNotAuthorized => {
                            location["file_delivery"] = json!("not_authorized");
                            ("checked", "the target device checked this request. It has no matching authorization to return this file to the chat, so the file was not opened".into())
                        }
                        DeviceReadCheck::FileUnavailable => {
                            location["dispatched"] = json!(true);
                            let mut result = ToolResult::error(&call_id, format!(
                                "The target device could not complete {tool} for the requested path. Location {location}."
                            ));
                            result.metadata.insert("file_location".into(), location);
                            return Ok(result);
                        }
                        DeviceReadCheck::Denied(detail) => ("denied", detail),
                        DeviceReadCheck::Unavailable(detail) => ("unavailable", detail),
                        DeviceReadCheck::ConfirmationRequired { permission_saved } => ("confirmation_required", if permission_saved {
                            "your read permission was saved, but the target policy still requires confirmation for this read"
                        } else {
                            "the target policy requires the user's confirmation for this read"
                        }.into()),
                        DeviceReadCheck::Checked => ("checked", "the target checked the requested path against its policy; no file content was read".into()),
                    };
                    location["target_permission_check"] = json!(status);
                    reason = detail;
                }
            }
            let mut result = ToolResult::error(
                &call_id,
                format!("{tool} was not dispatched: {reason}. Location {location}. Report this missing scope and defer edits that depend on it."),
            );
            result.metadata.insert("file_location".into(), location);
            return Ok(result);
        }
    }
    let device_id = ctx.local_device_id.as_deref();
    let requested_error = |error| at_location(error, device_id, &paths);
    let source = if let Some(file) = opened {
        ReadSource::Opened(file)
    } else if tool == "read" {
        let media = match path {
            Some(path) => ctx
                .open_chat_media_reference(path)
                .await
                .map_err(requested_error)?,
            None => None,
        };
        media.map_or(ReadSource::Path, ReadSource::Media)
    } else {
        ReadSource::Path
    };
    let root = if matches!(source, ReadSource::Media(_)) {
        WorkspaceFs::new(&ctx.working_dir)
    } else {
        WorkspaceFs::for_execution(
            &ctx.working_dir,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
    }
    .map_err(|error| requested_error(error.into()))?;
    let paths: Vec<_> = if paths.is_empty() {
        vec![root.public_path(Path::new(""))]
    } else {
        paths
            .iter()
            .map(|path| {
                if rua_core::chat_media::reference_id(path).is_some() {
                    std::path::PathBuf::from(path)
                } else {
                    root.public_path(Path::new(path))
                }
            })
            .collect()
    };
    let resolved_error = |error| at_location(error, device_id, &paths);
    let search_args = matches!(tool, "grep" | "glob").then(|| args.clone());
    let exec_ctx = ToolExecutionContext {
        tool_result_store_dir: crate::result_store::archive_dir_for_context(&ctx)
            .map_err(resolved_error)?,
        workspace_fs: Arc::new(root),
        working_dir: ctx.working_dir,
        session_id: ctx.session_id,
        user_id: String::new(),
        agent_id: ctx.agent_id,
        channel: "console".into(),
        trace_id: call_id,
        cancel: ctx
            .cancel
            .unwrap_or_else(|| tokio::sync::watch::channel(false).1),
        security_context: SecurityContext::default(),
        executor: Arc::new(crate::executor::DisabledCommandExecutor::default()),
        task_session: None,
        limits: ToolExecutionLimits::default(),
        execution_output_max_tokens: ctx.execution_output_max_tokens,
        raw_capture_max_bytes: ctx.raw_capture_max_bytes,
        recall_slice_max_tokens: ctx.recall_slice_max_tokens,
        old_max_bytes: ctx.old_max_bytes,
        token_estimate_divisor: ctx.token_estimate_divisor,
    };
    let mut result = match tool {
        "read" => {
            crate::ReadFileTool
                .read_from(
                    args,
                    &exec_ctx,
                    source,
                    device_id.zip(paths.first().map(|p| p.as_path())),
                )
                .await
        }
        "edit" => {
            crate::EditFileTool
                .execute_with_context(args, &exec_ctx)
                .await
        }
        #[cfg(feature = "search")]
        "grep" => {
            crate::extended::search::GrepSearchTool
                .execute_with_context(args, &exec_ctx)
                .await
        }
        #[cfg(feature = "search")]
        "glob" => {
            crate::extended::search::GlobSearchTool
                .execute_with_context(args, &exec_ctx)
                .await
        }
        _ => Err(ToolError::Internal(format!(
            "{tool} is unavailable in this build"
        ))),
    }
    .map_err(|error| resolved_error(error.into()))?;
    let partial = result.truncation.is_some()
        || result.metadata.contains_key("omitted_entries")
        || result.metadata.contains_key("continuation")
        || result.metadata.get("more_results") == Some(&Value::Bool(true));
    let coverage = match (tool, result.is_error, partial) {
        (_, _, true) => "partial",
        ("read", true, false) => "unavailable",
        // Searches can return matches and edits can commit files before an error.
        (_, true, false) => "partial",
        (_, false, false) => "requested_scope",
    };
    let location = json!({"device_id":ctx.local_device_id,"path":paths.first(),"paths":paths,"coverage":coverage});
    if let (Some(mut query), Some(offset)) =
        (search_args, result.metadata.get("next_offset").cloned())
    {
        query["offset"] = offset;
        result.metadata.insert("continuation".into(), query);
    }
    if let Some(next) = result
        .metadata
        .get_mut("continuation")
        .and_then(Value::as_object_mut)
    {
        next.insert(path_key.into(), json!(paths.first()));
        if let Some(device) = &ctx.local_device_id {
            next.insert("device_id".into(), json!(device));
        }
    }
    if ctx.local_device_id.is_some() {
        let read_hint_in_body = tool == "read"
            && result
                .truncation
                .as_ref()
                .is_some_and(|meta| meta.continuation_hint.is_some());
        let suffix = match result.metadata.get("continuation") {
            Some(next) if !read_hint_in_body => {
                format!("\nLocation {location}. Continue with {tool}({next}).")
            }
            _ => format!("\nLocation {location}."),
        };
        result
            .content
            .push(agent_types::ContentPart::Text { text: suffix });
    }
    result.metadata.insert("file_location".into(), location);
    Ok(result)
}

fn shared_read_result(
    call_id: &str,
    args: &Value,
    ctx: &ToolContext,
    mut location: Value,
    page: rua_core::tool::SharedReadPage,
) -> ToolResult {
    use rua_core::tool::ReadPageContinuation;
    location["dispatched"] = json!(true);
    let start = args["start_line"].as_u64().unwrap_or(1);
    let follows_window = match (
        &page.byte_range,
        &page.continuation,
        args.get("byte_offset"),
    ) {
        (Some(range), _, Some(offset)) => offset.as_u64() == Some(range.start),
        (None, _, Some(_)) => false,
        (Some(range), _, None) if start == 1 => range.start == 0,
        (_, Some(ReadPageContinuation::Lines { start_line, limit }), _) => {
            let window = args["limit"]
                .as_u64()
                .unwrap_or(crate::context::DEFAULT_READ_LINES as u64)
                .min(crate::context::DEFAULT_MAX_READ_LINES as u64);
            *limit == window && *start_line > start && *start_line <= start.saturating_add(window)
        }
        _ => true,
    };
    let mut result = if !page.is_valid()
        || !follows_window
        || page.text.len() > ctx.raw_capture_max_bytes.max(1)
    {
        ToolResult::error(
            call_id,
            "The file page was invalid or exceeded the limit for this read.",
        )
    } else {
        location["coverage"] = json!(if page.continuation.is_some() {
            "partial"
        } else {
            "requested_scope"
        });
        let mut result = ToolResult::text(call_id, page.text);
        if let Some(range) = page.byte_range {
            result.metadata.insert("byte_range".into(), json!(range));
        }
        if let Some(next) = page.continuation {
            let mut next = match next {
                ReadPageContinuation::Lines { start_line, limit } => {
                    json!({ "start_line":start_line,"limit":limit })
                }
                ReadPageContinuation::Bytes { byte_offset } => json!({"byte_offset":byte_offset}),
            };
            next["device_id"] = args["device_id"].clone();
            next["file_path"] = args["file_path"].clone();
            result.content.push(agent_types::ContentPart::Text {
                text: format!("\nContinue with read({next})."),
            });
            result.metadata.insert("continuation".into(), next);
        }
        result
    };
    result.content.push(agent_types::ContentPart::Text {
        text: format!("\nLocation {location}."),
    });
    result.metadata.insert("file_location".into(), location);
    result
}
