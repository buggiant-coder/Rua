pub mod search;

#[cfg(feature = "web")]
pub mod web_fetch;
#[cfg(feature = "web")]
pub mod web_search;

pub mod ask_user;
pub mod collaboration;
pub mod conversation;
pub mod media;

pub mod recall;
pub mod send_file;
pub mod sync_transfer;
pub mod time;
pub mod update_plan;
pub mod usage;

pub mod introspect;

#[cfg(feature = "tasks")]
pub mod task_tools;

use std::sync::Arc;

use crate::platform::ToolProfile;
use crate::registry::{ToolNamespace, ToolRegistry};

pub fn register_extended_tools(
    registry: &mut ToolRegistry,
    profile: ToolProfile,
) -> Result<(), crate::error::ToolError> {
    collaboration::register_collaboration_tools(registry)?;
    sync_transfer::register_sync_transfer_tools(registry)?;
    if profile.has_search() {
        registry.register(
            Arc::new(search::GrepSearchTool::new()),
            search::GrepSearchTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(search::GlobSearchTool::new()),
            search::GlobSearchTool::spec(),
            ToolNamespace::Builtin,
        )?;
    }

    #[cfg(feature = "web")]
    if profile.has_web() {
        registry.register(
            Arc::new(web_fetch::WebFetchTool::new()),
            web_fetch::WebFetchTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(web_search::WebSearchTool::new()),
            web_search::WebSearchTool::spec(),
            ToolNamespace::Builtin,
        )?;
    }

    registry.register(
        Arc::new(conversation::ConversationRecallTool::new()),
        conversation::ConversationRecallTool::spec(),
        ToolNamespace::Builtin,
    )?;
    registry.register(
        Arc::new(conversation::ConversationReadSpanTool::new()),
        conversation::ConversationReadSpanTool::spec(),
        ToolNamespace::Builtin,
    )?;

    registry.register(
        Arc::new(time::GetCurrentTimeTool::new()),
        time::GetCurrentTimeTool::spec(),
        ToolNamespace::Builtin,
    )?;
    registry.register(
        Arc::new(time::SetUserTimezoneTool::new()),
        time::SetUserTimezoneTool::spec(),
        ToolNamespace::Builtin,
    )?;

    #[cfg(feature = "tasks")]
    if profile.has_tasks() {
        registry.register(
            Arc::new(task_tools::TaskCreateTool::new()),
            task_tools::TaskCreateTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(task_tools::TaskSendInputTool::new()),
            task_tools::TaskSendInputTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(task_tools::TaskCancelTool::new()),
            task_tools::TaskCancelTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(task_tools::TaskRetryTool::new()),
            task_tools::TaskRetryTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(task_tools::TaskReadTool::new()),
            task_tools::TaskReadTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(task_tools::TaskListTool::new()),
            task_tools::TaskListTool::spec(),
            ToolNamespace::Builtin,
        )?;
    }

    registry.register(
        Arc::new(media::ViewImageTool::new()),
        media::ViewImageTool::spec(),
        ToolNamespace::Builtin,
    )?;
    registry.register(
        Arc::new(media::ViewVideoTool::new()),
        media::ViewVideoTool::spec(),
        ToolNamespace::Builtin,
    )?;

    registry.register(
        Arc::new(recall::RecallToolResultTool::new()),
        recall::RecallToolResultTool::spec(),
        ToolNamespace::Builtin,
    )?;

    registry.register(
        Arc::new(usage::GetTokenUsageTool::new()),
        usage::GetTokenUsageTool::spec(),
        ToolNamespace::Builtin,
    )?;

    registry.register(
        Arc::new(update_plan::UpdatePlanTool::new()),
        update_plan::UpdatePlanTool::spec(),
        ToolNamespace::Builtin,
    )?;

    registry.register(
        Arc::new(introspect::ListToolsTool),
        introspect::ListToolsTool::spec(),
        ToolNamespace::Builtin,
    )?;

    Ok(())
}
