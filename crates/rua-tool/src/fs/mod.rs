pub(crate) mod atomic_write;
pub(crate) mod workspace_fs;

pub use atomic_write::{atomic_write, detect_newline_style, normalize_newlines, NewlineStyle};
pub use workspace_fs::WorkspaceFs;
