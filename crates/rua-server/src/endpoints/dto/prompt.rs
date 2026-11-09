pub use rua_prompts::library::{
    PromptBroadcastRequest, PromptBroadcastResponse, PromptCreate, PromptEntry,
    PromptLibrarySyncRequest, PromptLibrarySyncResponse, PromptUpdate,
};

use super::{FileContent, FileEntry};

impl From<rua_prompts::library::FileEntry> for FileEntry {
    fn from(v: rua_prompts::library::FileEntry) -> Self {
        Self {
            name: v.name,
            path: v.path,
            size: v.size,
            modified: v.modified,
            created_time: v.created_time,
            builtin: v.builtin,
            extra: v.extra,
        }
    }
}

impl From<rua_prompts::library::FileContent> for FileContent {
    fn from(v: rua_prompts::library::FileContent) -> Self {
        Self {
            name: v.name,
            path: v.path,
            content: v.content,
            size: v.size,
            extra: v.extra,
        }
    }
}
