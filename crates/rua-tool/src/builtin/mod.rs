pub mod acp_permission_evaluator;
pub mod execute_python_code;
pub mod phone_use;
pub mod registry;
pub mod runtime_environment;

pub use acp_permission_evaluator::{AcpPermissionTaskContext, SecurityPermissionEvaluator};
pub use execute_python_code::{ExecutePythonCodeTool, SandboxLimits};
pub use registry::register_all_builtin_tools;
pub use runtime_environment::RuntimeGetRuntimeInfoTool;

use std::sync::Arc;

use crate::registry::{ToolNamespace, ToolRegistry};

pub(crate) fn register_builtin_tools(
    registry: &mut ToolRegistry,
    executor: Arc<dyn crate::executor::CommandExecutor>,
    sandbox_limits: Option<SandboxLimits>,
) -> Result<(), crate::error::ToolError> {
    // Catalog construction also runs in `rua init`, before the resident handshake.
    // Execution still requires the authenticated native bridge in each tool call.
    if cfg!(target_os = "android") {
        registry.register(
            Arc::new(phone_use::PhoneStatusTool),
            phone_use::PhoneStatusTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(phone_use::PhoneObserveTool),
            phone_use::PhoneObserveTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(phone_use::PhoneClickTool),
            phone_use::PhoneClickTool::spec(),
            ToolNamespace::Builtin,
        )?;
        registry.register(
            Arc::new(phone_use::PhoneListAppsTool),
            phone_use::PhoneListAppsTool::spec(),
            ToolNamespace::Builtin,
        )?;
        for capture in [false, true] {
            registry.register(
                Arc::new(phone_use::PhoneCameraTool { capture }),
                phone_use::PhoneCameraTool::spec(capture),
                ToolNamespace::Builtin,
            )?;
        }
        for action in [
            phone_use::PhoneAction::LongPress,
            phone_use::PhoneAction::TypeText,
            phone_use::PhoneAction::Swipe,
            phone_use::PhoneAction::Back,
            phone_use::PhoneAction::Home,
            phone_use::PhoneAction::LaunchApp,
        ] {
            registry.register(
                Arc::new(phone_use::PhoneActionTool(action)),
                phone_use::PhoneActionTool::spec(action),
                ToolNamespace::Builtin,
            )?;
        }
    }
    let tool = ExecutePythonCodeTool::with_executor(executor, sandbox_limits);
    registry.register(
        Arc::new(tool),
        ExecutePythonCodeTool::spec(),
        ToolNamespace::Builtin,
    )?;
    registry.register(
        Arc::new(RuntimeGetRuntimeInfoTool::new()),
        RuntimeGetRuntimeInfoTool::spec(),
        ToolNamespace::Builtin,
    )?;

    Ok(())
}
