use std::sync::Arc;

use crate::memory_manager::MemoryManager;

pub fn register_memory_tools(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    crate::memory_search_tool::register_memory_search_tool(toolkit, Arc::clone(&memory_manager));
    crate::memory_get_tool::register_memory_get_tool(toolkit, Arc::clone(&memory_manager));
    crate::memory_write_tool::register_memory_write_tool(toolkit, Arc::clone(&memory_manager));
    crate::memory_update_tool::register_memory_update_tool(toolkit, Arc::clone(&memory_manager));
    crate::memory_delete_tool::register_memory_delete_tool(toolkit, memory_manager);
}
