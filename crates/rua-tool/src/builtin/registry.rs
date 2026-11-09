use std::sync::Arc;

use crate::error::ToolError;
use crate::executor::CommandExecutor;
use crate::registry::ToolRegistry;

use super::SandboxLimits;

pub fn register_all_builtin_tools(
    registry: &mut ToolRegistry,
    executor: Arc<dyn CommandExecutor>,
    sandbox_limits: Option<SandboxLimits>,
) -> Result<(), ToolError> {
    registry.register_core_tools(Arc::clone(&executor))?;
    super::register_builtin_tools(registry, executor, sandbox_limits)?;
    Ok(())
}
